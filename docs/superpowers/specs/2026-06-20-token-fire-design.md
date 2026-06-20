# TokenFire Design

Date: 2026-06-20

## Goal

Build an independent macOS app that shows AI token usage in a floating widget.

The first version supports Traex only. It does not integrate with Flux Island, does not use Flux sockets, does not write Flux data, and does not depend on the Flux process. It reuses the same architectural idea: hooks trigger local collection, local files are parsed for token data, and a local UI renders usage.

## Non-Goals

- No network upload.
- No prompt, response, tool argument, or file-content storage.
- No Flux socket, manifest, config, or data-directory integration.
- No first-version support for Claude Code, Codex, Cursor, OpenCode, Gemini, or Copilot.
- No bill-grade guarantee if Traex itself does not write `token_count` records.

## Architecture

TokenFire has two process entry points:

- `token-fire`: Tauri macOS app. It owns the menu bar item, floating widget, local socket, file watcher, SQLite database, logging, and settings.
- `token-fire-hook`: small CLI installed into Traex hooks. It reads hook payload JSON from stdin and forwards minimal metadata to the running app.

Core modules:

- `traex adapter`: resolves and parses Traex JSONL session files.
- `usage store`: writes token observations and serves aggregate totals from SQLite.
- `watcher`: monitors Traex session directories for writes, creates, moves, and renames.
- `reconcile`: periodically checks recently modified session files while the app is running.
- `debug logger`: writes structured logs and supports future debugger UI.
- `floating widget`: shows token totals with meter-style animation.

Data flow:

```text
Traex hook
  -> token-fire-hook
  -> ~/.token-fire/run/token-fire.sock
  -> token-fire app
  -> traex adapter
  -> SQLite observations
  -> menu bar + floating widget
```

## Traex Token Collection

The first version listens to Traex `Stop` hooks. It does not use `SessionEnd` as a token event in v1, to avoid duplicate reporting.

`token-fire-hook` keeps only these fields:

- `source`
- `hook_event_name`
- `session_id`
- `transcript_path`
- `turn_id`
- `model`
- `cwd`
- `timestamp`

The hook is a trigger, not the source of truth. The source of truth is the local Traex JSONL file under:

- `~/.trae/cli/sessions`
- `~/.trae/cli/archived_sessions`

The adapter reads `event_msg.payload.type === "token_count"` records and extracts:

- `last_token_usage.input_tokens`
- `last_token_usage.output_tokens`
- `last_token_usage.cached_input_tokens`
- `last_token_usage.cache_creation_input_tokens`
- `last_token_usage.reasoning_output_tokens`
- `last_token_usage.total_tokens`
- `total_token_usage` for fallback and validation

The parser should follow the Flux Codex approach:

1. Read the rollout JSONL from the end.
2. Find the latest `user_message` boundary.
3. Collect `token_count` records after that user message.
4. Find the preceding `token_count` as the baseline.
5. Prefer summing non-zero `last_token_usage`.
6. If `last_token_usage` is missing or zero, use `total_token_usage - baseline`.
7. Return one observation for the latest turn.

This handles multiple `token_count` records in one turn and keeps a fallback when only cumulative totals are available.

## File Resolution

When a hook event arrives, the app resolves the transcript in this order:

1. Use `payload.transcript_path` if it exists.
2. Search `~/.trae/cli/sessions` for a filename containing `session_id`.
3. Search `~/.trae/cli/archived_sessions` for a filename containing `session_id`.

If the file is missing, the event is logged as `warn` and retried by watcher/reconcile when a matching file appears.

## Reliability Model

TokenFire does not rely on a single hook delivery.

It combines:

- Traex `Stop` hook as the low-latency trigger.
- FSEvents watching of Traex session directories.
- A short periodic reconcile of recently modified session files while the app is running.
- SQLite idempotency through a stable dedupe key.

The guarantee is:

If Traex writes a `token_count` record into a readable local session file while TokenFire is running or shortly after a hook event, TokenFire should eventually record it once.

The app cannot guarantee statistics when:

- Traex does not write `token_count`.
- The Traex session directory is unreadable.
- The session file is deleted before TokenFire reads it.
- The user removes the hook and disables file access.

These cases appear in the UI status and logs.

## Pending Hooks

If `token-fire-hook` cannot connect to the local socket, it writes a minimal pending event to:

```text
~/.token-fire/pending-hooks.jsonl
```

The app replays pending events when it is running. Pending hooks are a fallback, not the normal path.

Large directory scans are not part of normal startup. A menu action named `Repair Today's Stats` may run a bounded scan for manual repair.

## Storage

SQLite stores observations as the fact table. Aggregates are derived from observations.

Table: `token_observations`

- `id`
- `source`
- `session_id`
- `turn_id`
- `transcript_path`
- `line_no`
- `byte_offset`
- `input_tokens`
- `output_tokens`
- `cached_input_tokens`
- `cache_creation_input_tokens`
- `reasoning_output_tokens`
- `total_tokens`
- `model`
- `cwd`
- `created_at`
- `dedupe_key`

`dedupe_key` is:

```text
source + session_id + byte_offset + sha256(token_count_payload)
```

SQLite has a unique index on `dedupe_key`. Repeated hook delivery, repeated FSEvents, or repeated reconcile runs must not duplicate totals.

Daily and recent totals are computed from observations:

```sql
select sum(total_tokens)
from token_observations
where created_at >= :start_of_day;
```

Local day boundaries use the user's current system timezone.

## UI

The app uses Tauri.

It has two visible surfaces:

- Menu bar item: shows today's compact token count, such as `128K`.
- Floating widget: can be pinned and shows live usage.

Floating widget content:

