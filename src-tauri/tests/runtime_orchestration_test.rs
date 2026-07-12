use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use chrono::{Datelike, TimeZone, Utc};
use serde_json::Value;
use tempfile::tempdir;
use token_fire::adapters::source::{SourcePaths, SourceRegistry, TokenSourceKind};
use token_fire::adapters::traex::resolver::TraexPaths;
use token_fire::adapters::traex::watcher::watch_roots;
use token_fire::adapters::HookMetadata;
use token_fire::app::hook_intake::handle_traex_hook_metadata;
use token_fire::app::ingest_scheduler::IngestScheduler;
use token_fire::app::logging::{DebugLogGate, RuntimeLogger};
use token_fire::app::paths::RuntimePaths;
use token_fire::app::runtime::{
    activate_runtime_tracking_window, baseline_existing_source_files, handle_runtime_event,
    handle_runtime_event_with_logger, start_app_runtime_for_state, token_fire_hook_path_from_exe,
    AppRuntime, RuntimeEvent,
};
use token_fire::app::source_ingest::{SourceEmptyReason, SourceIngestPaths, SourceIngestRouter};
use token_fire::app::state::AppState;
use token_fire::app::tracking::TrackingGate;
use token_fire::app::usage_invalidation::notify_usage_facts_invalidated;
use token_fire::app::widget_events::{
    emit_usage_fact_invalidation_events_with, usage_fact_invalidation_event_names,
    UsageFactsInvalidatedEvent, WidgetEventEmitter, PROFILE_SUMMARY_CHANGED_EVENT,
    PROFILE_WINDOW_FOCUSED_EVENT, USAGE_FACTS_INVALIDATED_EVENT, WIDGET_STATE_CHANGED_EVENT,
};
use token_fire::core::usage_store::{RetentionPolicy, UsageStore};

fn paths(home: &Path) -> RuntimePaths {
    let run_dir = home.join("run");
    let logs_dir = home.join("logs");
    RuntimePaths {
        database: home.join("token-fire.sqlite"),
        socket: run_dir.join("token-fire.sock"),
        app_log: logs_dir.join("app.log"),
        hook_log: logs_dir.join("hook.log"),
        parser_log: logs_dir.join("parser.log"),
        db_log: logs_dir.join("db.log"),
        backups_dir: home.join("backups"),
        debug_bundles_dir: home.join("debug-bundles"),
        home: home.to_path_buf(),
        run_dir,
        logs_dir,
    }
}

fn find_app_log_event(app_log: &str, event: &str) -> Value {
    app_log
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|value| value["event"] == event)
        .unwrap_or_else(|| panic!("expected {event} log"))
}

#[test]
fn widget_event_names_keep_usage_facts_and_legacy_aliases() {
    assert_eq!(USAGE_FACTS_INVALIDATED_EVENT, "usage_facts_invalidated");
    assert_eq!(PROFILE_SUMMARY_CHANGED_EVENT, "profile_summary_changed");
    assert_eq!(PROFILE_WINDOW_FOCUSED_EVENT, "profile_window_focused");
    assert_eq!(WIDGET_STATE_CHANGED_EVENT, "widget_state_changed");
    assert_eq!(
        usage_fact_invalidation_event_names(),
        [
            USAGE_FACTS_INVALIDATED_EVENT,
            PROFILE_SUMMARY_CHANGED_EVENT,
            WIDGET_STATE_CHANGED_EVENT,
        ]
    );
}

#[test]
fn usage_fact_invalidation_emits_canonical_event_and_legacy_aliases() {
    let payload = UsageFactsInvalidatedEvent {
        state_revision: 7,
        last_observed_at: Some(Utc.with_ymd_and_hms(2026, 7, 7, 9, 0, 0).unwrap()),
        inserted: 3,
    };
    let emitted: Arc<Mutex<Vec<(&'static str, UsageFactsInvalidatedEvent)>>> =
        Arc::new(Mutex::new(Vec::new()));

    emit_usage_fact_invalidation_events_with(&payload, {
        let emitted = Arc::clone(&emitted);
        move |event_name, event_payload| {
            emitted
                .lock()
                .unwrap()
                .push((event_name, event_payload.clone()));
        }
    });

    let emitted = emitted.lock().unwrap();
    assert_eq!(emitted.len(), 3);
    assert_eq!(
        emitted
            .iter()
            .map(|(event_name, _)| *event_name)
            .collect::<Vec<_>>(),
        vec![
            USAGE_FACTS_INVALIDATED_EVENT,
            PROFILE_SUMMARY_CHANGED_EVENT,
            WIDGET_STATE_CHANGED_EVENT,
        ]
    );
    assert!(emitted
        .iter()
        .all(|(_, event_payload)| event_payload == &payload));
}

#[test]
fn usage_fact_invalidation_emits_before_refreshing_surfaces() {
    let payload = UsageFactsInvalidatedEvent {
        state_revision: 8,
        last_observed_at: Some(Utc.with_ymd_and_hms(2026, 7, 7, 10, 0, 0).unwrap()),
        inserted: 4,
    };
    let expected_payload = payload.clone();
    let calls: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));

    notify_usage_facts_invalidated(
        &payload,
        {
            let calls = Arc::clone(&calls);
            move |event_payload| {
                assert_eq!(event_payload, &expected_payload);
                calls.lock().unwrap().push("emit");
            }
        },
        {
            let calls = Arc::clone(&calls);
            move || {
                calls.lock().unwrap().push("refresh");
                Ok::<(), &'static str>(())
            }
        },
    );

    assert_eq!(*calls.lock().unwrap(), vec!["emit", "refresh"]);
}

#[test]
fn usage_fact_invalidation_refresh_failure_does_not_suppress_event_emission() {
    let payload = UsageFactsInvalidatedEvent {
        state_revision: 9,
        last_observed_at: Some(Utc.with_ymd_and_hms(2026, 7, 7, 11, 0, 0).unwrap()),
        inserted: 5,
    };
    let emitted = Arc::new(AtomicBool::new(false));
    let refreshed = Arc::new(AtomicBool::new(false));

    notify_usage_facts_invalidated(
        &payload,
        {
            let emitted = Arc::clone(&emitted);
            move |_| {
                emitted.store(true, Ordering::SeqCst);
            }
        },
        {
            let refreshed = Arc::clone(&refreshed);
            move || {
                refreshed.store(true, Ordering::SeqCst);
                Err::<(), &'static str>("tray refresh failed")
            }
        },
    );

    assert!(emitted.load(Ordering::SeqCst));
    assert!(refreshed.load(Ordering::SeqCst));
}

