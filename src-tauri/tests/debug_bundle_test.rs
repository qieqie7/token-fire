use std::fs;

use chrono::{TimeZone, Utc};
use serde_json::json;
use tempfile::tempdir;
use token_fire::adapters::claude::hook_config::ClaudeHookConfigManager;
use token_fire::adapters::codex::hook_config::CodexHookConfigManager;
use token_fire::adapters::codex::status::CodexStatusSource;
use token_fire::adapters::cursor::hook_config::CursorHookConfigManager;
use token_fire::adapters::source::{SourcePaths, SourceStatus, TokenSourceKind};
use token_fire::adapters::traex::hook_config::HookConfigManager;
use token_fire::adapters::traex::resolver::TraexPaths;
use token_fire::adapters::traex::status::TraexStatus;
use token_fire::adapters::traex::status::TraexStatusSource;
use token_fire::app::debug_bundle::{
    create_debug_bundle, create_debug_bundle_with_source_statuses_and_runtime_health,
    create_debug_bundle_with_status_and_runtime_health, RuntimeHealth,
};
use token_fire::app::logging::write_jsonl_event;
use token_fire::app::paths::RuntimePaths;
use token_fire::app::state::{AppState, MenuAction, MenuActionOutcome, SourceHookManagers};
use token_fire::core::observation::{NormalizedObservation, SourceRecordIdConfidence};
use token_fire::core::usage_store::{RetentionPolicy, UsageStore};

fn paths(home: &std::path::Path) -> RuntimePaths {
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

fn observation() -> NormalizedObservation {
    NormalizedObservation {
        source: "traex".to_string(),
        adapter_version: "traex-jsonl-v1".to_string(),
        source_record_id: "session-a:10".to_string(),
        source_record_id_confidence: SourceRecordIdConfidence::Exact,
        session_id: Some("session-a".to_string()),
        turn_id: Some("turn-a".to_string()),
        turn_boundary_id: Some("turn-a".to_string()),
        source_path: Some("/Users/example/private/rollout-session-a.jsonl".to_string()),
        line_no: Some(1),
        byte_offset: Some(10),
        input_tokens: 5,
        output_tokens: 6,
        cached_input_tokens: 0,
        cache_creation_input_tokens: 0,
        reasoning_output_tokens: 0,
        total_tokens: 11,
        cumulative_total_tokens: Some(111),
        model: Some("model-a".to_string()),
        cwd: Some("/Users/example/private-project".to_string()),
        observed_at: Utc.with_ymd_and_hms(2026, 6, 20, 2, 0, 0).unwrap(),
        token_payload_hash: "private-hash".to_string(),
    }
}

#[test]
fn debug_bundle_redacts_paths_and_excludes_raw_content() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path());
    fs::create_dir_all(&paths.logs_dir).unwrap();
    write_jsonl_event(
        &paths.parser_log,
        "parser",
        "info",
        "observation_inserted",
        json!({
            "cwd": "/Users/example/Documents/private-project",
            "source_path": "/Users/example/.trae/cli/sessions/2026/06/20/rollout-019.jsonl",
            "prompt": "secret prompt",
            "tool_payload": {"secret": true},
            "raw_line": "{\"private\":true}",
            "byte_offset": 42
        }),
    )
    .unwrap();
    fs::write(&paths.app_log, "not structured secret raw text\n").unwrap();

    let bundle = create_debug_bundle(&paths, true).unwrap();
    let body = fs::read_to_string(bundle).unwrap();

    assert!(body.contains("observation_inserted"));
    assert!(body.contains("\"sources\""));
    assert!(body.contains("rollout-019.jsonl"));
    assert!(body.contains("byte_offset"));
    assert!(!body.contains("private-project"));
    assert!(!body.contains("secret prompt"));
    assert!(!body.contains("tool_payload"));
    assert!(!body.contains("raw_line"));
    assert!(!body.contains("not structured secret raw text"));
}