- Today's total token count as the largest number.
- Meter-style rolling digits for total changes.
- Latest turn delta, such as `+12,430`.
- Recent three-day usage.
- Status indicator:
  - Green: watcher, hook, parser, and DB are healthy.
  - Yellow: app is running but hook is missing, no recent events have arrived, or a non-critical fallback is active.
  - Red: session directory unreadable, socket failed, or SQLite writes failed.

Menu items:

- Show or hide floating widget.
- Pin on all desktops.
- Pause tracking.
- Repair today's stats.
- Open data directory.
- Open logs directory.
- Settings.
- Quit.

Animations are display-only. Computation always comes from SQLite aggregates.

## Hook Installation

TokenFire does not overwrite Traex hooks. It appends its own hook to `~/.trae/traecli.toml`.

On first launch, the app checks whether the TokenFire hook is installed. If not, it shows an `Install Traex Hook` action.

Before editing `~/.trae/traecli.toml`, TokenFire creates a backup:

```text
~/.token-fire/backups/traecli.toml.<timestamp>
```

The v1 hook is:

```toml
[[hooks.Stop.hooks]]
command = "'/Applications/TokenFire.app/Contents/MacOS/token-fire-hook' --source traex"
type = "command"
timeout = 5
```

Compatibility rules:

- Preserve existing Flux hooks.
- Preserve all non-TokenFire hooks.
- Append TokenFire after existing `Stop` hooks.
- `token-fire-hook` exits 0 even on internal failure.
- Uninstall removes only the TokenFire-marked hook.
- Hook commands use absolute paths.

Status fields:

- `hook_installed`
- `hook_executable_exists`
- `hook_last_seen_at`
- `last_hook_error`

## Logging

TokenFire uses structured JSONL logs.

Log files:

- `~/.token-fire/logs/app.log`
- `~/.token-fire/logs/hook.log`
- `~/.token-fire/logs/parser.log`
- `~/.token-fire/logs/db.log`

Example:

```json
{
  "ts": "2026-06-20T12:00:00.000+08:00",
  "level": "info",
  "component": "parser",
  "event": "token_count_parsed",
  "source": "traex",
  "session_id": "019...",
  "turn_id": "turn...",
  "transcript_path": "/Users/.../.trae/cli/sessions/...",
  "byte_offset": 123456,
  "input_tokens": 1000,
  "output_tokens": 200,
  "total_tokens": 1200
}
```

Levels:

- `error`: SQLite write failure, unreadable session directory, socket bind failure.
- `warn`: stale transcript path, socket unavailable, JSONL partial line, duplicate observation.
- `info`: hook received, file changed, token count parsed, observation inserted.
- `debug`: parser boundary, byte offset, baseline, delta calculation.

Default logging writes `info`, `warn`, and `error`. Debug logging can be enabled for 30 minutes from settings and then automatically disabled.

Core events:

- `hook_received`
- `hook_forwarded`
- `hook_socket_unavailable`
- `watch_started`
- `file_changed`
- `transcript_resolved`
- `transcript_stale_fallback`
- `token_count_seen`
- `turn_boundary_found`
- `token_delta_computed`
- `observation_inserted`
- `observation_duplicate`
- `daily_total_updated`
- `reconcile_started`
- `reconcile_completed`
- `error_unreadable_session_dir`

## Debug Bundle

The menu includes `Copy Debug Bundle`.

The bundle contains:

- Recent 200 lines from app, hook, parser, and DB logs.
- Recent 20 observation metadata rows.
- Current watcher status.
- Hook install status.
- Traex directory readability status.
- SQLite health status.

The bundle must not include prompt, response, tool arguments, or file contents.

Future debugger page:

- Recent event stream.
- Recent session parse status.
- Today's aggregate totals versus raw observations.
- Duplicate and failed event list.

## Error Handling

- Hook malformed payload: log `warn`, exit 0.
- Hook socket unavailable: write pending event, exit 0.
- Socket bind failure: log `error`, show red UI status.
- Transcript path stale: search by `session_id`.
- JSONL partial line: skip and retry after more bytes arrive.
- JSON parse failure: log the line metadata, not content, then continue.
- SQLite unique violation: treat as duplicate observation.
- Traex directory unreadable: show red status.
- Hook not installed: show yellow status, continue watcher/reconcile tracking.

## Privacy

TokenFire is local-first.

It stores:

- token counts
- model
- cwd
- session id
- transcript path
- timestamps
- parser metadata such as offsets

It does not store:

- user prompts
- assistant responses
- tool arguments
- command outputs
- file contents

It does not send network requests in v1.

## Testing

Parser tests:

- Parse `token_count`.
- Sum multiple `token_count` records in one turn.
- Prefer `last_token_usage`.
- Fall back to `total_token_usage - baseline`.
- Skip JSONL partial lines.
- Resolve archived transcript paths.

Database tests:

- `dedupe_key` uniqueness.
- Duplicate observations do not change totals.
- Today's aggregate is correct.
- Recent three-day aggregate is correct.
- Local timezone day boundaries are correct.

Hook integration tests:

- stdin payload forwards to socket.
- socket unavailable writes pending file.
- malformed payload exits 0 and logs warn.

Watcher and reconcile tests:

- file append triggers parsing.
- moved archived file can still resolve.
- duplicate FSEvents do not duplicate observations.

UI smoke tests:

- today's number updates.
- latest `+N` delta appears.
- status indicator switches between green, yellow, and red.
- debug bundle excludes prompt, response, tool arguments, and file contents.

Fixtures:

- Add a sanitized `fixtures/traex-session.jsonl` containing representative `user_message` and `token_count` records.

## Open Decisions

No open product decisions remain for v1.

Implementation planning should decide exact Rust crates, Tauri plugin choices, and schema migration tooling.
