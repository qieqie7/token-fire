use std::fs;
use std::path::Path;

use chrono::{TimeZone, Utc};
use serde_json::Value;
use tempfile::tempdir;
use token_fire::adapters::cursor::{
    collect_pending_from_transcript_path, CursorCollectResult, CursorTranscriptIdentity,
};
use token_fire::adapters::source::{SourceRegistry, TokenSourceKind};
use token_fire::adapters::HookMetadata;
use token_fire::app::ingest_scheduler::IngestScheduler;
use token_fire::app::logging::{DebugLogGate, RuntimeLogger};
use token_fire::app::paths::RuntimePaths;
use token_fire::app::source_ingest::{
    SourceEmptyReason, SourceIngestEvent, SourceIngestPaths, SourceIngestRouter, SourceResolution,
};
use token_fire::core::usage_store::UsageStore;

fn runtime_paths(home: &Path) -> RuntimePaths {
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

fn assert_source_collect_empty_schema(
    event: &Value,
    hook_event_name: &str,
    session_id_present: bool,
    conversation_id_present: bool,
    transcript_path_present: bool,
    resolved_by: &str,
    empty_reason: &str,
    inserted: Option<i64>,
    duplicates: Option<i64>,
    skipped_outside_tracking: Option<i64>,
) {
    assert_eq!(event["source"], "cursor");
    assert_eq!(event["hook_event_name"], hook_event_name);
    assert_eq!(event["session_id_present"], session_id_present);
    assert_eq!(event["conversation_id_present"], conversation_id_present);
    assert_eq!(event["transcript_path_present"], transcript_path_present);
    assert_eq!(event["resolved_by"], resolved_by);
    assert_eq!(event["empty_reason"], empty_reason);
    assert_eq!(
        event["inserted"],
        inserted.map(Value::from).unwrap_or(Value::Null)
    );
    assert_eq!(
        event["duplicates"],
        duplicates.map(Value::from).unwrap_or(Value::Null)
    );
    assert_eq!(
        event["skipped_outside_tracking"],
        skipped_outside_tracking
            .map(Value::from)
            .unwrap_or(Value::Null)
    );
}

fn assert_source_ingested_schema(
    event: &Value,
    hook_event_name: &str,
    session_id_present: bool,
    conversation_id_present: bool,
    transcript_path_present: bool,
    resolved_by: &str,
    inserted: i64,
    duplicates: i64,
    skipped_outside_tracking: i64,
) {
    assert_eq!(event["source"], "cursor");
    assert_eq!(event["hook_event_name"], hook_event_name);
    assert_eq!(event["session_id_present"], session_id_present);
    assert_eq!(event["conversation_id_present"], conversation_id_present);
    assert_eq!(event["transcript_path_present"], transcript_path_present);
    assert_eq!(event["resolved_by"], resolved_by);
    assert_eq!(event["inserted"], inserted);
    assert_eq!(event["duplicates"], duplicates);
    assert_eq!(event["skipped_outside_tracking"], skipped_outside_tracking);
}

#[test]
fn router_ingests_cursor_hook_by_transcript_path() {
    let dir = tempdir().unwrap();
    let paths = runtime_paths(dir.path());
    let transcript_path = dir.path().join("cursor-router.jsonl");
    fs::write(
        &transcript_path,
        include_str!("fixtures/cursor-transcript.jsonl"),
    )
    .unwrap();
    let store = UsageStore::open(&paths.database).unwrap();
    store
        .open_tracking_window(Utc.with_ymd_and_hms(2026, 7, 5, 9, 0, 0).unwrap())
        .unwrap();
    let scheduler = IngestScheduler::new(store);
    let registry = SourceRegistry::new(vec![]);
    let logger = RuntimeLogger::new(paths.clone(), DebugLogGate::default());
    let router = SourceIngestRouter {
        registry: &registry,
        scheduler: &scheduler,
        logger: Some(&logger),
        paths: SourceIngestPaths {
            cursor_home: None,
            cursor_watermark_dir: Some(paths.home.join("watermarks").join("cursor")),
        },
    };

    let outcome = router
        .ingest_hook(
            TokenSourceKind::Cursor,
            HookMetadata {
                source: Some("cursor".to_string()),
                hook_event_name: Some("stop".to_string()),
                session_id: Some("router-session".to_string()),
                transcript_path: Some(transcript_path.to_string_lossy().to_string()),
                conversation_id: Some("stale-conversation-id".to_string()),
                timestamp: Some("2026-07-05T11:00:00Z".to_string()),
                ..HookMetadata::default()
            },
        )
        .unwrap();

    assert_eq!(outcome.source, TokenSourceKind::Cursor);
    assert_eq!(outcome.event, SourceIngestEvent::Hook);
    assert_eq!(outcome.resolution, SourceResolution::TranscriptPath);
    assert_eq!(outcome.empty_reason, None);
    assert_eq!(outcome.report.unwrap().inserted, 1);
}

#[test]
fn router_logs_watermark_at_eof_when_cursor_path_has_no_append() {
    let dir = tempdir().unwrap();
    let paths = runtime_paths(dir.path());
    let transcript_path = dir.path().join("cursor-duplicate.jsonl");
    fs::write(
        &transcript_path,
        include_str!("fixtures/cursor-transcript.jsonl"),
    )
    .unwrap();
    let store = UsageStore::open(&paths.database).unwrap();
    store
        .open_tracking_window(Utc.with_ymd_and_hms(2026, 7, 5, 9, 0, 0).unwrap())
        .unwrap();
    let scheduler = IngestScheduler::new(store);
    let registry = SourceRegistry::new(vec![]);
    let logger = RuntimeLogger::new(paths.clone(), DebugLogGate::default());
    let router = SourceIngestRouter {
        registry: &registry,
        scheduler: &scheduler,
        logger: Some(&logger),
        paths: SourceIngestPaths {
            cursor_home: None,
            cursor_watermark_dir: Some(paths.home.join("watermarks").join("cursor")),
        },
    };
    let metadata = HookMetadata {
        source: Some("cursor".to_string()),
        hook_event_name: Some("stop".to_string()),
        session_id: Some("router-watermark-eof".to_string()),
        transcript_path: Some(transcript_path.to_string_lossy().to_string()),
        timestamp: Some("2026-07-05T11:00:00Z".to_string()),
        ..HookMetadata::default()
    };

    router
        .ingest_hook(TokenSourceKind::Cursor, metadata.clone())
        .unwrap();
    let second = router
        .ingest_hook(TokenSourceKind::Cursor, metadata)
        .unwrap();

    assert_eq!(second.resolution, SourceResolution::TranscriptPath);
    assert_eq!(second.empty_reason, Some(SourceEmptyReason::WatermarkAtEof));
    let app_log = fs::read_to_string(&paths.app_log).unwrap();
    let last: Value = serde_json::from_str(app_log.lines().last().unwrap()).unwrap();
    assert_eq!(last["event"], "source_collect_empty");
    assert_source_collect_empty_schema(
        &last,
        "stop",
        true,
        false,
        true,
        "transcript_path",
        "watermark_at_eof",
        None,
        None,
        None,
    );
}

#[test]
fn router_logs_duplicate_only_when_scheduler_inserts_zero_rows() {
    let dir = tempdir().unwrap();
    let paths = runtime_paths(dir.path());
    let transcript_path = dir.path().join("cursor-duplicate-only.jsonl");
    fs::write(
        &transcript_path,
        include_str!("fixtures/cursor-transcript.jsonl"),
    )
    .unwrap();
    let store = UsageStore::open(&paths.database).unwrap();
    store
        .open_tracking_window(Utc.with_ymd_and_hms(2026, 7, 5, 9, 0, 0).unwrap())
        .unwrap();
    let metadata = HookMetadata {
        source: Some("cursor".to_string()),
        hook_event_name: Some("stop".to_string()),
        session_id: Some("router-duplicate-only".to_string()),
        transcript_path: Some(transcript_path.to_string_lossy().to_string()),
        timestamp: Some("2026-07-05T11:00:00Z".to_string()),
        ..HookMetadata::default()
    };
    let identity = CursorTranscriptIdentity::from_path(&transcript_path, &metadata);
    let pending = match collect_pending_from_transcript_path(
        &transcript_path,
        &metadata,
        identity,
        Some(&paths.home.join("manual-watermarks").join("cursor")),
    )
    .unwrap()
    {
        CursorCollectResult::Pending(pending) => pending,
        CursorCollectResult::Empty(reason) => panic!("expected pending, got {reason:?}"),
    };
    let window_id = store.active_tracking_windows().unwrap()[0].id;
    store
        .insert_observation_for_tracking_window(pending.observation(), window_id)
        .unwrap();

    let scheduler = IngestScheduler::new(store);
    let registry = SourceRegistry::new(vec![]);
    let logger = RuntimeLogger::new(paths.clone(), DebugLogGate::default());
    let router = SourceIngestRouter {
        registry: &registry,
        scheduler: &scheduler,
        logger: Some(&logger),
        paths: SourceIngestPaths {
            cursor_home: None,
            cursor_watermark_dir: Some(paths.home.join("watermarks").join("cursor")),
        },
    };

    let outcome = router
        .ingest_hook(TokenSourceKind::Cursor, metadata)
        .unwrap();

    assert_eq!(outcome.resolution, SourceResolution::TranscriptPath);
    assert_eq!(outcome.empty_reason, Some(SourceEmptyReason::DuplicateOnly));
    assert_eq!(outcome.report.as_ref().unwrap().inserted, 0);
    assert_eq!(outcome.report.as_ref().unwrap().duplicates, 1);
    let app_log = fs::read_to_string(&paths.app_log).unwrap();
    let last: Value = serde_json::from_str(app_log.lines().last().unwrap()).unwrap();
    assert_eq!(last["event"], "source_collect_empty");
    assert_source_collect_empty_schema(
        &last,
        "stop",
        true,
        false,
        true,
        "transcript_path",
        "duplicate_only",
        Some(0),
        Some(1),
        Some(0),
    );
}

#[test]
fn router_uses_conversation_id_only_when_cursor_transcript_path_is_absent() {
    let dir = tempdir().unwrap();
    let paths = runtime_paths(dir.path());
    let cursor_home = dir.path().join("cursor-home");
    let conversation_id = "cursor-router-fallback";
    let transcript_dir = cursor_home
        .join(".cursor/projects/project-a/agent-transcripts")
        .join(conversation_id);
    fs::create_dir_all(&transcript_dir).unwrap();
    fs::write(
        transcript_dir.join(format!("{conversation_id}.jsonl")),
        include_str!("fixtures/cursor-transcript.jsonl"),
    )
    .unwrap();
    let store = UsageStore::open(&paths.database).unwrap();
    store
        .open_tracking_window(Utc.with_ymd_and_hms(2026, 7, 5, 9, 0, 0).unwrap())
        .unwrap();
    let scheduler = IngestScheduler::new(store);
    let registry = SourceRegistry::new(vec![]);
    let logger = RuntimeLogger::new(paths.clone(), DebugLogGate::default());
    let router = SourceIngestRouter {
        registry: &registry,
        scheduler: &scheduler,
        logger: Some(&logger),
        paths: SourceIngestPaths {
            cursor_home: Some(cursor_home),
            cursor_watermark_dir: Some(paths.home.join("watermarks").join("cursor")),
        },
    };

    let outcome = router
        .ingest_hook(
            TokenSourceKind::Cursor,
            HookMetadata {
                source: Some("cursor".to_string()),
                hook_event_name: Some("stop".to_string()),
                conversation_id: Some(conversation_id.to_string()),
                timestamp: Some("2026-07-05T11:00:00Z".to_string()),
                ..HookMetadata::default()
            },
        )
        .unwrap();

    assert_eq!(outcome.resolution, SourceResolution::ConversationId);
    assert_eq!(outcome.report.unwrap().inserted, 1);
}

#[test]
fn router_logs_missing_cursor_conversation_transcript_as_path_missing() {
    let dir = tempdir().unwrap();
    let paths = runtime_paths(dir.path());
    let cursor_home = dir.path().join("cursor-home");
    let store = UsageStore::open(&paths.database).unwrap();
    store
        .open_tracking_window(Utc.with_ymd_and_hms(2026, 7, 5, 9, 0, 0).unwrap())
        .unwrap();
    let scheduler = IngestScheduler::new(store);
    let registry = SourceRegistry::new(vec![]);
    let logger = RuntimeLogger::new(paths.clone(), DebugLogGate::default());
    let router = SourceIngestRouter {
        registry: &registry,
        scheduler: &scheduler,
        logger: Some(&logger),
        paths: SourceIngestPaths {
            cursor_home: Some(cursor_home),
            cursor_watermark_dir: Some(paths.home.join("watermarks").join("cursor")),
        },
    };

    let outcome = router
        .ingest_hook(
            TokenSourceKind::Cursor,
            HookMetadata {
                source: Some("cursor".to_string()),
                hook_event_name: Some("stop".to_string()),
                conversation_id: Some("cursor-router-missing-conversation".to_string()),
                timestamp: Some("2026-07-05T11:00:00Z".to_string()),
                ..HookMetadata::default()
            },
        )
        .unwrap();

    assert_eq!(outcome.resolution, SourceResolution::ConversationId);
    assert_eq!(
        outcome.empty_reason,
        Some(SourceEmptyReason::TranscriptPathMissing)
    );
    let app_log = fs::read_to_string(&paths.app_log).unwrap();
    let empty_event: Value = serde_json::from_str(app_log.lines().last().unwrap()).unwrap();
    assert_eq!(empty_event["event"], "source_collect_empty");
    assert_source_collect_empty_schema(
        &empty_event,
        "stop",
        false,
        true,
        false,
        "conversation_id",
        "transcript_path_missing",
        None,
        None,
        None,
    );
}

#[test]
fn router_falls_back_to_conversation_id_when_cursor_transcript_path_is_missing() {
    let dir = tempdir().unwrap();
    let paths = runtime_paths(dir.path());
    let cursor_home = dir.path().join("cursor-home");
    let conversation_id = "cursor-router-missing-path-fallback";
    let transcript_dir = cursor_home
        .join(".cursor/projects/project-a/agent-transcripts")
        .join(conversation_id);
    fs::create_dir_all(&transcript_dir).unwrap();
    fs::write(
        transcript_dir.join(format!("{conversation_id}.jsonl")),
        include_str!("fixtures/cursor-transcript.jsonl"),
    )
    .unwrap();
    let store = UsageStore::open(&paths.database).unwrap();
    store
        .open_tracking_window(Utc.with_ymd_and_hms(2026, 7, 5, 9, 0, 0).unwrap())
        .unwrap();
    let scheduler = IngestScheduler::new(store);
    let registry = SourceRegistry::new(vec![]);
    let logger = RuntimeLogger::new(paths.clone(), DebugLogGate::default());
    let router = SourceIngestRouter {
        registry: &registry,
        scheduler: &scheduler,
        logger: Some(&logger),
        paths: SourceIngestPaths {
            cursor_home: Some(cursor_home),
            cursor_watermark_dir: Some(paths.home.join("watermarks").join("cursor")),
        },
    };

    let outcome = router
        .ingest_hook(
            TokenSourceKind::Cursor,
            HookMetadata {
                source: Some("cursor".to_string()),
                hook_event_name: Some("stop".to_string()),
                transcript_path: Some(
                    dir.path()
                        .join("does-not-exist.jsonl")
                        .to_string_lossy()
                        .to_string(),
                ),
                conversation_id: Some(conversation_id.to_string()),
                timestamp: Some("2026-07-05T11:00:00Z".to_string()),
                ..HookMetadata::default()
            },
        )
        .unwrap();

    assert_eq!(outcome.resolution, SourceResolution::ConversationId);
    assert_eq!(outcome.report.unwrap().inserted, 1);
    let app_log = fs::read_to_string(&paths.app_log).unwrap();
    let ingested_event: Value = app_log
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|value| value["event"] == "source_ingested")
        .expect("source_ingested log");
    assert_source_ingested_schema(
        &ingested_event,
        "stop",
        false,
        true,
        true,
        "conversation_id",
        1,
        0,
        0,
    );
}