fn runtime_observation(
    source_record_id: &str,
    total_tokens: i64,
    observed_at: chrono::DateTime<Utc>,
) -> token_fire::core::observation::NormalizedObservation {
    token_fire::core::observation::NormalizedObservation {
        source: "traex".to_string(),
        adapter_version: "traex-jsonl-v1".to_string(),
        source_record_id: source_record_id.to_string(),
        source_record_id_confidence: token_fire::core::observation::SourceRecordIdConfidence::Exact,
        session_id: Some("session-a".to_string()),
        turn_id: Some("turn-a".to_string()),
        turn_boundary_id: Some("turn-a".to_string()),
        source_path: Some("/tmp/rollout-session-a.jsonl".to_string()),
        line_no: Some(1),
        byte_offset: Some(10),
        input_tokens: total_tokens,
        output_tokens: 0,
        cached_input_tokens: 0,
        cache_creation_input_tokens: 0,
        reasoning_output_tokens: 0,
        total_tokens,
        cumulative_total_tokens: Some(total_tokens),
        model: Some("model-a".to_string()),
        cwd: Some("~/project".to_string()),
        observed_at,
        token_payload_hash: format!("runtime-hash-{source_record_id}"),
    }
}

#[test]
fn app_state_resume_and_pause_open_and_close_tracking_windows() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path());
    let state = AppState::new(paths.clone());
    let started = Utc.with_ymd_and_hms(2026, 6, 20, 3, 0, 0).unwrap();
    let ended = Utc.with_ymd_and_hms(2026, 6, 20, 4, 0, 0).unwrap();

    state.resume_tracking_at(started).unwrap();
    let store = UsageStore::open(&paths.database).unwrap();
    assert_eq!(store.active_tracking_windows().unwrap().len(), 1);

    state.pause_tracking_at(ended).unwrap();
    let store = UsageStore::open(&paths.database).unwrap();
    assert_eq!(store.active_tracking_windows().unwrap().len(), 0);
}

#[test]
fn runtime_event_ingests_hook_and_watcher_paths_into_sqlite() {
    let dir = tempdir().unwrap();
    let transcript = dir
        .path()
        .join("sessions/2026/06/20/rollout-019-session-a.jsonl");
    fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    fs::write(&transcript, include_str!("fixtures/traex-session.jsonl")).unwrap();

    let store = UsageStore::open(&dir.path().join("token-fire.sqlite")).unwrap();
    store
        .open_tracking_window(Utc.with_ymd_and_hms(2026, 6, 20, 3, 0, 0).unwrap())
        .unwrap();
    let scheduler = IngestScheduler::new(store);
    let traex_paths = TraexPaths {
        sessions_dir: dir.path().join("sessions"),
        archived_sessions_dir: dir.path().join("archived_sessions"),
    };
    let metadata = HookMetadata {
        hook_event_name: Some("Stop".to_string()),
        transcript_path: Some(transcript.to_string_lossy().to_string()),
        session_id: Some("019-session-a".to_string()),
        ..HookMetadata::default()
    };

    let gate = TrackingGate::new();
    let hook_report = handle_runtime_event(
        RuntimeEvent::Hook {
            source: TokenSourceKind::Traex,
            metadata,
        },
        &SourceRegistry::new(vec![SourcePaths::new(
            TokenSourceKind::Traex,
            traex_paths.sessions_dir.clone(),
            traex_paths.archived_sessions_dir.clone(),
        )]),
        &scheduler,
        &gate,
    )
    .unwrap()
    .expect("hook report");
    assert_eq!(hook_report.inserted, 2);

    let watcher_report = handle_runtime_event(
        RuntimeEvent::TranscriptChanged {
            source: TokenSourceKind::Traex,
            path: transcript.clone(),
        },
        &SourceRegistry::new(vec![SourcePaths::new(
            TokenSourceKind::Traex,
            traex_paths.sessions_dir.clone(),
            traex_paths.archived_sessions_dir.clone(),
        )]),
        &scheduler,
        &gate,
    )
    .unwrap()
    .expect("watcher report");
    assert_eq!(watcher_report.inserted, 0);
    assert_eq!(watcher_report.duplicates, 0);
}

#[test]
fn runtime_event_preserves_codex_source_for_watcher_paths() {
    let dir = tempdir().unwrap();
    let transcript = dir
        .path()
        .join("codex/sessions/2026/06/26/rollout-019-codex-session.jsonl");
    fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    fs::write(&transcript, include_str!("fixtures/codex-session.jsonl")).unwrap();
    let store = UsageStore::open(&dir.path().join("token-fire.sqlite")).unwrap();
    store
        .open_tracking_window(Utc.with_ymd_and_hms(2026, 6, 26, 2, 59, 0).unwrap())
        .unwrap();
    let scheduler = IngestScheduler::new(store);
    let registry = SourceRegistry::new(vec![SourcePaths::new(
        TokenSourceKind::Codex,
        dir.path().join("codex/sessions"),
        dir.path().join("codex/archived_sessions"),
    )]);
    let gate = TrackingGate::new();

    let report = handle_runtime_event(
        RuntimeEvent::TranscriptChanged {
            source: TokenSourceKind::Codex,
            path: transcript,
        },
        &registry,
        &scheduler,
        &gate,
    )
    .unwrap()
    .expect("watcher report");

    assert_eq!(report.inserted, 2);
}

#[test]
fn runtime_polling_ingests_active_traex_file_without_notify_event() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path());
    let sessions_dir = dir.path().join("sessions");
    let archived_sessions_dir = dir.path().join("archived_sessions");
    let observed_at = Utc::now() + chrono::Duration::seconds(30);
    let transcript = sessions_dir
        .join(format!("{:04}", observed_at.year()))
        .join(format!("{:02}", observed_at.month()))
        .join(format!("{:02}", observed_at.day()))
        .join("rollout-019-active-session.jsonl");
    fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    fs::create_dir_all(&archived_sessions_dir).unwrap();
    fs::write(
        &transcript,
        format!(
            "{{\"type\":\"session_meta\",\"timestamp\":\"{}\",\"payload\":{{\"id\":\"019-active-session\"}}}}\n\
             {{\"type\":\"turn_context\",\"timestamp\":\"{}\",\"payload\":{{\"turn_id\":\"turn-active\",\"model\":\"model-a\",\"cwd\":\"/tmp/project\"}}}}\n\
             {{\"type\":\"event_msg\",\"timestamp\":\"{}\",\"payload\":{{\"type\":\"token_count\",\"info\":{{\"last_token_usage\":{{\"input_tokens\":7,\"output_tokens\":3,\"cached_input_tokens\":0,\"cache_creation_input_tokens\":0,\"reasoning_output_tokens\":0,\"total_tokens\":10}},\"total_token_usage\":{{\"input_tokens\":7,\"output_tokens\":3,\"cached_input_tokens\":0,\"cache_creation_input_tokens\":0,\"reasoning_output_tokens\":0,\"total_tokens\":10}}}}}}}}\n",
            observed_at.to_rfc3339(),
            observed_at.to_rfc3339(),
            observed_at.to_rfc3339()
        ),
    )
    .unwrap();
    let registry = SourceRegistry::new(vec![SourcePaths::new(
        TokenSourceKind::Traex,
        sessions_dir,
        archived_sessions_dir,
    )]);

    let runtime = AppRuntime::start_with_sources(
        paths.clone(),
        registry,
        TrackingGate::new(),
        DebugLogGate::default(),
    )
    .unwrap();

    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        let count = rusqlite::Connection::open(&paths.database)
            .unwrap()
            .query_row("select count(*) from token_observations", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap();
        if count == 1 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "active TraeX file was not ingested without a notify event"
        );
        thread::sleep(Duration::from_millis(50));
    }

    drop(runtime);
}