#[test]
fn debug_bundle_includes_app_and_hook_build_identity_section() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path());

    let bundle_path = create_debug_bundle(&paths, true).unwrap();
    let body = fs::read_to_string(bundle_path).unwrap();
    let bundle: serde_json::Value = serde_json::from_str(&body).unwrap();

    assert_eq!(
        bundle["build_identity"]["app_runtime"]["version"],
        env!("CARGO_PKG_VERSION")
    );
    assert!(bundle["build_identity"]["app_runtime"]
        .get("dirty")
        .is_some());
    assert!(bundle["build_identity"]["app_runtime"]
        .get("build_time")
        .is_some());
    assert!(bundle["build_identity"].get("hook_sidecar").is_some());
    assert!(bundle["build_identity"].get("mismatch").is_some());
}

#[test]
fn debug_bundle_prefers_recent_hook_log_build_identity_for_mismatch() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path());
    fs::create_dir_all(paths.hook_log.parent().unwrap()).unwrap();
    write_jsonl_event(
        &paths.hook_log,
        "hook",
        "info",
        "hook_forwarded",
        json!({
            "source": "traex",
            "hook_path": "/Users/example/dev/token-fire/src-tauri/target/release/token-fire-hook",
            "version": "0.1.0",
            "git_commit": "stale-hook-commit",
            "git_commit_short": "stale-h",
            "build_time": "unix:1",
            "dirty": false
        }),
    )
    .unwrap();

    let bundle_path = create_debug_bundle(&paths, true).unwrap();
    let body = fs::read_to_string(bundle_path).unwrap();
    let bundle: serde_json::Value = serde_json::from_str(&body).unwrap();

    assert_eq!(
        bundle["build_identity"]["hook_sidecar"]["git_commit"],
        "stale-hook-commit"
    );
    assert_eq!(
        bundle["build_identity"]["hook_sidecar"]["git_commit_short"],
        "stale-h"
    );
    assert_eq!(
        bundle["build_identity"]["hook_sidecar"]["build_time"],
        "unix:1"
    );
    assert_eq!(bundle["build_identity"]["hook_sidecar"]["dirty"], false);
    assert_eq!(bundle["build_identity"]["mismatch"], true);
    assert!(!body.contains("/Users/example/dev/token-fire"));
    assert!(body.contains("dev_target/token-fire-hook"));
}

#[test]
fn debug_bundle_redacts_hook_path() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path());
    fs::create_dir_all(paths.hook_log.parent().unwrap()).unwrap();
    fs::write(
        &paths.hook_log,
        r#"{"event":"hook_forwarded","hook_path":"/Users/example/dev/token-fire/src-tauri/target/release/token-fire-hook","version":"0.1.0"}"#,
    )
    .unwrap();

    let bundle_path = create_debug_bundle(&paths, true).unwrap();
    let body = fs::read_to_string(bundle_path).unwrap();

    assert!(!body.contains("/Users/example/dev/token-fire"));
    assert!(body.contains("dev_target/token-fire-hook"));
}

#[test]
fn jsonl_writer_drops_forbidden_fields_and_preserves_reserved_fields() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path());

    write_jsonl_event(
        &paths.parser_log,
        "parser",
        "info",
        "observation_inserted",
        json!({
            "ts": "caller-ts",
            "level": "caller-level",
            "component": "caller-component",
            "event": "caller-event",
            "prompt": "secret prompt",
            "response": "secret response",
            "tool_arguments": {"secret": true},
            "command_output": "secret output",
            "raw_line": "secret raw",
            "file_content": "secret file",
            "byte_offset": 42
        }),
    )
    .unwrap();

    let body = fs::read_to_string(&paths.parser_log).unwrap();
    let event: serde_json::Value = serde_json::from_str(body.lines().next().unwrap()).unwrap();

    assert_eq!(event["level"], "info");
    assert_eq!(event["component"], "parser");
    assert_eq!(event["event"], "observation_inserted");
    assert_ne!(event["ts"], "caller-ts");
    assert_eq!(event["byte_offset"], 42);
    for key in [
        "prompt",
        "response",
        "tool_arguments",
        "command_output",
        "raw_line",
        "file_content",
    ] {
        assert!(event.get(key).is_none(), "{key} should not be persisted");
        assert!(!body.contains(key));
    }
}

