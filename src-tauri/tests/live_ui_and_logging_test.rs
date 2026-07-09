use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::sync::Mutex;

use chrono::{Local, TimeZone, Utc};
use serde_json::Value;
use tempfile::tempdir;
use token_fire::adapters::source::{SourcePaths, SourceRegistry, TokenSourceKind};
use token_fire::adapters::traex::resolver::TraexPaths;
use token_fire::adapters::HookMetadata;
use token_fire::app::ingest_scheduler::IngestScheduler;
use token_fire::app::logging::{DebugLogGate, RuntimeLogSinks, RuntimeLogger};
use token_fire::app::menu::MenuAction;
use token_fire::app::paths::RuntimePaths;
use token_fire::app::runtime::{
    handle_runtime_event, handle_runtime_event_with_logger, RuntimeEvent,
};
use token_fire::app::state::{AppState, MenuActionOutcome};
use token_fire::app::tracking::TrackingGate;
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

static HOME_ENV_LOCK: Mutex<()> = Mutex::new(());

struct HomeEnvRestore(Option<OsString>);

impl HomeEnvRestore {
    fn set(home: &Path) -> Self {
        let original_home = std::env::var_os("HOME");
        unsafe {
            std::env::set_var("HOME", home);
        }
        Self(original_home)
    }
}

impl Drop for HomeEnvRestore {
    fn drop(&mut self) {
        match self.0.take() {
            Some(home) => unsafe {
                std::env::set_var("HOME", home);
            },
            None => unsafe {
                std::env::remove_var("HOME");
            },
        }
    }
}

fn traex_paths(root: &Path) -> TraexPaths {
    TraexPaths {
        sessions_dir: root.join("sessions"),
        archived_sessions_dir: root.join("archived_sessions"),
    }
}

fn traex_registry(root: &Path) -> SourceRegistry {
    SourceRegistry::new(vec![SourcePaths::from(&traex_paths(root))])
}

#[test]
fn menu_actions_report_logs_and_tracking_side_effects_without_widget_toggle() {
    let dir = tempdir().unwrap();
    let paths = runtime_paths(dir.path());
    let state = AppState::new(paths.clone());

    assert_eq!(
        state.handle_menu_action(MenuAction::OpenLogs).unwrap(),
        MenuActionOutcome::LogsDirectoryRequested(paths.logs_dir.clone())
    );
    assert!(paths.logs_dir.exists());
    assert_eq!(
        state.handle_menu_action(MenuAction::PauseTracking).unwrap(),
        MenuActionOutcome::TrackingPaused
    );
    assert_eq!(
        state
            .handle_menu_action(MenuAction::ResumeTracking)
            .unwrap(),
        MenuActionOutcome::TrackingResumed
    );
}

#[test]
fn ingest_writes_parser_and_db_structured_events() {
    let dir = tempdir().unwrap();
    let paths = runtime_paths(dir.path());
    let transcript = dir
        .path()
        .join("sessions/2026/06/20/rollout-019-session-a.jsonl");
    fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    fs::write(&transcript, include_str!("fixtures/traex-session.jsonl")).unwrap();

    let store = UsageStore::open(&paths.database).unwrap();
    store
        .open_tracking_window(Utc.with_ymd_and_hms(2026, 6, 20, 3, 0, 0).unwrap())
        .unwrap();
    let scheduler = IngestScheduler::new_with_logs(
        store,
        RuntimeLogSinks::new(paths.clone(), DebugLogGate::default()),
    );

    scheduler.ingest_path(&transcript).unwrap();
    scheduler.ingest_path(&transcript).unwrap();

    let parser_log = fs::read_to_string(&paths.parser_log).unwrap();
    let db_log = fs::read_to_string(&paths.db_log).unwrap();
    assert!(parser_log
        .lines()
        .all(|line| serde_json::from_str::<Value>(line).is_ok()));
    assert!(db_log
        .lines()
        .all(|line| serde_json::from_str::<Value>(line).is_ok()));
    assert!(parser_log.contains("token_count_row_seen"));
    assert!(db_log.contains("observation_inserted"));
    assert!(db_log.contains("observation_duplicate"));
}