#[test]
fn runtime_emits_usage_facts_invalidated_after_inserted_ingest() {
    let dir = tempdir().unwrap();
    let transcript = dir
        .path()
        .join("sessions/2026/06/20/rollout-019-session-a.jsonl");
    fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    fs::write(&transcript, include_str!("fixtures/traex-session.jsonl")).unwrap();
    let store = UsageStore::open(&dir.path().join("token-fire.sqlite")).unwrap();
    store
        .open_tracking_window(Utc.with_ymd_and_hms(2026, 6, 20, 3, 0, 0).unwrap())
        .unwrap();
    let scheduler = IngestScheduler::new(store);
    let registry = SourceRegistry::new(vec![SourcePaths::new(
        TokenSourceKind::Traex,
        dir.path().join("sessions"),
        dir.path().join("archived_sessions"),
    )]);
    let gate = TrackingGate::new();
    let emitted: Arc<Mutex<Vec<UsageFactsInvalidatedEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let emitter = WidgetEventEmitter::from_fn({
        let emitted = Arc::clone(&emitted);
        move |payload| emitted.lock().unwrap().push(payload)
    });

    let report = token_fire::app::runtime::handle_runtime_event_with_logger_and_widget_events(
        RuntimeEvent::TranscriptChanged {
            source: TokenSourceKind::Traex,
            path: transcript.clone(),
        },
        &registry,
        &scheduler,
        &gate,
        &RuntimeLogger::new(paths(dir.path()), DebugLogGate::default()),
        &emitter,
    )
    .unwrap()
    .expect("insert report");

    assert_eq!(report.inserted, 2);
    let events = emitted.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].inserted, 2);
    assert_eq!(events[0].state_revision, 2);
    assert!(events[0].last_observed_at.is_some());
}

#[test]
fn runtime_does_not_emit_usage_facts_invalidated_for_duplicate_only_ingest() {
    let dir = tempdir().unwrap();
    let transcript = dir
        .path()
        .join("sessions/2026/06/20/rollout-019-session-a.jsonl");
    fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    fs::write(&transcript, include_str!("fixtures/traex-session.jsonl")).unwrap();
    let store = UsageStore::open(&dir.path().join("token-fire.sqlite")).unwrap();
    store
        .open_tracking_window(Utc.with_ymd_and_hms(2026, 6, 20, 3, 0, 0).unwrap())
        .unwrap();
    let scheduler = IngestScheduler::new(store);
    let registry = SourceRegistry::new(vec![SourcePaths::new(
        TokenSourceKind::Traex,
        dir.path().join("sessions"),
        dir.path().join("archived_sessions"),
    )]);
    let gate = TrackingGate::new();
    let emitted: Arc<Mutex<Vec<UsageFactsInvalidatedEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let emitter = WidgetEventEmitter::from_fn({
        let emitted = Arc::clone(&emitted);
        move |payload| emitted.lock().unwrap().push(payload)
    });
    let logger = RuntimeLogger::new(paths(dir.path()), DebugLogGate::default());

    token_fire::app::runtime::handle_runtime_event_with_logger_and_widget_events(
        RuntimeEvent::TranscriptChanged {
            source: TokenSourceKind::Traex,
            path: transcript.clone(),
        },
        &registry,
        &scheduler,
        &gate,
        &logger,
        &emitter,
    )
    .unwrap();
    emitted.lock().unwrap().clear();

    let duplicate_report =
        token_fire::app::runtime::handle_runtime_event_with_logger_and_widget_events(
            RuntimeEvent::TranscriptChanged {
                source: TokenSourceKind::Traex,
                path: transcript,
            },
            &registry,
            &scheduler,
            &gate,
            &logger,
            &emitter,
        )
        .unwrap()
        .expect("duplicate report");

    assert_eq!(duplicate_report.inserted, 0);
    assert!(emitted.lock().unwrap().is_empty());
}

#[test]
fn runtime_claude_hook_inserts_observation_once() {
    let dir = tempdir().unwrap();
    let transcript = dir.path().join("claude-transcript.jsonl");
    fs::write(
        &transcript,
        include_str!("fixtures/claude-transcript.jsonl"),
    )
    .unwrap();
    let store = UsageStore::open(&dir.path().join("token-fire.sqlite")).unwrap();
    store
        .open_tracking_window(Utc.with_ymd_and_hms(2026, 7, 5, 9, 0, 0).unwrap())
        .unwrap();
    let scheduler = IngestScheduler::new(store);
    let gate = TrackingGate::new();
    let event = RuntimeEvent::Hook {
        source: TokenSourceKind::Claude,
        metadata: HookMetadata {
            source: Some("claude".to_string()),
            hook_event_name: Some("Stop".to_string()),
            transcript_path: Some(transcript.to_string_lossy().to_string()),
            session_id: Some("claude-session-1".to_string()),
            turn_id: Some("claude-turn-1".to_string()),
            timestamp: Some("2026-07-05T10:00:03Z".to_string()),
            ..HookMetadata::default()
        },
    };

    let first = handle_runtime_event(
        event.clone(),
        &SourceRegistry::new(vec![]),
        &scheduler,
        &gate,
    )
    .unwrap()
    .expect("claude report");
    let second = handle_runtime_event(event, &SourceRegistry::new(vec![]), &scheduler, &gate)
        .unwrap()
        .expect("claude duplicate report");

    assert_eq!(first.inserted, 1);
    assert_eq!(second.inserted, 0);
    assert_eq!(second.duplicates, 1);
}

#[test]
fn runtime_claude_unreadable_transcript_is_skipped() {
    let dir = tempdir().unwrap();
    let store = UsageStore::open(&dir.path().join("token-fire.sqlite")).unwrap();
    store
        .open_tracking_window(Utc.with_ymd_and_hms(2026, 7, 5, 9, 0, 0).unwrap())
        .unwrap();
    let scheduler = IngestScheduler::new(store);
    let gate = TrackingGate::new();

    let result = handle_runtime_event(
        RuntimeEvent::Hook {
            source: TokenSourceKind::Claude,
            metadata: HookMetadata {
                source: Some("claude".to_string()),
                hook_event_name: Some("Stop".to_string()),
                transcript_path: Some(
                    dir.path()
                        .join("missing.jsonl")
                        .to_string_lossy()
                        .to_string(),
                ),
                ..HookMetadata::default()
            },
        },
        &SourceRegistry::new(vec![]),
        &scheduler,
        &gate,
    )
    .unwrap();

    assert!(result.is_none());
}

