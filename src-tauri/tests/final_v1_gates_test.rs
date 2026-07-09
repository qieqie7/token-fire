use std::fs;
use std::path::Path;
use std::time::Duration as StdDuration;

use chrono::{Duration, TimeZone, Utc};
use serde_json::json;
use tempfile::tempdir;
use token_fire::adapters::source::{SourcePaths, SourceRegistry, TokenSourceKind};
use token_fire::adapters::traex::resolver::TraexPaths;
use token_fire::adapters::traex::status::collect_traex_status;
use token_fire::app::ingest_scheduler::IngestScheduler;
use token_fire::app::logging::{append_app_log, write_jsonl_event, DebugLogGate, RuntimeLogger};
use token_fire::app::menu::{menu_labels, MenuAction};
use token_fire::app::paths::RuntimePaths;
use token_fire::app::runtime::{
    handle_runtime_event, handle_runtime_event_with_logger, RuntimeEvent,
};
use token_fire::app::state::AppState;
use token_fire::app::tracking::TrackingGate;
use token_fire::app::tray::{
    copy_debug_bundle_path_with, tray_icon_path, tray_title_from_cost_summary,
    tray_title_from_total, TRAY_REFRESH_FALLBACK_INTERVAL,
};
use token_fire::core::pricing::{CostPeriodSummary, PricingStatus};
use token_fire::core::usage_store::UsageStore;

fn traex_paths(root: &Path) -> TraexPaths {
    TraexPaths {
        sessions_dir: root.join("sessions"),
        archived_sessions_dir: root.join("archived_sessions"),
    }
}

fn traex_registry(root: &Path) -> SourceRegistry {
    SourceRegistry::new(vec![SourcePaths::from(&traex_paths(root))])
}

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

#[test]
fn closed_tracking_window_rows_are_still_ingested_for_late_delivery() {
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
    store
        .close_tracking_window(Utc.with_ymd_and_hms(2026, 6, 20, 3, 10, 0).unwrap())
        .unwrap();

    let scheduler = IngestScheduler::new(store);
    let report = scheduler.ingest_path(&transcript).unwrap();

    assert_eq!(report.inserted, 2);
    assert_eq!(report.skipped_outside_tracking, 0);
}

#[test]
fn paused_runtime_gate_drops_hook_and_watcher_ingestion() {
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
    let gate = TrackingGate::new();
    gate.pause();

    let report = handle_runtime_event(
        RuntimeEvent::TranscriptChanged {
            source: TokenSourceKind::Traex,
            path: transcript,
        },
        &traex_registry(dir.path()),
        &scheduler,
        &gate,
    )
    .unwrap();

    assert!(report.is_none());
}

#[test]
fn collects_traex_status_from_config_hook_path_log_and_directories() {
    let dir = tempdir().unwrap();
    let paths = traex_paths(dir.path());
    fs::create_dir_all(&paths.sessions_dir).unwrap();
    fs::create_dir_all(&paths.archived_sessions_dir).unwrap();

    let hook_path = dir
        .path()
        .join("TokenFire.app/Contents/MacOS/token-fire-hook");
    fs::create_dir_all(hook_path.parent().unwrap()).unwrap();
    fs::write(&hook_path, "").unwrap();
    make_executable(&hook_path);

    let config_path = dir.path().join("traecli.toml");
    fs::write(
        &config_path,
        format!(
            r#"[[Stop.hooks]]
command = "'{}' --source traex --owner token-fire"
type = "command"
timeout = 5
"#,
            hook_path.display()
        ),
    )
    .unwrap();

    let hook_log = dir.path().join("logs").join("hook.log");
    write_jsonl_event(
        &hook_log,
        "hook",
        "info",
        "hook_forwarded",
        json!({ "source": "traex", "hook_path": hook_path.to_string_lossy() }),
    )
    .unwrap();

    let status = collect_traex_status(&config_path, &hook_log, &paths);

    assert!(status.hook_installed);
    assert!(status.hook_executable_exists);
    assert!(status.hook_last_seen_at.is_some());
    assert!(status.hook_smoke_test_passed);
    assert!(status.sessions_readable);
    assert!(status.archived_sessions_readable);
}