#[test]
fn runtime_logs_file_changes_and_ignores_non_stop_hooks() {
    let dir = tempdir().unwrap();
    let paths = runtime_paths(dir.path());
    let transcript = dir
        .path()
        .join("sessions/2026/06/20/rollout-019-session-a.jsonl");
    fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    fs::write(&transcript, include_str!("fixtures/traex-session.jsonl")).unwrap();

    let store = UsageStore::open(&paths.database).unwrap();
    store
        .open_tracking_window(Utc.with_ymd_and_hms(2026, 6, 20, 3, 0, 0).unwrap())
        .unwrap();
    let scheduler = IngestScheduler::new_with_logs(
        store,
        RuntimeLogSinks::new(paths.clone(), DebugLogGate::default()),
    );
    let gate = TrackingGate::new();
    let logger = RuntimeLogger::new(paths.clone(), DebugLogGate::default());

    let session_end = HookMetadata {
        hook_event_name: Some("SessionEnd".to_string()),
        transcript_path: Some(transcript.to_string_lossy().to_string()),
        ..HookMetadata::default()
    };
    let ignored = handle_runtime_event(
        RuntimeEvent::Hook {
            source: TokenSourceKind::Traex,
            metadata: session_end,
        },
        &traex_registry(dir.path()),
        &scheduler,
        &gate,
    )
    .unwrap();
    assert!(ignored.is_none());
    let today = Local.with_ymd_and_hms(2026, 6, 20, 12, 0, 0).unwrap();
    assert_eq!(
        UsageStore::open(&paths.database)
            .unwrap()
            .today_total(today)
            .unwrap(),
        0
    );

    let report = handle_runtime_event_with_logger(
        RuntimeEvent::TranscriptChanged {
            source: TokenSourceKind::Traex,
            path: transcript,
        },
        &traex_registry(dir.path()),
        &scheduler,
        &gate,
        &logger,
    )
    .unwrap();
    assert!(report.is_some());
    let app_log = fs::read_to_string(&paths.app_log).unwrap();
    assert!(app_log.contains("file_changed"));
    let source_ingested = app_log
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|value| value["event"] == "source_ingested")
        .expect("source_ingested log for file change");
    assert_eq!(source_ingested["source"], "traex");
    assert_eq!(source_ingested["hook_event_name"], Value::Null);
    assert_eq!(source_ingested["session_id_present"], false);
    assert_eq!(source_ingested["conversation_id_present"], false);
    assert_eq!(source_ingested["transcript_path_present"], true);
    assert_eq!(source_ingested["resolved_by"], "source_registry");
    assert_eq!(source_ingested["inserted"], 2);
    assert_eq!(source_ingested["duplicates"], 0);
    assert_eq!(source_ingested["skipped_outside_tracking"], 0);
    assert!(source_ingested.get("source_path").is_none());
}

#[test]
fn unresolved_stop_hook_transcript_writes_structured_warning() {
    let dir = tempdir().unwrap();
    let paths = runtime_paths(dir.path());
    let store = UsageStore::open(&paths.database).unwrap();
    store
        .open_tracking_window(Utc.with_ymd_and_hms(2026, 6, 20, 3, 0, 0).unwrap())
        .unwrap();
    let scheduler = IngestScheduler::new_with_logs(
        store,
        RuntimeLogSinks::new(paths.clone(), DebugLogGate::default()),
    );
    let logger = RuntimeLogger::new(paths.clone(), DebugLogGate::default());
    let gate = TrackingGate::new();
    let metadata = HookMetadata {
        hook_event_name: Some("Stop".to_string()),
        session_id: Some("missing-session".to_string()),
        ..HookMetadata::default()
    };

    let report = handle_runtime_event_with_logger(
        RuntimeEvent::Hook {
            source: TokenSourceKind::Traex,
            metadata,
        },
        &traex_registry(dir.path()),
        &scheduler,
        &gate,
        &logger,
    )
    .unwrap();

    assert!(report.is_none());
    let app_log = fs::read_to_string(&paths.app_log).unwrap();
    assert!(app_log.contains("hook_transcript_unresolved"));
    assert!(app_log.contains("missing-session"));
    assert!(app_log.contains("\"hook_event_name\":\"Stop\""));
    assert!(app_log.contains("\"transcript_path_present\":false"));
}