#[test]
fn runtime_claude_hook_without_transcript_path_is_skipped() {
    let dir = tempdir().unwrap();
    let store = UsageStore::open(&dir.path().join("token-fire.sqlite")).unwrap();
    store
        .open_tracking_window(Utc.with_ymd_and_hms(2026, 7, 5, 9, 0, 0).unwrap())
        .unwrap();
    let scheduler = IngestScheduler::new(store);
    let gate = TrackingGate::new();

    let result = handle_runtime_event(
        RuntimeEvent::Hook {
            source: TokenSourceKind::Claude,
            metadata: HookMetadata {
                source: Some("claude".to_string()),
                hook_event_name: Some("Stop".to_string()),
                ..HookMetadata::default()
            },
        },
        &SourceRegistry::new(vec![]),
        &scheduler,
        &gate,
    )
    .unwrap();

    assert!(result.is_none());
}

#[test]
fn runtime_cursor_hook_without_conversation_id_is_skipped() {
    let dir = tempdir().unwrap();
    let store = UsageStore::open(&dir.path().join("token-fire.sqlite")).unwrap();
    store
        .open_tracking_window(Utc.with_ymd_and_hms(2026, 7, 5, 9, 0, 0).unwrap())
        .unwrap();
    let scheduler = IngestScheduler::new(store);
    let gate = TrackingGate::new();

    let result = handle_runtime_event(
        RuntimeEvent::Hook {
            source: TokenSourceKind::Cursor,
            metadata: HookMetadata {
                source: Some("cursor".to_string()),
                hook_event_name: Some("stop".to_string()),
                ..HookMetadata::default()
            },
        },
        &SourceRegistry::new(vec![]),
        &scheduler,
        &gate,
    )
    .unwrap();

    assert!(result.is_none());
}

#[test]
fn runtime_cursor_hook_inserts_observation_and_commits_watermark() {
    let dir = tempdir().unwrap();
    let token_fire_home = dir.path().join("token-fire");
    let cursor_home = dir.path().join("cursor-home");
    let conversation_id = "cursor-runtime-conv";
    let transcript_dir = cursor_home
        .join(".cursor/projects/project-a/agent-transcripts")
        .join(conversation_id);
    fs::create_dir_all(&transcript_dir).unwrap();
    fs::write(
        transcript_dir.join(format!("{conversation_id}.jsonl")),
        include_str!("fixtures/cursor-transcript.jsonl"),
    )
    .unwrap();
    let runtime_paths = paths(&token_fire_home);
    let store = UsageStore::open(&runtime_paths.database).unwrap();
    store
        .open_tracking_window(Utc.with_ymd_and_hms(2026, 7, 5, 9, 0, 0).unwrap())
        .unwrap();
    let scheduler = IngestScheduler::new(store);
    let gate = TrackingGate::new();

    let report = token_fire::app::runtime::handle_runtime_event_with_cursor_home(
        RuntimeEvent::Hook {
            source: TokenSourceKind::Cursor,
            metadata: HookMetadata {
                source: Some("cursor".to_string()),
                hook_event_name: Some("stop".to_string()),
                conversation_id: Some(conversation_id.to_string()),
                timestamp: Some("2026-07-05T11:00:00Z".to_string()),
                ..HookMetadata::default()
            },
        },
        &SourceRegistry::new(vec![]),
        &scheduler,
        &gate,
        Some(&cursor_home),
        Some(&runtime_paths.home.join("watermarks").join("cursor")),
    )
    .unwrap()
    .expect("cursor report");

    assert_eq!(report.inserted, 1);
    assert_eq!(
        fs::read_dir(runtime_paths.home.join("watermarks/cursor"))
            .unwrap()
            .count(),
        1
    );
    let connection = rusqlite::Connection::open(&runtime_paths.database).unwrap();
    let inserted_source: String = connection
        .query_row("select source from token_observations", [], |row| {
            row.get(0)
        })
        .unwrap();
    let inserted_source_path: String = connection
        .query_row("select source_path from token_observations", [], |row| {
            row.get(0)
        })
        .unwrap();

    assert_eq!(inserted_source, TokenSourceKind::Cursor.as_str());
    assert!(inserted_source_path.starts_with("cursor:"));
    assert!(!inserted_source_path.contains(conversation_id));
}