#[test]
fn hook_smoke_requires_current_installed_hook_path() {
    let dir = tempdir().unwrap();
    let paths = traex_paths(dir.path());
    fs::create_dir_all(&paths.sessions_dir).unwrap();
    fs::create_dir_all(&paths.archived_sessions_dir).unwrap();

    let current_hook = dir
        .path()
        .join("Current.app/Contents/MacOS/token-fire-hook");
    fs::create_dir_all(current_hook.parent().unwrap()).unwrap();
    fs::write(&current_hook, "").unwrap();
    make_executable(&current_hook);

    let old_hook = dir.path().join("Old.app/Contents/MacOS/token-fire-hook");
    fs::create_dir_all(old_hook.parent().unwrap()).unwrap();
    fs::write(&old_hook, "").unwrap();
    make_executable(&old_hook);

    let config_path = dir.path().join("traecli.toml");
    fs::write(
        &config_path,
        format!(
            r#"[[Stop.hooks]]
command = "'{}' --source traex --owner token-fire"
type = "command"
timeout = 5
"#,
            current_hook.display()
        ),
    )
    .unwrap();

    let hook_log = dir.path().join("logs").join("hook.log");
    write_jsonl_event(
        &hook_log,
        "hook",
        "info",
        "hook_forwarded",
        json!({ "source": "traex", "hook_path": old_hook.to_string_lossy() }),
    )
    .unwrap();

    let status = collect_traex_status(&config_path, &hook_log, &paths);

    assert!(status.hook_last_seen_at.is_none());
    assert!(!status.hook_smoke_test_passed);
}

#[test]
fn hook_smoke_accepts_canonical_equivalent_hook_path() {
    let dir = tempdir().unwrap();
    let paths = traex_paths(dir.path());
    fs::create_dir_all(&paths.sessions_dir).unwrap();
    fs::create_dir_all(&paths.archived_sessions_dir).unwrap();

    let real_root = dir.path().join("Real.app");
    let hook_path = real_root.join("Contents/MacOS/token-fire-hook");
    fs::create_dir_all(hook_path.parent().unwrap()).unwrap();
    fs::write(&hook_path, "").unwrap();
    make_executable(&hook_path);

    let alias_root = dir.path().join("Alias.app");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&real_root, &alias_root).unwrap();
    #[cfg(not(unix))]
    fs::create_dir_all(&alias_root).unwrap();
    let config_hook_path = alias_root.join("Contents/MacOS/token-fire-hook");

    let config_path = dir.path().join("traecli.toml");
    fs::write(
        &config_path,
        format!(
            r#"[[Stop.hooks]]
command = "'{}' --source traex --owner token-fire"
type = "command"
timeout = 5
"#,
            config_hook_path.display()
        ),
    )
    .unwrap();

    let hook_log = dir.path().join("logs").join("hook.log");
    write_jsonl_event(
        &hook_log,
        "hook",
        "info",
        "hook_forwarded",
        json!({ "source": "traex", "hook_path": hook_path.canonicalize().unwrap().to_string_lossy() }),
    )
    .unwrap();

    let status = collect_traex_status(&config_path, &hook_log, &paths);

    assert!(status.hook_last_seen_at.is_some());
    assert!(status.hook_smoke_test_passed);
}

#[test]
fn hook_smoke_status_expires_when_last_seen_is_stale() {
    let dir = tempdir().unwrap();
    let paths = traex_paths(dir.path());
    fs::create_dir_all(&paths.sessions_dir).unwrap();
    fs::create_dir_all(&paths.archived_sessions_dir).unwrap();

    let hook_path = dir
        .path()
        .join("TokenFire.app/Contents/MacOS/token-fire-hook");
    fs::create_dir_all(hook_path.parent().unwrap()).unwrap();
    fs::write(&hook_path, "").unwrap();

    let config_path = dir.path().join("traecli.toml");
    fs::write(
        &config_path,
        format!(
            r#"[[Stop.hooks]]
command = "'{}' --source traex --owner token-fire"
type = "command"
timeout = 5
"#,
            hook_path.display()
        ),
    )
    .unwrap();

    let hook_log = dir.path().join("logs").join("hook.log");
    fs::create_dir_all(hook_log.parent().unwrap()).unwrap();
    fs::write(
        &hook_log,
        r#"{"ts":"2000-01-01T00:00:00Z","event":"hook_received"}"#,
    )
    .unwrap();

    let status = collect_traex_status(&config_path, &hook_log, &paths);

    assert!(status.hook_last_seen_at.is_none());
    assert!(!status.hook_smoke_test_passed);
}

#[test]
fn debug_logging_expires_after_thirty_minutes() {
    let gate = DebugLogGate::default();
    let now = Utc.with_ymd_and_hms(2026, 6, 20, 3, 0, 0).unwrap();

    assert!(gate.should_write("info", now));
    assert!(!gate.should_write("debug", now));

    gate.enable_debug_for_30_minutes(now);

    assert!(gate.should_write("debug", now + Duration::minutes(29)));
    assert!(!gate.should_write("debug", now + Duration::minutes(31)));
}