#[test]
fn router_falls_back_to_conversation_id_when_cursor_transcript_path_is_unreadable() {
    let dir = tempdir().unwrap();
    let paths = runtime_paths(dir.path());
    let cursor_home = dir.path().join("cursor-home");
    let conversation_id = "cursor-router-unreadable-fallback";
    let transcript_dir = cursor_home
        .join(".cursor/projects/project-a/agent-transcripts")
        .join(conversation_id);
    fs::create_dir_all(&transcript_dir).unwrap();
    fs::write(
        transcript_dir.join(format!("{conversation_id}.jsonl")),
        include_str!("fixtures/cursor-transcript.jsonl"),
    )
    .unwrap();
    let unreadable_transcript_path = dir.path().join("cursor-unreadable.jsonl");
    fs::write(&unreadable_transcript_path, [0xff, 0xfe, 0xfd]).unwrap();
    let store = UsageStore::open(&paths.database).unwrap();
    store
        .open_tracking_window(Utc.with_ymd_and_hms(2026, 7, 5, 9, 0, 0).unwrap())
        .unwrap();
    let scheduler = IngestScheduler::new(store);
    let registry = SourceRegistry::new(vec![]);
    let logger = RuntimeLogger::new(paths.clone(), DebugLogGate::default());
    let router = SourceIngestRouter {
        registry: &registry,
        scheduler: &scheduler,
        logger: Some(&logger),
        paths: SourceIngestPaths {
            cursor_home: Some(cursor_home),
            cursor_watermark_dir: Some(paths.home.join("watermarks").join("cursor")),
        },
    };

    let outcome = router
        .ingest_hook(
            TokenSourceKind::Cursor,
            HookMetadata {
                source: Some("cursor".to_string()),
                hook_event_name: Some("stop".to_string()),
                transcript_path: Some(unreadable_transcript_path.to_string_lossy().to_string()),
                conversation_id: Some(conversation_id.to_string()),
                timestamp: Some("2026-07-05T11:00:00Z".to_string()),
                ..HookMetadata::default()
            },
        )
        .unwrap();

    assert_eq!(outcome.resolution, SourceResolution::ConversationId);
    assert_eq!(outcome.report.unwrap().inserted, 1);
    let app_log = fs::read_to_string(&paths.app_log).unwrap();
    let ingested_event: Value = app_log
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|value| value["event"] == "source_ingested")
        .expect("source_ingested log");
    assert_source_ingested_schema(
        &ingested_event,
        "stop",
        false,
        true,
        true,
        "conversation_id",
        1,
        0,
        0,
    );
}