#[test]
fn runtime_cursor_hook_uses_transcript_path_without_cursor_home() {
    let dir = tempdir().unwrap();
    let token_fire_home = dir.path().join("token-fire");
    let transcript_path = dir.path().join("cursor-runtime-path.jsonl");
    fs::write(
        &transcript_path,
        include_str!("fixtures/cursor-transcript.jsonl"),
    )
    .unwrap();
    let runtime_paths = paths(&token_fire_home);
    let store = UsageStore::open(&runtime_paths.database).unwrap();
    store
        .open_tracking_window(Utc.with_ymd_and_hms(2026, 7, 5, 9, 0, 0).unwrap())
        .unwrap();
    let scheduler = IngestScheduler::new(store);
    let gate = TrackingGate::new();

    let report = token_fire::app::runtime::handle_runtime_event_with_cursor_home(
        RuntimeEvent::Hook {
            source: TokenSourceKind::Cursor,
            metadata: HookMetadata {
                source: Some("cursor".to_string()),
                hook_event_name: Some("stop".to_string()),
                session_id: Some("runtime-session-path".to_string()),
                transcript_path: Some(transcript_path.to_string_lossy().to_string()),
                conversation_id: Some("runtime-stale-conv".to_string()),
                timestamp: Some("2026-07-05T11:00:00Z".to_string()),
                ..HookMetadata::default()
            },
        },
        &SourceRegistry::new(vec![]),
        &scheduler,
        &gate,
        None,
        Some(&runtime_paths.home.join("watermarks").join("cursor")),
    )
    .unwrap()
    .expect("cursor report");

    assert_eq!(report.inserted, 1);
    let connection = rusqlite::Connection::open(&runtime_paths.database).unwrap();
    let source_path: String = connection
        .query_row("select source_path from token_observations", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert!(source_path.starts_with("cursor:"));
    assert!(!source_path.contains(dir.path().to_string_lossy().as_ref()));
}

#[test]
fn runtime_cursor_hook_sqlite_failure_returns_err_for_health_path() {
    let dir = tempdir().unwrap();
    let token_fire_home = dir.path().join("token-fire");
    let transcript_path = dir.path().join("cursor-runtime-sqlite-failure.jsonl");
    fs::write(
        &transcript_path,
        include_str!("fixtures/cursor-transcript.jsonl"),
    )
    .unwrap();
    let runtime_paths = paths(&token_fire_home);
    let store = UsageStore::open(&runtime_paths.database).unwrap();
    store
        .open_tracking_window(Utc.with_ymd_and_hms(2026, 7, 5, 9, 0, 0).unwrap())
        .unwrap();
    let scheduler = IngestScheduler::new(store);
    let logger = RuntimeLogger::new(runtime_paths.clone(), DebugLogGate::default());
    let gate = TrackingGate::new();
    rusqlite::Connection::open(&runtime_paths.database)
        .unwrap()
        .execute_batch("drop table tracking_windows;")
        .unwrap();
    let event = RuntimeEvent::Hook {
        source: TokenSourceKind::Cursor,
        metadata: HookMetadata {
            source: Some("cursor".to_string()),
            hook_event_name: Some("stop".to_string()),
            session_id: Some("runtime-session-sqlite-failure".to_string()),
            transcript_path: Some(transcript_path.to_string_lossy().to_string()),
            timestamp: Some("2026-07-05T11:00:00Z".to_string()),
            ..HookMetadata::default()
        },
    };

    let error = handle_runtime_event_with_logger(
        event.clone(),
        &SourceRegistry::new(vec![]),
        &scheduler,
        &gate,
        &logger,
    )
    .unwrap_err();
    token_fire::app::runtime::log_runtime_event_failure(&logger, &event, &error);

    let app_log = fs::read_to_string(&runtime_paths.app_log).unwrap();
    assert!(app_log.contains("source_collect_failed"));
    assert!(app_log.contains("sqlite_write_failed"));
    let failed_event = find_app_log_event(&app_log, "runtime_event_failed");
    assert_eq!(failed_event["runtime_event"], "hook");
    assert_eq!(failed_event["error_kind"], "sqlite_write_failed");
}

#[test]
fn runtime_cursor_hook_malformed_transcript_logs_parse_failure_kind() {
    let dir = tempdir().unwrap();
    let token_fire_home = dir.path().join("token-fire");
    let transcript_path = dir.path().join("cursor-runtime-malformed.jsonl");
    fs::write(&transcript_path, "not-json\n").unwrap();
    let runtime_paths = paths(&token_fire_home);
    let store = UsageStore::open(&runtime_paths.database).unwrap();
    store
        .open_tracking_window(Utc.with_ymd_and_hms(2026, 7, 5, 9, 0, 0).unwrap())
        .unwrap();
    let scheduler = IngestScheduler::new(store);
    let logger = RuntimeLogger::new(runtime_paths.clone(), DebugLogGate::default());
    let gate = TrackingGate::new();
    let event = RuntimeEvent::Hook {
        source: TokenSourceKind::Cursor,
        metadata: HookMetadata {
            source: Some("cursor".to_string()),
            hook_event_name: Some("stop".to_string()),
            session_id: Some("runtime-session-parse-failure".to_string()),
            transcript_path: Some(transcript_path.to_string_lossy().to_string()),
            timestamp: Some("2026-07-05T11:00:00Z".to_string()),
            ..HookMetadata::default()
        },
    };

    let error = handle_runtime_event_with_logger(
        event.clone(),
        &SourceRegistry::new(vec![]),
        &scheduler,
        &gate,
        &logger,
    )
    .unwrap_err();
    token_fire::app::runtime::log_runtime_event_failure(&logger, &event, &error);

    let app_log = fs::read_to_string(&runtime_paths.app_log).unwrap();
    let source_event = find_app_log_event(&app_log, "source_collect_failed");
    assert_eq!(source_event["error_kind"], "transcript_parse_failed");
    let runtime_event = find_app_log_event(&app_log, "runtime_event_failed");
    assert_eq!(runtime_event["runtime_event"], "hook");
    assert_eq!(runtime_event["error_kind"], "transcript_parse_failed");
}

#[test]
fn app_runtime_accepts_traex_and_codex_sources_without_hook_config_writes() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path().join("token-fire").as_path());
    let traex_sessions = dir.path().join("traex/sessions");
    let codex_sessions = dir.path().join("codex/sessions");
    fs::create_dir_all(&traex_sessions).unwrap();
    fs::create_dir_all(&codex_sessions).unwrap();
    let codex_hooks = dir.path().join("codex/hooks.json");
    fs::create_dir_all(codex_hooks.parent().unwrap()).unwrap();
    fs::write(&codex_hooks, r#"{"hooks":{"Stop":[]}}"#).unwrap();
    let before = fs::read_to_string(&codex_hooks).unwrap();
    let registry = SourceRegistry::new(vec![
        SourcePaths::new(
            TokenSourceKind::Traex,
            traex_sessions,
            dir.path().join("traex/archived_sessions"),
        ),
        SourcePaths::new(
            TokenSourceKind::Codex,
            codex_sessions,
            dir.path().join("codex/archived_sessions"),
        ),
    ]);

    let runtime = AppRuntime::start_with_sources(
        paths,
        registry,
        TrackingGate::new(),
        DebugLogGate::default(),
    )
    .unwrap();

    drop(runtime);
    assert_eq!(fs::read_to_string(&codex_hooks).unwrap(), before);
}

#[test]
fn codex_baseline_skips_bad_files_without_blocking_runtime_start() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path().join("token-fire").as_path());
    let traex_sessions = dir.path().join("traex/sessions");
    let codex_sessions = dir.path().join("codex/sessions");
    fs::create_dir_all(&traex_sessions).unwrap();
    fs::create_dir_all(&codex_sessions).unwrap();
    fs::write(
        codex_sessions.join("bad-non-utf8.jsonl"),
        [0xff, 0xfe, 0xfd],
    )
    .unwrap();
    let good_codex = codex_sessions.join("good.jsonl");
    fs::write(&good_codex, include_str!("fixtures/codex-session.jsonl")).unwrap();
    let registry = SourceRegistry::new(vec![
        SourcePaths::new(
            TokenSourceKind::Traex,
            traex_sessions,
            dir.path().join("traex/archived_sessions"),
        ),
        SourcePaths::new(
            TokenSourceKind::Codex,
            codex_sessions,
            dir.path().join("codex/archived_sessions"),
        ),
    ]);

    let runtime = AppRuntime::start_with_sources(
        paths.clone(),
        registry,
        TrackingGate::new(),
        DebugLogGate::default(),
    )
    .unwrap();

    let store = UsageStore::open(&paths.database).unwrap();
    assert!(store
        .file_baseline(&good_codex.to_string_lossy())
        .unwrap()
        .is_some());
    assert!(paths.socket.exists());
    drop(runtime);
}

#[test]
fn codex_startup_baseline_stops_before_trailing_partial_line() {
    let dir = tempdir().unwrap();
    let store = UsageStore::open(&dir.path().join("token-fire.sqlite")).unwrap();
    let sessions_dir = dir.path().join("codex/sessions");
    fs::create_dir_all(&sessions_dir).unwrap();
    let transcript = sessions_dir.join("rollout-partial.jsonl");
    let complete_line = "{\"type\":\"event_msg\",\"timestamp\":\"2026-06-26T03:01:02.000Z\",\"payload\":{\"type\":\"token_count\",\"info\":{\"last_token_usage\":{\"input_tokens\":1,\"output_tokens\":2,\"cached_input_tokens\":0,\"cache_creation_input_tokens\":0,\"reasoning_output_tokens\":0,\"total_tokens\":3},\"total_token_usage\":{\"input_tokens\":1,\"output_tokens\":2,\"cached_input_tokens\":0,\"cache_creation_input_tokens\":0,\"reasoning_output_tokens\":0,\"total_tokens\":3}}}}\n";
    let partial_line = "{\"type\":\"event_msg\",\"timestamp\":\"2026-06-26T03:01:03.000Z\",\"payload\":{\"type\":\"token_count\",\"info\":{\"last_token_usage\":{\"input_tokens\":4,\"output_tokens\":5";
    fs::write(&transcript, format!("{complete_line}{partial_line}")).unwrap();

    baseline_existing_source_files(
        &store,
        &[SourcePaths::new(
            TokenSourceKind::Codex,
            sessions_dir,
            dir.path().join("codex/archived_sessions"),
        )],
    )
    .unwrap();

    assert_eq!(
        store.file_baseline(&transcript.to_string_lossy()).unwrap(),
        Some(0)
    );
}