#[test]
fn app_runtime_logger_suppresses_debug_by_default() {
    let dir = tempdir().unwrap();
    let paths = runtime_paths(dir.path());
    let logger = RuntimeLogger::new(paths.clone(), DebugLogGate::default());
    let scheduler = IngestScheduler::new(UsageStore::open(&paths.database).unwrap());
    let gate = TrackingGate::new();
    gate.pause();

    handle_runtime_event_with_logger(
        RuntimeEvent::TranscriptChanged {
            source: TokenSourceKind::Traex,
            path: dir.path().join("missing.jsonl"),
        },
        &traex_registry(dir.path()),
        &scheduler,
        &gate,
        &logger,
    )
    .unwrap();
    append_app_log(
        &logger,
        "info",
        "info_probe",
        json!({ "source": "runtime" }),
    )
    .unwrap();

    let body = fs::read_to_string(&paths.app_log).unwrap();
    assert!(!body.contains("runtime_event_received"));
    assert!(body.contains("info_probe"));
    assert!(!body.contains("secret"));
}

#[test]
fn menu_action_enables_debug_for_app_runtime_logger_shared_gate() {
    let dir = tempdir().unwrap();
    let paths = runtime_paths(dir.path());
    let gate = DebugLogGate::default();
    let app_state = AppState::new_with_gates(paths.clone(), TrackingGate::new(), gate.clone());
    let logger = RuntimeLogger::new(paths.clone(), gate);
    let scheduler = IngestScheduler::new(UsageStore::open(&paths.database).unwrap());
    let tracking_gate = TrackingGate::new();
    tracking_gate.pause();

    handle_runtime_event_with_logger(
        RuntimeEvent::TranscriptChanged {
            source: TokenSourceKind::Traex,
            path: dir.path().join("before-menu.jsonl"),
        },
        &traex_registry(dir.path()),
        &scheduler,
        &tracking_gate,
        &logger,
    )
    .unwrap();
    app_state
        .handle_menu_action(MenuAction::EnableDebugLogging)
        .unwrap();
    handle_runtime_event_with_logger(
        RuntimeEvent::TranscriptChanged {
            source: TokenSourceKind::Traex,
            path: dir.path().join("after-menu.jsonl"),
        },
        &traex_registry(dir.path()),
        &scheduler,
        &tracking_gate,
        &logger,
    )
    .unwrap();

    let body = fs::read_to_string(&paths.app_log).unwrap();
    assert_eq!(body.matches("runtime_event_received").count(), 1);
}

#[test]
fn tray_title_and_menu_labels_cover_v1_ui_surface() {
    let labels = menu_labels();
    let summary = CostPeriodSummary {
        estimated_cost: 0.70,
        total_tokens: 128_400,
        pricing_status: PricingStatus::Rule,
    };
    let large_summary = CostPeriodSummary {
        estimated_cost: 18_312.98,
        total_tokens: 1_250_000,
        pricing_status: PricingStatus::Mixed,
    };

    assert_eq!(tray_title_from_total(128_400), "128K");
    assert_eq!(tray_title_from_total(1_250_000), "1.3M");
    assert_eq!(tray_title_from_cost_summary(&summary), " ¥0.70 · 128K");
    assert_eq!(
        tray_title_from_cost_summary(&large_summary),
        " ¥18.3k · 1.3M"
    );
    let icon_path = tray_icon_path();
    assert!(icon_path.ends_with("src-tauri/icons/tray-icon.png"));
    assert!(icon_path.exists());
    assert!(fs::metadata(&icon_path).unwrap().len() > 100);
    assert_eq!(labels.source_submenu, "来源");
    assert_eq!(labels.traex_source, "TraeX");
    assert_eq!(labels.codex_source, "Codex");
    assert_eq!(labels.claude_source, "Claude");
    assert_eq!(labels.cursor_source, "Cursor");
    assert_eq!(labels.enable_debug_logging, "开启调试日志");
    assert_eq!(
        MenuAction::ToggleSourceHook(TokenSourceKind::Traex).to_menu_id(),
        "toggle_source_traex_hook"
    );
    assert_eq!(MenuAction::OpenLogs.to_menu_id(), "open_logs");
    assert_eq!(MenuAction::Quit.to_menu_id(), "quit");
    assert_eq!(
        MenuAction::ToggleSourceHook(TokenSourceKind::Codex).to_menu_id(),
        "toggle_source_codex_hook"
    );
    assert_eq!(
        MenuAction::ToggleSourceHook(TokenSourceKind::Claude).to_menu_id(),
        "toggle_source_claude_hook"
    );
    assert_eq!(
        MenuAction::ToggleSourceHook(TokenSourceKind::Cursor).to_menu_id(),
        "toggle_source_cursor_hook"
    );
    assert_eq!(
        MenuAction::EnableDebugLogging.to_menu_id(),
        "enable_debug_logging"
    );
    assert_ne!(labels.source_submenu, "安装 TokenFire Hooks");
}