#[test]
fn debug_bundle_recursively_redacts_nested_sensitive_keys() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path());

    write_jsonl_event(
        &paths.parser_log,
        "parser",
        "info",
        "nested_event",
        json!({
            "safe": {
                "items": [
                    {
                        "prompt": "nested secret prompt",
                        "file_content": "nested secret file",
                        "byte_offset": 7
                    }
                ]
            }
        }),
    )
    .unwrap();

    let bundle = create_debug_bundle(&paths, true).unwrap();
    let body = fs::read_to_string(bundle).unwrap();

    assert!(body.contains("nested_event"));
    assert!(body.contains("byte_offset"));
    assert!(!body.contains("nested secret prompt"));
    assert!(!body.contains("nested secret file"));
    assert!(!body.contains("file_content"));
}

#[test]
fn debug_bundle_includes_redacted_sqlite_metadata_when_available() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path());
    let store = UsageStore::open(&paths.database).unwrap();
    store
        .insert_untracked_observation_for_test(&observation())
        .unwrap();

    let bundle = create_debug_bundle(&paths, true).unwrap();
    let body = fs::read_to_string(bundle).unwrap();

    assert!(body.contains("sqlite_metadata"));
    assert!(body.contains("session-a"));
    assert!(body.contains("rollout-session-a.jsonl"));
    assert!(body.contains("byte_offset"));
    assert!(body.contains("total_tokens"));
    assert!(!body.contains("private-project"));
    assert!(!body.contains("private-hash"));
    assert!(!body.contains("source_record_id"));
}

#[test]
fn debug_bundle_filenames_do_not_collide_within_same_second() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path());

    let first = create_debug_bundle(&paths, true).unwrap();
    let second = create_debug_bundle(&paths, true).unwrap();

    assert_ne!(first, second);
    assert!(first.exists());
    assert!(second.exists());
}

#[test]
fn debug_bundle_reports_socket_and_watcher_health_separately() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path());
    fs::create_dir_all(&paths.run_dir).unwrap();
    fs::write(&paths.socket, b"stale socket").unwrap();

    let bundle = create_debug_bundle_with_status_and_runtime_health(
        &paths,
        true,
        &TraexStatus::default(),
        RuntimeHealth {
            socket_ok: true,
            watcher_ok: false,
        },
    )
    .unwrap();
    let body = fs::read_to_string(bundle).unwrap();
    let value: serde_json::Value = serde_json::from_str(&body).unwrap();

    assert_eq!(value["socket_status"], "running");
    assert_eq!(value["watcher_status"], "unavailable");
    assert_ne!(value["watcher_status"], "socket_present");
}

#[test]
fn debug_bundle_sources_summary_reflects_supplied_codex_status() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path());
    let sources = vec![
        SourceStatus {
            source: TokenSourceKind::Traex,
            enabled: true,
            detected: true,
            hook_installed: true,
            hook_executable_exists: true,
            hook_smoke_test_passed: true,
            sessions_readable: true,
            archived_sessions_readable: true,
            last_hook_seen_at: None,
            last_hook_error: None,
        },
        SourceStatus {
            source: TokenSourceKind::Codex,
            enabled: true,
            detected: true,
            hook_installed: false,
            hook_executable_exists: true,
            hook_smoke_test_passed: true,
            sessions_readable: true,
            archived_sessions_readable: false,
            last_hook_seen_at: None,
            last_hook_error: None,
        },
        SourceStatus {
            source: TokenSourceKind::Claude,
            enabled: true,
            detected: true,
            hook_installed: true,
            hook_executable_exists: false,
            hook_smoke_test_passed: false,
            sessions_readable: true,
            archived_sessions_readable: true,
            last_hook_seen_at: None,
            last_hook_error: Some("registered hook executable is missing".to_string()),
        },
        SourceStatus {
            source: TokenSourceKind::Cursor,
            enabled: false,
            detected: false,
            hook_installed: false,
            hook_executable_exists: false,
            hook_smoke_test_passed: true,
            sessions_readable: true,
            archived_sessions_readable: true,
            last_hook_seen_at: None,
            last_hook_error: None,
        },
    ];

    let bundle = create_debug_bundle_with_source_statuses_and_runtime_health(
        &paths,
        true,
        &sources,
        RuntimeHealth {
            socket_ok: true,
            watcher_ok: true,
        },
    )
    .unwrap();
    let value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(bundle).unwrap()).unwrap();

    assert_eq!(value["sources"][1]["source"], "codex");
    assert_eq!(value["sources"][1]["enabled"], true);
    assert_eq!(value["sources"][1]["detected"], true);
    assert_eq!(value["sources"][2]["source"], "claude");
    assert_eq!(value["sources"][2]["enabled"], true);
    assert_eq!(value["sources"][2]["hook_install_status"], "installed");
    assert_eq!(value["sources"][2]["hook_executable_status"], "missing");
    assert_eq!(value["sources"][2]["hook_smoke_test_status"], "not_passed");
    assert!(value["sources"][2]["last_hook_error"]
        .as_str()
        .is_some_and(|message| message.contains("missing")));
    assert_eq!(value["sources"][3]["source"], "cursor");
    assert_eq!(value["sources"][3]["enabled"], false);
    assert_eq!(value["sources"][3]["hook_install_status"], "missing");
    assert_eq!(value["sources"][3]["hook_executable_status"], "missing");
    assert_eq!(value["sources"][3]["hook_smoke_test_status"], "passed");
    assert!(value["sources"][3]["last_hook_error"].is_null());
}