#[test]
fn hook_intake_delegates_to_runtime_stop_and_pause_gates() {
    let dir = tempdir().unwrap();
    let transcript = dir
        .path()
        .join("sessions/2026/06/20/rollout-019-session-a.jsonl");
    fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    fs::write(&transcript, include_str!("fixtures/traex-session.jsonl")).unwrap();

    let store = UsageStore::open(&dir.path().join("token-fire.sqlite")).unwrap();
    store
        .open_tracking_window(Utc.with_ymd_and_hms(2026, 6, 20, 3, 0, 0).unwrap())
        .unwrap();
    let scheduler = IngestScheduler::new(store);
    let traex_paths = TraexPaths {
        sessions_dir: dir.path().join("sessions"),
        archived_sessions_dir: dir.path().join("archived_sessions"),
    };
    let gate = TrackingGate::new();
    gate.pause();
    let metadata = HookMetadata {
        hook_event_name: Some("Stop".to_string()),
        transcript_path: Some(transcript.to_string_lossy().to_string()),
        ..HookMetadata::default()
    };

    let handled = handle_traex_hook_metadata(metadata, &traex_paths, &scheduler, &gate).unwrap();

    assert!(!handled);
}

#[test]
fn app_runtime_start_failure_does_not_leave_active_tracking_window() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path());
    fs::write(&paths.run_dir, b"not a directory").unwrap();
    let traex_paths = TraexPaths {
        sessions_dir: dir.path().join("sessions"),
        archived_sessions_dir: dir.path().join("archived_sessions"),
    };

    let result = AppRuntime::start(
        paths.clone(),
        traex_paths,
        TrackingGate::new(),
        DebugLogGate::default(),
    );

    assert!(result.is_err());
    let store = UsageStore::open(&paths.database).unwrap();
    assert_eq!(store.active_tracking_windows().unwrap(), []);
}

#[test]
fn main_facing_runtime_start_degrades_socket_status_instead_of_panicking() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path());
    fs::write(&paths.run_dir, b"not a directory").unwrap();
    let traex_paths = TraexPaths {
        sessions_dir: dir.path().join("sessions"),
        archived_sessions_dir: dir.path().join("archived_sessions"),
    };
    let app_state = AppState::new(paths.clone());

    let runtime = start_app_runtime_for_state(
        &app_state,
        paths.clone(),
        traex_paths,
        TrackingGate::new(),
        DebugLogGate::default(),
    );

    assert!(runtime.is_none());
    assert!(!app_state.socket_ok());
    let log = fs::read_to_string(&paths.app_log).unwrap();
    assert!(log.contains("runtime_start_failed"));
}

#[test]
fn app_runtime_start_closes_stale_active_windows_and_opens_current_process_window() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path());
    let store = UsageStore::open(&paths.database).unwrap();
    store
        .open_tracking_window(Utc.with_ymd_and_hms(2026, 6, 20, 3, 0, 0).unwrap())
        .unwrap();
    store
        .open_tracking_window(Utc.with_ymd_and_hms(2026, 6, 20, 4, 0, 0).unwrap())
        .unwrap();
    let traex_paths = TraexPaths {
        sessions_dir: dir.path().join("sessions"),
        archived_sessions_dir: dir.path().join("archived_sessions"),
    };

    let before_start = Utc::now();
    let _runtime = AppRuntime::start(
        paths.clone(),
        traex_paths,
        TrackingGate::new(),
        DebugLogGate::default(),
    )
    .unwrap();
    let after_start = Utc::now();

    let store = UsageStore::open(&paths.database).unwrap();
    let active_windows = store.active_tracking_windows().unwrap();
    assert_eq!(active_windows.len(), 1);
    assert!(active_windows[0].started_at >= before_start);
    assert!(active_windows[0].started_at <= after_start);
}

#[test]
fn runtime_tracking_window_uses_timestamp_captured_before_component_initialization() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path());
    let store = UsageStore::open(&paths.database).unwrap();
    store
        .open_tracking_window(Utc.with_ymd_and_hms(2026, 6, 20, 3, 0, 0).unwrap())
        .unwrap();
    let intended_started_at = Utc.with_ymd_and_hms(2026, 6, 20, 4, 0, 0).unwrap();
    let initialization_observed_at = Utc.with_ymd_and_hms(2026, 6, 20, 4, 0, 1).unwrap();
    let captured_before_initialization = AtomicBool::new(false);

    activate_runtime_tracking_window(
        &paths.database,
        || intended_started_at,
        |started_at| {
            captured_before_initialization.store(true, Ordering::SeqCst);
            assert_eq!(started_at, intended_started_at);
            assert!(initialization_observed_at > started_at);
            let store = UsageStore::open(&paths.database).unwrap();
            assert_eq!(store.active_tracking_windows().unwrap().len(), 1);
            Ok(())
        },
    )
    .unwrap();

    let store = UsageStore::open(&paths.database).unwrap();
    let active_windows = store.active_tracking_windows().unwrap();
    assert!(captured_before_initialization.load(Ordering::SeqCst));
    assert_eq!(active_windows.len(), 1);
    assert_eq!(active_windows[0].started_at, intended_started_at);
}

#[test]
fn token_fire_hook_path_is_resolved_from_current_executable_directory() {
    let app_exe = PathBuf::from("/Applications/TokenFire.app/Contents/MacOS/token-fire");
    assert_eq!(
        token_fire_hook_path_from_exe(&app_exe),
        PathBuf::from("/Applications/TokenFire.app/Contents/MacOS/token-fire-hook")
    );
}

#[test]
fn watcher_uses_nearest_existing_parent_when_traex_roots_are_missing() {
    let dir = tempdir().unwrap();
    let cli_dir = dir.path().join(".trae").join("cli");
    fs::create_dir_all(&cli_dir).unwrap();
    let traex_paths = TraexPaths {
        sessions_dir: cli_dir.join("sessions"),
        archived_sessions_dir: cli_dir.join("archived_sessions"),
    };

    let roots = watch_roots(&traex_paths);

    assert_eq!(roots, vec![cli_dir]);
}