#[test]
fn app_bundle_smoke_checks_packaged_tray_icon_resource() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let smoke = fs::read_to_string(manifest_dir.join("../scripts/app-bundle-smoke.sh")).unwrap();

    assert!(smoke.contains("Contents/Resources/icons/tray-icon.png"));
}

#[test]
fn smoke_scripts_build_sidecar_before_requiring_external_bin() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let app_smoke =
        fs::read_to_string(manifest_dir.join("../scripts/app-bundle-smoke.sh")).unwrap();
    let release_smoke =
        fs::read_to_string(manifest_dir.join("../scripts/release-smoke.sh")).unwrap();
    let external_bin_override = r#"TAURI_CONFIG='{"bundle":{"externalBin":[]}}' cargo build"#;

    assert!(app_smoke.contains(external_bin_override));
    assert!(release_smoke.contains(external_bin_override));
}

#[test]
fn tray_refresh_fallback_interval_is_low_frequency() {
    assert_eq!(TRAY_REFRESH_FALLBACK_INTERVAL, StdDuration::from_secs(300));
    assert_ne!(TRAY_REFRESH_FALLBACK_INTERVAL, StdDuration::from_secs(1));
}

#[test]
fn maintenance_menu_does_not_expose_widget_toggle() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let menu_rs = std::fs::read_to_string(manifest_dir.join("src/app/menu.rs")).unwrap();
    let tray_rs = std::fs::read_to_string(manifest_dir.join("src/app/tray.rs")).unwrap();

    assert!(!menu_rs.contains("toggle_widget"));
    assert!(!tray_rs.contains("toggle_widget"));
}

#[test]
fn profile_window_uses_tray_rect_instead_of_click_position() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let tray_rs = std::fs::read_to_string(manifest_dir.join("src/app/tray.rs")).unwrap();
    let click_handler = tray_rs
        .split(".on_tray_icon_event")
        .nth(1)
        .expect("tray click handler exists");

    assert!(click_handler.contains("show_profile_window_near_tray(tray.app_handle(), rect)"));
    assert!(!click_handler.contains("show_profile_window_near_tray(tray.app_handle(), position)"));
    assert!(!tray_rs.contains("position.x / scale_factor - 214.0"));
}

#[test]
fn visible_widget_commands_are_not_registered_in_main_runtime() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let main_rs = std::fs::read_to_string(manifest_dir.join("src/main.rs")).unwrap();

    assert!(main_rs.contains("profile_summary"));
    assert!(!main_rs.contains("widget_usage_series,"));
    assert!(!main_rs.contains("widget_cost_summary"));
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) {}

#[test]
fn copy_debug_bundle_side_effect_copies_bundle_path_text() {
    let dir = tempdir().unwrap();
    let bundle = dir.path().join("debug-bundles/token-fire-debug.json");
    let mut copied = None;

    copy_debug_bundle_path_with(&bundle, |text| {
        copied = Some(text.to_string());
        Ok(())
    })
    .unwrap();

    assert_eq!(copied, Some(bundle.to_string_lossy().to_string()));
}

#[test]
fn tauri_bundle_identifier_does_not_end_with_app_extension() {
    let config =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tauri.conf.json")).unwrap();

    assert!(config.contains(r#""identifier": "dev.tokenfire.desktop""#));
    assert!(!config.contains(r#""identifier": "dev.tokenfire.app""#));
}

#[test]
fn retention_policy_is_not_exposed_to_user_surfaces() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let main_rs = fs::read_to_string(manifest_dir.join("src/main.rs")).unwrap();
    let state_rs = fs::read_to_string(manifest_dir.join("src/app/state.rs")).unwrap();
    let menu_rs = fs::read_to_string(manifest_dir.join("src/app/menu.rs")).unwrap();
    let tray_rs = fs::read_to_string(manifest_dir.join("src/app/tray.rs")).unwrap();
    let hook_rs = fs::read_to_string(manifest_dir.join("src/bin/token_fire_hook.rs")).unwrap();

    assert!(main_rs.contains("profile_summary"));
    assert!(!main_rs.contains("tauri::generate_handler![widget_state]"));
    assert!(!main_rs.contains("retention"));
    assert!(!state_rs.contains("Retention"));
    assert!(!state_rs.contains("retention"));
    assert!(!menu_rs.contains("Retention"));
    assert!(!menu_rs.contains("retention"));
    assert!(!tray_rs.contains("Retention"));
    assert!(!tray_rs.contains("retention"));
    assert!(!hook_rs.contains("RETENTION"));
    assert!(!hook_rs.contains("retention"));
}