#[test]
fn copy_debug_bundle_includes_registered_claude_hook_with_missing_executable() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path().join("token-fire").as_path());
    let missing_hook_path = dir
        .path()
        .join("TokenFire.app/Contents/MacOS/token-fire-hook");
    let managers = SourceHookManagers::new(
        HookConfigManager::new(dir.path().join("traecli.toml"), paths.backups_dir.clone()),
        CodexHookConfigManager::new(
            dir.path().join("codex-hooks.json"),
            paths.backups_dir.clone(),
        ),
        ClaudeHookConfigManager::new(
            dir.path().join("claude-settings.json"),
            paths.backups_dir.clone(),
        ),
        CursorHookConfigManager::new(
            dir.path().join("cursor-hooks.json"),
            paths.backups_dir.clone(),
        ),
    );
    managers.claude().install(&missing_hook_path).unwrap();
    let app_state = AppState::new_with_source_hook_managers(paths, managers);

    let MenuActionOutcome::DebugBundleCreated(bundle_path) = app_state
        .handle_menu_action(MenuAction::CopyDebugBundle)
        .unwrap()
    else {
        panic!("expected debug bundle path");
    };
    let value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(bundle_path).unwrap()).unwrap();
    let claude = value["sources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|source| source["source"] == "claude")
        .expect("claude source should be present");

    assert_eq!(claude["enabled"], true);
    assert_eq!(claude["detected"], true);
    assert_eq!(claude["hook_install_status"], "installed");
    assert_eq!(claude["hook_executable_status"], "missing");
    assert_eq!(claude["hook_smoke_test_status"], "not_passed");
    assert!(claude["last_hook_error"]
        .as_str()
        .is_some_and(|message| message.contains("missing")));
}

#[test]
fn copy_debug_bundle_reports_traex_config_parse_error_when_runtime_status_exists() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path().join("token-fire").as_path());
    let traex_config = dir.path().join("traecli.toml");
    let hook_log = dir.path().join("hook.log");
    fs::write(&traex_config, "[Stop\n").unwrap();
    write_jsonl_event(
        &hook_log,
        "app",
        "error",
        "hook_internal_failure",
        json!({ "error": "existing runtime error" }),
    )
    .unwrap();
    let managers = SourceHookManagers::new(
        HookConfigManager::new(traex_config.clone(), paths.backups_dir.clone()),
        CodexHookConfigManager::new(
            dir.path().join("codex-hooks.json"),
            paths.backups_dir.clone(),
        ),
        ClaudeHookConfigManager::new(
            dir.path().join("claude-settings.json"),
            paths.backups_dir.clone(),
        ),
        CursorHookConfigManager::new(
            dir.path().join("cursor-hooks.json"),
            paths.backups_dir.clone(),
        ),
    );
    let traex_paths = TraexPaths {
        sessions_dir: dir.path().join("traex-sessions"),
        archived_sessions_dir: dir.path().join("traex-archived"),
    };
    let app_state = AppState::new_with_source_hook_managers_gates_and_source_statuses(
        paths,
        managers,
        Default::default(),
        Default::default(),
        Some(TraexStatusSource::new(traex_config, hook_log, traex_paths)),
        None,
    );

    let MenuActionOutcome::DebugBundleCreated(bundle_path) = app_state
        .handle_menu_action(MenuAction::CopyDebugBundle)
        .unwrap()
    else {
        panic!("expected debug bundle path");
    };
    let value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(bundle_path).unwrap()).unwrap();
    let traex = value["sources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|source| source["source"] == "traex")
        .expect("traex source should be present");

    let error = traex["last_hook_error"].as_str().unwrap();
    assert!(error.contains("hook_internal_failure"));
    assert!(error.contains("failed to parse"));
}