#[test]
fn router_falls_back_to_conversation_id_when_cursor_transcript_path_is_non_file() {
    let dir = tempdir().unwrap();
    let paths = runtime_paths(dir.path());
    let cursor_home = dir.path().join("cursor-home");
    let conversation_id = "cursor-router-non-file-fallback";
    let transcript_dir = cursor_home
        .join(".cursor/projects/project-a/agent-transcripts")
        .join(conversation_id);
    fs::create_dir_all(&transcript_dir).unwrap();
    fs::write(
        transcript_dir.join(format!("{conversation_id}.jsonl")),
        include_str!("fixtures/cursor-transcript.jsonl"),
    )
    .unwrap();
    let non_file_transcript_path = dir.path().join("cursor-transcript-dir");
    fs::create_dir_all(&non_file_transcript_path).unwrap();
    let store = UsageStore::open(&paths.database).unwrap();
    store
        .open_tracking_window(Utc.with_ymd_and_hms(2026, 7, 5, 9, 0, 0).unwrap())
        .unwrap();
    let scheduler = IngestScheduler::new(store);
    let registry = SourceRegistry::new(vec![]);
    let logger = RuntimeLogger::new(paths.clone(), DebugLogGate::default());
    let router = SourceIngestRouter {
        registry: &registry,
        scheduler: &scheduler,
        logger: Some(&logger),
        paths: SourceIngestPaths {
            cursor_home: Some(cursor_home),
            cursor_watermark_dir: Some(paths.home.join("watermarks").join("cursor")),
        },
    };

    let outcome = router
        .ingest_hook(
            TokenSourceKind::Cursor,
            HookMetadata {
                source: Some("cursor".to_string()),
                hook_event_name: Some("stop".to_string()),
                transcript_path: Some(non_file_transcript_path.to_string_lossy().to_string()),
                conversation_id: Some(conversation_id.to_string()),
                timestamp: Some("2026-07-05T11:00:00Z".to_string()),
                ..HookMetadata::default()
            },
        )
        .unwrap();

    assert_eq!(outcome.resolution, SourceResolution::ConversationId);
    assert_eq!(outcome.report.unwrap().inserted, 1);
    let app_log = fs::read_to_string(&paths.app_log).unwrap();
    let ingested_event: Value = app_log
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|value| value["event"] == "source_ingested")
        .expect("source_ingested log");
    assert_source_ingested_schema(
        &ingested_event,
        "stop",
        false,
        true,
        true,
        "conversation_id",
        1,
        0,
        0,
    );
}