#[test]
fn watcher_does_not_escape_missing_traex_cli_boundary() {
    let dir = tempdir().unwrap();
    let cli_dir = dir.path().join(".trae").join("cli");
    let traex_paths = TraexPaths {
        sessions_dir: cli_dir.join("sessions"),
        archived_sessions_dir: cli_dir.join("archived_sessions"),
    };

    let roots = watch_roots(&traex_paths);

    assert!(roots.is_empty());
}

#[test]
fn runtime_worker_logger_records_ingestion_errors() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path());
    let store = UsageStore::open(&paths.database).unwrap();
    store
        .open_tracking_window(Utc.with_ymd_and_hms(2026, 6, 20, 3, 0, 0).unwrap())
        .unwrap();
    let scheduler = IngestScheduler::new(store);
    let logger = RuntimeLogger::new(paths.clone(), DebugLogGate::default());
    let gate = TrackingGate::new();

    let error = handle_runtime_event_with_logger(
        RuntimeEvent::TranscriptChanged {
            source: TokenSourceKind::Traex,
            path: dir.path().join("missing.jsonl"),
        },
        &SourceRegistry::new(vec![SourcePaths::from(&TraexPaths {
            sessions_dir: dir.path().join("sessions"),
            archived_sessions_dir: dir.path().join("archived_sessions"),
        })]),
        &scheduler,
        &gate,
        &logger,
    )
    .unwrap_err();
    token_fire::app::runtime::log_runtime_event_failure(
        &logger,
        &RuntimeEvent::TranscriptChanged {
            source: TokenSourceKind::Traex,
            path: dir.path().join("missing.jsonl"),
        },
        &error,
    );

    let app_log = fs::read_to_string(&paths.app_log).unwrap();
    let source_event = find_app_log_event(&app_log, "source_collect_failed");
    assert_eq!(source_event["error_kind"], "transcript_unreadable");
    let runtime_event = find_app_log_event(&app_log, "runtime_event_failed");
    assert_eq!(runtime_event["runtime_event"], "transcript_changed");
    assert_eq!(runtime_event["error_kind"], "transcript_unreadable");
}

#[test]
fn app_runtime_start_runs_retention_before_returning() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path());
    fs::create_dir_all(&paths.run_dir).unwrap();
    let sessions_dir = dir.path().join("sessions");
    let archived_sessions_dir = dir.path().join("archived_sessions");
    fs::create_dir_all(&sessions_dir).unwrap();
    fs::create_dir_all(&archived_sessions_dir).unwrap();
    let old = Utc::now()
        - chrono::Duration::days(RetentionPolicy::default().observation_retention_days + 1);
    let store = UsageStore::open(&paths.database).unwrap();
    let window_id = store
        .open_tracking_window(old - chrono::Duration::minutes(1))
        .unwrap();
    store
        .insert_observation_for_tracking_window(
            &runtime_observation("retention-startup-old", 10, old),
            window_id,
        )
        .unwrap();
    drop(store);

    let runtime = AppRuntime::start(
        paths.clone(),
        TraexPaths {
            sessions_dir,
            archived_sessions_dir,
        },
        TrackingGate::new(),
        DebugLogGate::default(),
    )
    .unwrap();

    assert_eq!(
        rusqlite::Connection::open(&paths.database)
            .unwrap()
            .query_row("select count(*) from token_observations", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    drop(runtime);
}

#[test]
fn startup_retention_failure_logs_and_does_not_block_runtime() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path());
    fs::create_dir_all(&paths.run_dir).unwrap();
    let sessions_dir = dir.path().join("sessions");
    let archived_sessions_dir = dir.path().join("archived_sessions");
    fs::create_dir_all(&sessions_dir).unwrap();
    fs::create_dir_all(&archived_sessions_dir).unwrap();
    let old = Utc::now()
        - chrono::Duration::days(RetentionPolicy::default().observation_retention_days + 1);
    let store = UsageStore::open(&paths.database).unwrap();
    let window_id = store
        .open_tracking_window(old - chrono::Duration::minutes(1))
        .unwrap();
    store
        .insert_observation_for_tracking_window(
            &runtime_observation("retention-startup-fail", 10, old),
            window_id,
        )
        .unwrap();
    drop(store);
    rusqlite::Connection::open(&paths.database)
        .unwrap()
        .execute_batch(
            r#"
            create trigger fail_retention_delete
            before delete on token_observations
            begin
              select raise(abort, 'retention delete blocked');
            end;
            "#,
        )
        .unwrap();

    let runtime = AppRuntime::start(
        paths.clone(),
        TraexPaths {
            sessions_dir,
            archived_sessions_dir,
        },
        TrackingGate::new(),
        DebugLogGate::default(),
    )
    .unwrap();

    let db_log = fs::read_to_string(&paths.db_log).unwrap();
    assert!(db_log.contains("retention_failed"));
    assert!(db_log.contains("sqlite_retention_failed"));
    assert_eq!(
        rusqlite::Connection::open(&paths.database)
            .unwrap()
            .query_row(
                "select value from retention_state where key = 'last_success_at'",
                [],
                |row| row.get::<_, String>(0),
            )
            .ok(),
        None
    );
    drop(runtime);
}

#[test]
fn token_collection_events_are_source_aware() {
    let dir = tempdir().unwrap();
    let runtime_paths = paths(dir.path());
    let store = UsageStore::open(&runtime_paths.database).unwrap();
    store
        .open_tracking_window(Utc.with_ymd_and_hms(2026, 7, 5, 9, 0, 0).unwrap())
        .unwrap();
    let scheduler = IngestScheduler::new(store);
    let registry = SourceRegistry::new(vec![]);
    let router = SourceIngestRouter {
        registry: &registry,
        scheduler: &scheduler,
        logger: None,
        paths: SourceIngestPaths::default(),
    };

    for (source, event) in [
        (TokenSourceKind::Traex, Some("Stop")),
        (TokenSourceKind::Codex, Some("Stop")),
        (TokenSourceKind::Claude, Some("Stop")),
        (TokenSourceKind::Claude, Some("StopFailure")),
        (TokenSourceKind::Claude, Some("SubagentStop")),
        (TokenSourceKind::Cursor, Some("stop")),
    ] {
        let outcome = router
            .ingest_hook(
                source,
                HookMetadata {
                    hook_event_name: event.map(str::to_string),
                    ..HookMetadata::default()
                },
            )
            .unwrap();
        assert_ne!(
            outcome.empty_reason,
            Some(SourceEmptyReason::UnsupportedHookEvent)
        );
    }

    for (source, event) in [
        (TokenSourceKind::Traex, Some("StopFailure")),
        (TokenSourceKind::Codex, Some("StopFailure")),
        (TokenSourceKind::Claude, Some("stop")),
        (TokenSourceKind::Claude, Some("PermissionRequest")),
        (TokenSourceKind::Cursor, Some("Stop")),
    ] {
        let outcome = router
            .ingest_hook(
                source,
                HookMetadata {
                    hook_event_name: event.map(str::to_string),
                    ..HookMetadata::default()
                },
            )
            .unwrap();
        assert_eq!(
            outcome.empty_reason,
            Some(SourceEmptyReason::UnsupportedHookEvent)
        );
    }
}