#[test]
fn cursor_source_collect_empty_log_has_support_fields() {
    let dir = tempdir().unwrap();
    let _home_env_lock = HOME_ENV_LOCK.lock().unwrap();
    let _home_env_restore = HomeEnvRestore::set(dir.path());
    let paths = runtime_paths(dir.path());
    let transcript_path = dir.path().join("cursor-empty-log.jsonl");
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
    let gate = TrackingGate::new();
    let logger = RuntimeLogger::new(paths.clone(), DebugLogGate::default());
    let metadata = HookMetadata {
        source: Some("cursor".to_string()),
        hook_event_name: Some("stop".to_string()),
        session_id: Some("cursor-empty-log-session".to_string()),
        conversation_id: Some("cursor-empty-log-conversation".to_string()),
        transcript_path: Some(transcript_path.to_string_lossy().to_string()),
        timestamp: Some("2026-07-05T11:00:00Z".to_string()),
        ..HookMetadata::default()
    };

    handle_runtime_event_with_logger(
        RuntimeEvent::Hook {
            source: TokenSourceKind::Cursor,
            metadata: metadata.clone(),
        },
        &registry,
        &scheduler,
        &gate,
        &logger,
    )
    .unwrap();
    handle_runtime_event_with_logger(
        RuntimeEvent::Hook {
            source: TokenSourceKind::Cursor,
            metadata,
        },
        &registry,
        &scheduler,
        &gate,
        &logger,
    )
    .unwrap();

    let app_log = fs::read_to_string(&paths.app_log).unwrap();
    let empty_event = app_log
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|value| value["event"] == "source_collect_empty")
        .expect("source_collect_empty log");

    assert_eq!(empty_event["source"], "cursor");
    assert_eq!(empty_event["hook_event_name"], "stop");
    assert_eq!(empty_event["session_id_present"], true);
    assert_eq!(empty_event["conversation_id_present"], true);
    assert_eq!(empty_event["transcript_path_present"], true);
    assert_eq!(empty_event["resolved_by"], "transcript_path");
    assert_eq!(empty_event["empty_reason"], "watermark_at_eof");
    assert_eq!(empty_event["inserted"], Value::Null);
    assert_eq!(empty_event["duplicates"], Value::Null);
    assert_eq!(empty_event["skipped_outside_tracking"], Value::Null);
    assert!(empty_event.get("transcript_path").is_none());
}

#[test]
fn cursor_source_ingested_log_has_resolution_and_counts() {
    let dir = tempdir().unwrap();
    let _home_env_lock = HOME_ENV_LOCK.lock().unwrap();
    let _home_env_restore = HomeEnvRestore::set(dir.path());
    let paths = runtime_paths(dir.path());
    let transcript_path = dir.path().join("cursor-ingested-log.jsonl");
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
    let gate = TrackingGate::new();
    let logger = RuntimeLogger::new(paths.clone(), DebugLogGate::default());
    let metadata = HookMetadata {
        source: Some("cursor".to_string()),
        hook_event_name: Some("stop".to_string()),
        session_id: Some("cursor-ingested-log-session".to_string()),
        conversation_id: Some("cursor-ingested-log-conversation".to_string()),
        transcript_path: Some(transcript_path.to_string_lossy().to_string()),
        timestamp: Some("2026-07-05T11:00:00Z".to_string()),
        ..HookMetadata::default()
    };

    handle_runtime_event_with_logger(
        RuntimeEvent::Hook {
            source: TokenSourceKind::Cursor,
            metadata,
        },
        &registry,
        &scheduler,
        &gate,
        &logger,
    )
    .unwrap();

    let app_log = fs::read_to_string(&paths.app_log).unwrap();
    let ingested_event = app_log
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|value| value["event"] == "source_ingested")
        .expect("source_ingested log");

    assert_eq!(ingested_event["source"], "cursor");
    assert_eq!(ingested_event["hook_event_name"], "stop");
    assert_eq!(ingested_event["session_id_present"], true);
    assert_eq!(ingested_event["conversation_id_present"], true);
    assert_eq!(ingested_event["transcript_path_present"], true);
    assert_eq!(ingested_event["resolved_by"], "transcript_path");
    assert_eq!(ingested_event["inserted"], 1);
    assert_eq!(ingested_event["duplicates"], 0);
    assert_eq!(ingested_event["skipped_outside_tracking"], 0);
    assert!(ingested_event.get("transcript_path").is_none());
}

#[test]
fn app_side_hook_receive_is_logged_after_successful_socket_parse() {
    use std::io::Write;
    use std::os::unix::net::UnixStream;
    use std::sync::mpsc;
    use std::time::Duration;
    use token_fire::app::socket_server::SocketServer;

    let dir = tempdir().unwrap();
    let paths = runtime_paths(dir.path());
    let (tx, rx) = mpsc::channel();
    let _server = SocketServer::start_with_logger(
        paths.socket.clone(),
        tx,
        RuntimeLogger::new(paths.clone(), DebugLogGate::default()),
    )
    .unwrap();

    let mut stream = UnixStream::connect(&paths.socket).unwrap();
    stream
        .write_all(br#"{"source":"traex","hook_event_name":"Stop"}"#)
        .unwrap();
    drop(stream);

    rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let app_log = fs::read_to_string(&paths.app_log).unwrap();
    assert!(app_log.contains("hook_received"));
    assert!(app_log.contains("\"hook_event_name\":\"Stop\""));
}