#[test]
fn router_logs_source_collect_failed_for_unreadable_cursor_transcript() {
    let dir = tempdir().unwrap();
    let paths = runtime_paths(dir.path());
    let transcript_path = dir.path().join("cursor-unreadable.jsonl");
    fs::write(&transcript_path, [0xff, 0xfe, 0xfd]).unwrap();
    let store = UsageStore::open(&paths.database).unwrap();
    store
        .open_tracking_window(Utc.with_ymd_and_hms(2026, 7, 5, 9, 0, 0).unwrap())
        .unwrap();
    let scheduler = IngestScheduler::new(store);
    let registry = SourceRegistry::new(vec![]);
    let logger = RuntimeLogger::new(paths.clone(), DebugLogGate::default());
    let router = SourceIngestRouter {
        registry: &registry,
        scheduler: &scheduler,
        logger: Some(&logger),
        paths: SourceIngestPaths {
            cursor_home: None,
            cursor_watermark_dir: Some(paths.home.join("watermarks").join("cursor")),
        },
    };

    let outcome = router
        .ingest_hook(
            TokenSourceKind::Cursor,
            HookMetadata {
                source: Some("cursor".to_string()),
                hook_event_name: Some("stop".to_string()),
                session_id: Some("cursor-unreadable-session".to_string()),
                transcript_path: Some(transcript_path.to_string_lossy().to_string()),
                timestamp: Some("2026-07-05T11:00:00Z".to_string()),
                ..HookMetadata::default()
            },
        )
        .unwrap();

    assert_eq!(outcome.resolution, SourceResolution::TranscriptPath);
    assert_eq!(
        outcome.empty_reason,
        Some(SourceEmptyReason::TranscriptUnreadable)
    );
    let app_log = fs::read_to_string(&paths.app_log).unwrap();
    let failed_event: Value = serde_json::from_str(app_log.lines().last().unwrap()).unwrap();
    assert_eq!(failed_event["event"], "source_collect_failed");
    assert_eq!(failed_event["source"], "cursor");
    assert_eq!(failed_event["hook_event_name"], "stop");
    assert_eq!(failed_event["session_id_present"], true);
    assert_eq!(failed_event["conversation_id_present"], false);
    assert_eq!(failed_event["transcript_path_present"], true);
    assert_eq!(failed_event["resolved_by"], "transcript_path");
    assert_eq!(failed_event["error_kind"], "transcript_unreadable");
    assert!(failed_event.get("transcript_path").is_none());
}

