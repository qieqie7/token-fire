# TokenFire Product Direction

Last reviewed: 2026-07-08

## Product Positioning

TokenFire should no longer be treated as a Traex-only token widget.

The current product direction is a local macOS AI usage instrument:

- it lives in the menubar;
- it collects token metadata from TraeX, Codex, Claude, and Cursor;
- it stores usage locally in SQLite;
- it estimates cost, attributes usage, and helps diagnose missing or surprising data;
- it keeps prompts, responses, tool payloads, command output, and raw transcripts out of storage and exports.

The compact Profile is the primary review surface. The native tray menu is the maintenance surface.

## First Principles

TokenFire should answer three questions:

1. How much am I using or burning in the current period?
2. Which source, model, and token category caused the usage or estimated cost?
3. If the data looks wrong, why was it missing, duplicated, delayed, unknown, or surprising?

The next iteration should prioritize trust, explanation, and repair over more charts or more source integrations.

## Current Code Facts

Confirmed in the current codebase:

- `src-tauri/src/core/` owns source-neutral accounting, dedupe, SQLite storage, pricing, retention, and profile aggregation.
- `src-tauri/src/adapters/` owns source-specific behavior for TraeX, Codex, Claude, and Cursor.
- `src-tauri/src/app/` owns runtime orchestration, tray behavior, source health, source ingest routing, debug bundles, logging, and UI invalidation.
- `TokenSourceKind` currently includes `Traex`, `Codex`, `Claude`, and `Cursor`.
- `SourceIngestRouter` exists and routes hook/file events by source.
- Cursor uses path-first ingestion when hook metadata provides a transcript path.
- `usage_facts_invalidated` is the canonical fact invalidation event for derived UI refresh.
- Profile has a 365-day heatmap, natural periods, estimated cost, token count, app source breakdown, and model breakdown.
- `ProfileCostDrivers` exists in the backend and TypeScript types, but the current Profile UI does not render it.
- Pricing core has pricing status concepts such as rule/fallback/mixed, but Profile does not currently expose pricing status.
- Debug bundles already include redacted logs, recent observation metadata, source statuses, runtime health, and SQLite metadata.
- `Cargo.toml` still describes the app as `Local Traex token usage widget`, which is outdated.

## Recommended Iteration Order

### 1. Source Health Center

Build a lightweight source health surface before expanding analytics.

Each source should expose:

- enabled or disabled state;
- config detected;
- hook registered;
- hook executable exists;
- hook smoke status;
- latest hook seen time when available;
- latest ingest time when available;
- latest safe error category;
- directory or transcript readability status when available;
- actions such as install, reinstall, open logs, copy debug bundle, and enable debug logging.

Keep source health in `app` and `adapters`. Do not move source-specific file layout or hook config knowledge into `core`.

Health status must not collapse everything into one ambiguous red/yellow/green state. Hook registration, executable presence, directory readability, smoke status, and recent ingest are different facts.

Disabled or absent optional sources must not degrade global health.

### 2. Ingestion Ledger MVP

Build a recent ingestion ledger without a schema migration first.

The existing system already provides enough for an MVP:

- `recent_observation_metadata` can show successfully inserted observations.
- `source_ingested` logs include source, hook event name, metadata presence flags, resolution, inserted count, duplicate count, and skipped-outside-tracking count.
- `source_collect_empty` logs include source, hook event name, metadata presence flags, resolution, empty reason, inserted count, duplicate count, and skipped-outside-tracking count.

The ledger should distinguish:

- usage event time: `observed_at`;
- ingestion time: `created_at`;
- inserted rows;
- duplicate-only ingestion;
- outside-tracking-window skips;
- empty collection reasons;
- transcript path versus conversation id versus source registry resolution.

Only add a durable `ingestion_events`-style schema if the product needs long-term pagination, filtering, or auditing of attempts that did not insert observations.

### 3. Profile Explanation

Profile already answers "how much". Next it should explain "why".

Add compact explanations for:

- input cost;
- output cost;
- reasoning output cost;
- cache creation input cost;
- cached input cost;
- unattributed cost;
- cached input tokens;
- cache read ratio;
- pricing rule status, fallback status, or mixed status;
- Unknown model/source share and a safe explanation of likely causes;
- peak day, peak source, and peak model for the selected period.

The backend contract should expose typed fields first. The frontend should not infer technical causes from labels alone.

### 4. Positioning And Naming Cleanup

Align long-lived wording with the real product:

- replace Traex-only widget wording with local multi-source AI usage instrument wording;
- keep Profile as the usage review surface;
- keep tray as the maintenance surface;
- clean up old widget naming gradually when touching related code;
- avoid large refactors for naming alone.

## Deferred Directions

Do not prioritize these until source trust and explanation are solid:

- cloud sync;
- accounts;
- team dashboards;
- full BI dashboard;
- raw token table;
- exact billing engine;
- complex provider settlement simulation;
- broad new source expansion.

These directions change the product from a local instrument into a data platform. They should require a separate design.

## Engineering Constraints

- Keep `core` source-neutral.
- Keep source file layout, hook config, transcript resolution, and watermark behavior in `adapters` or `app`.
- Do not persist or export raw prompts, responses, tool arguments, command output, or transcript contents.
- Do not show full local `source_path` or `cwd` in Profile, Health Center, or Ledger UI.
- Use redacted, structured metadata for health, ledger, and debug output.
- Use `observed_at` for usage statistics.
- Use `created_at` for ingestion diagnostics.
- Do not mix usage time and ingestion time in cost or period totals.
- Prefer typed backend contracts and Rust tests before adding React presentation.
- Treat pricing as local estimated visibility, not billing settlement.

## User Intent Notes

Reusable user intent from prior product discussions:

- "点击 menubar 的时候，出来的是一个 profile，上面是最近一年使用的情况（有点像 github commit 的那个表格...）"
- "下面可能是多个维度的聚合数据（token 数，来源分布，等能力），有一个 filter 能力"
- "这个账单是计算的玩的，只是看个大致，不需要那么精准"

These imply:

- Profile should stay compact and review-oriented.
- The 365-day heatmap is a stable identity layer.
- Cost is estimated and explanatory, not bill-grade.
- Source distribution and model attribution are core product dimensions.

## Agent Guidance

When proposing TokenFire changes:

1. First check whether the change improves trust, explanation, or repair.
2. Prefer Source Health Center, Ingestion Ledger, and Profile explanation before more charts or more sources.
3. Do not reintroduce duplicate maintenance controls into Profile when the tray already owns them.
4. Do not treat Claude or Cursor as installable sidecar hooks in the same sense as TraeX/Codex unless the current code confirms that behavior.
5. Before changing pricing logic, verify token field semantics and whether component fields are present.
6. Before changing ingestion logic, preserve dedupe, tracking window, watermark, and privacy boundaries.
7. Use codebase-memory-mcp graph tools first for code discovery in this repository.