#[test]
fn hook_source_parser_never_defaults_unknown_source_to_traex() {
    assert_eq!(
        token_fire::app::runtime::parse_hook_source(Some("traex")),
        Some(TokenSourceKind::Traex)
    );
    assert_eq!(
        token_fire::app::runtime::parse_hook_source(Some("codex")),
        Some(TokenSourceKind::Codex)
    );
    assert_eq!(
        token_fire::app::runtime::parse_hook_source(Some("claude")),
        Some(TokenSourceKind::Claude)
    );
    assert_eq!(
        token_fire::app::runtime::parse_hook_source(Some("cursor")),
        Some(TokenSourceKind::Cursor)
    );
    assert_eq!(
        token_fire::app::runtime::parse_hook_source(Some("unknown")),
        None
    );
    assert_eq!(
        token_fire::app::runtime::parse_hook_source(Some("Cursor")),
        None
    );
    assert_eq!(token_fire::app::runtime::parse_hook_source(None), None);
}

fn find_db_log_event(db_log: &str, event: &str) -> Value {
    db_log
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|value| value["event"] == event)
        .unwrap_or_else(|| panic!("expected {event} db log"))
}

fn rollup_metadata_value(db_path: &Path, key: &str) -> Option<String> {
    rusqlite::Connection::open(db_path)
        .unwrap()
        .query_row(
            "select value from usage_rollup_metadata where key = ?1",
            [key],
            |row| row.get(0),
        )
        .ok()
}

/// 造一个已增量写入 rollup 但从未翻转 state/version 的库（模拟正常 ingest 后首次启动）。
fn seed_tracked_rollup_db(db_path: &Path) {
    let store = UsageStore::open(db_path).unwrap();
    let observed_at = Utc::now() - chrono::Duration::minutes(5);
    let window_id = store
        .open_tracking_window(observed_at - chrono::Duration::minutes(1))
        .unwrap();
    store
        .insert_observation_for_tracking_window(
            &runtime_observation("startup-rollup-seed", 1234, observed_at),
            window_id,
        )
        .unwrap();
}

#[test]
fn startup_profile_rollup_rebuilds_and_logs_status_when_state_missing() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path());
    fs::create_dir_all(&paths.run_dir).unwrap();
    let sessions_dir = dir.path().join("sessions");
    let archived_sessions_dir = dir.path().join("archived_sessions");
    fs::create_dir_all(&sessions_dir).unwrap();
    fs::create_dir_all(&archived_sessions_dir).unwrap();
    seed_tracked_rollup_db(&paths.database);

    let runtime = AppRuntime::start(
        paths.clone(),
        TraexPaths {
            sessions_dir,
            archived_sessions_dir,
        },
        TrackingGate::new(),
        DebugLogGate::default(),
    )
    .unwrap();

    // 启动维护把 rollup 翻转 ready + 写 schema_version。
    assert_eq!(
        rollup_metadata_value(&paths.database, "state"),
        Some("ready".to_string())
    );
    assert_eq!(
        rollup_metadata_value(&paths.database, "schema_version"),
        Some("1".to_string())
    );

    let db_log = fs::read_to_string(&paths.db_log).unwrap();
    let event = find_db_log_event(&db_log, "rollup_rebuild_status");
    assert_eq!(event["rollup_rebuild_status"], "rebuilt");
    assert_eq!(event["rollup_schema_version"], "1");
    assert_eq!(event["rollup_row_count"], 1);
    assert!(event["rollup_rebuild_duration_ms"].is_number());

    // 诊断日志只含 duration/status/version/row-count，绝不含 model/source path/session id。
    let object = event.as_object().unwrap();
    assert!(!object.contains_key("model"));
    assert!(!object.contains_key("source"));
    assert!(!object.contains_key("source_path"));
    assert!(!object.contains_key("session_id"));
    drop(runtime);
}

#[test]
fn startup_profile_rollup_ready_does_not_rebuild_again() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path());
    fs::create_dir_all(&paths.run_dir).unwrap();
    let sessions_dir = dir.path().join("sessions");
    let archived_sessions_dir = dir.path().join("archived_sessions");
    fs::create_dir_all(&sessions_dir).unwrap();
    fs::create_dir_all(&archived_sessions_dir).unwrap();
    seed_tracked_rollup_db(&paths.database);
    let traex_paths = TraexPaths {
        sessions_dir,
        archived_sessions_dir,
    };

    // 首次启动：state 缺失 → rebuild。
    let runtime = AppRuntime::start(
        paths.clone(),
        traex_paths.clone(),
        TrackingGate::new(),
        DebugLogGate::default(),
    )
    .unwrap();
    drop(runtime);
    fs::remove_file(&paths.db_log).ok();

    // 第二次启动：已 Ready → 只校验不重建。
    let runtime = AppRuntime::start(
        paths.clone(),
        traex_paths,
        TrackingGate::new(),
        DebugLogGate::default(),
    )
    .unwrap();

    let db_log = fs::read_to_string(&paths.db_log).unwrap();
    let event = find_db_log_event(&db_log, "rollup_rebuild_status");
    assert_eq!(event["rollup_rebuild_status"], "ready");
    assert_eq!(event["rollup_schema_version"], "1");
    drop(runtime);
}

#[test]
fn startup_profile_rollup_failure_logs_and_does_not_block_runtime() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path());
    fs::create_dir_all(&paths.run_dir).unwrap();
    let sessions_dir = dir.path().join("sessions");
    let archived_sessions_dir = dir.path().join("archived_sessions");
    fs::create_dir_all(&sessions_dir).unwrap();
    fs::create_dir_all(&archived_sessions_dir).unwrap();
    seed_tracked_rollup_db(&paths.database);

    // 用同名 index 占用 shadow 表名：rebuild 的 create table usage_rollups_15m_rebuild 必失败。
    rusqlite::Connection::open(&paths.database)
        .unwrap()
        .execute(
            "create index usage_rollups_15m_rebuild on usage_rollups_15m(source)",
            [],
        )
        .unwrap();

    // rebuild 失败不返回启动错误：runtime 仍返回 Some(AppRuntime)。
    let runtime = AppRuntime::start(
        paths.clone(),
        TraexPaths {
            sessions_dir,
            archived_sessions_dir,
        },
        TrackingGate::new(),
        DebugLogGate::default(),
    )
    .unwrap();

    let db_log = fs::read_to_string(&paths.db_log).unwrap();
    let event = find_db_log_event(&db_log, "rollup_rebuild_status");
    assert_eq!(event["rollup_rebuild_status"], "failed");
    assert!(event["error_kind"].is_string());
    // 失败后 rollup 未 ready，Profile 后续可走 raw fallback（Task 6）。
    assert_ne!(
        rollup_metadata_value(&paths.database, "state"),
        Some("ready".to_string())
    );
    drop(runtime);
}