#[test]
fn router_logs_transcript_parse_failure_for_malformed_cursor_transcript() {
    let dir = tempdir().unwrap();
    let paths = runtime_paths(dir.path());
    let transcript_path = dir.path().join("cursor-malformed.jsonl");
    fs::write(&transcript_path, "not-json\n").unwrap();
    let store = UsageStore::open(&paths.database).unwrap();
    store
        .open_tracking_window(Utc.with_ymd_and_hms(2026, 7, 5, 9, 0, 0).unwrap())
        .unwrap();
    let scheduler = IngestScheduler::new(store);
    let registry = SourceRegistry::new(vec![]);
    let logger = RuntimeLogger::new(paths.clone(), DebugLogGate::default());
    let router = SourceIngestRouter {
        registry: &registry,
        scheduler: &scheduler,
        logger: Some(&logger),
        paths: SourceIngestPaths {
            cursor_home: None,
            cursor_watermark_dir: Some(paths.home.join("watermarks").join("cursor")),
        },
    };

    router
        .ingest_hook(
            TokenSourceKind::Cursor,
            HookMetadata {
                source: Some("cursor".to_string()),
                hook_event_name: Some("stop".to_string()),
                session_id: Some("cursor-malformed-session".to_string()),
                transcript_path: Some(transcript_path.to_string_lossy().to_string()),
                timestamp: Some("2026-07-05T11:00:00Z".to_string()),
                ..HookMetadata::default()
            },
        )
        .unwrap_err();

    let app_log = fs::read_to_string(&paths.app_log).unwrap();
    let failed_event: Value = app_log
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|value| value["event"] == "source_collect_failed")
        .expect("source_collect_failed log");
    assert_eq!(failed_event["source"], "cursor");
    assert_eq!(failed_event["resolved_by"], "transcript_path");
    assert_eq!(failed_event["error_kind"], "transcript_parse_failed");
    assert!(failed_event.get("transcript_path").is_none());
}