#[test]
fn copy_debug_bundle_reports_codex_config_parse_error_when_runtime_status_exists() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path().join("token-fire").as_path());
    let codex_config = dir.path().join("codex-hooks.json");
    let codex_hook_log = dir.path().join("codex-hook.log");
    let codex_sessions = dir.path().join("codex-sessions");
    let codex_archived = dir.path().join("codex-archived");
    fs::create_dir_all(&codex_sessions).unwrap();
    fs::create_dir_all(&codex_archived).unwrap();
    fs::write(&codex_config, r#"{"hooks": "#).unwrap();
    write_jsonl_event(
        &codex_hook_log,
        "app",
        "error",
        "hook_internal_failure",
        json!({ "source": "codex", "error": "existing codex runtime error" }),
    )
    .unwrap();
    let managers = SourceHookManagers::new(
        HookConfigManager::new(dir.path().join("traecli.toml"), paths.backups_dir.clone()),
        CodexHookConfigManager::new(codex_config.clone(), paths.backups_dir.clone()),
        ClaudeHookConfigManager::new(
            dir.path().join("claude-settings.json"),
            paths.backups_dir.clone(),
        ),
        CursorHookConfigManager::new(
            dir.path().join("cursor-hooks.json"),
            paths.backups_dir.clone(),
        ),
    );
    let app_state = AppState::new_with_source_hook_managers_gates_and_source_statuses(
        paths,
        managers,
        Default::default(),
        Default::default(),
        None,
        Some(CodexStatusSource::new_with_hook_log(
            codex_config,
            codex_hook_log,
            SourcePaths::new(TokenSourceKind::Codex, codex_sessions, codex_archived),
        )),
    );

    let MenuActionOutcome::DebugBundleCreated(bundle_path) = app_state
        .handle_menu_action(MenuAction::CopyDebugBundle)
        .unwrap()
    else {
        panic!("expected debug bundle path");
    };
    let value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(bundle_path).unwrap()).unwrap();
    let codex = value["sources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|source| source["source"] == "codex")
        .expect("codex source should be present");

    let error = codex["last_hook_error"].as_str().unwrap();
    assert!(error.contains("hook_internal_failure"));
    assert!(error.contains("failed to parse"));
}

#[test]
fn debug_bundle_includes_retention_metadata() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path());
    let mut store = UsageStore::open(&paths.database).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 6, 26, 12, 0, 0).unwrap();
    store
        .run_retention_if_due(now, RetentionPolicy::default())
        .unwrap();
    store
        .record_retention_failure(
            now + chrono::Duration::minutes(5),
            "sqlite_retention_failed",
        )
        .unwrap();

    let bundle_path = create_debug_bundle(&paths, true).unwrap();
    let bundle: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(bundle_path).unwrap()).unwrap();
    let retention = &bundle["retention"];

    assert_eq!(retention["policy_days"], json!(365));
    assert_eq!(retention["min_interval_hours"], json!(24));
    assert_eq!(retention["last_success_at"], json!(now.to_rfc3339()));
    assert_eq!(retention["last_deleted_observations"], json!(0));
    assert_eq!(
        retention["last_failure_at"],
        json!((now + chrono::Duration::minutes(5)).to_rfc3339())
    );
    assert_eq!(
        retention["last_error_kind"],
        json!("sqlite_retention_failed")
    );
}