#[test]
fn router_outside_tracking_window_does_not_commit_cursor_watermark() {
    let dir = tempdir().unwrap();
    let paths = runtime_paths(dir.path());
    let transcript_path = dir.path().join("cursor-outside-tracking.jsonl");
    fs::write(
        &transcript_path,
        include_str!("fixtures/cursor-transcript.jsonl"),
    )
    .unwrap();
    let store = UsageStore::open(&paths.database).unwrap();
    let scheduler = IngestScheduler::new(store);
    let registry = SourceRegistry::new(vec![]);
    let logger = RuntimeLogger::new(paths.clone(), DebugLogGate::default());
    let watermark_dir = paths.home.join("watermarks").join("cursor");
    let metadata = HookMetadata {
        source: Some("cursor".to_string()),
        hook_event_name: Some("stop".to_string()),
        session_id: Some("cursor-outside-tracking".to_string()),
        transcript_path: Some(transcript_path.to_string_lossy().to_string()),
        timestamp: Some("2026-07-05T11:00:00Z".to_string()),
        ..HookMetadata::default()
    };
    let router = SourceIngestRouter {
        registry: &registry,
        scheduler: &scheduler,
        logger: Some(&logger),
        paths: SourceIngestPaths {
            cursor_home: None,
            cursor_watermark_dir: Some(watermark_dir.clone()),
        },
    };

    let outcome = router
        .ingest_hook(TokenSourceKind::Cursor, metadata.clone())
        .unwrap();

    assert_eq!(
        outcome.empty_reason,
        Some(SourceEmptyReason::OutsideTrackingWindow)
    );
    assert_eq!(outcome.report.as_ref().unwrap().inserted, 0);
    assert_eq!(outcome.report.as_ref().unwrap().skipped_outside_tracking, 1);
    let retry = collect_pending_from_transcript_path(
        &transcript_path,
        &metadata,
        CursorTranscriptIdentity::from_path(&transcript_path, &metadata),
        Some(&watermark_dir),
    )
    .unwrap();
    assert!(matches!(retry, CursorCollectResult::Pending(_)));
}
