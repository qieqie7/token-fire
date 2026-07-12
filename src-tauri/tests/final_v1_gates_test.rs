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
fn release_pipeline_checks_packaged_tray_icon_resource() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let pipeline = fs::read_to_string(manifest_dir.join("../scripts/release-pipeline.sh")).unwrap();

    assert!(pipeline.contains("Contents/Resources/icons/tray-icon.png"));
}

#[test]
fn release_pipeline_builds_sidecar_before_requiring_external_bin() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let pipeline = fs::read_to_string(manifest_dir.join("../scripts/release-pipeline.sh")).unwrap();
    let external_bin_override = r#"TAURI_CONFIG='{"bundle":{"externalBin":[]}}' cargo build"#;

    assert!(pipeline.contains(external_bin_override));
}

#[test]
fn release_pipeline_runs_version_guard_before_builds() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let pipeline = fs::read_to_string(manifest_dir.join("../scripts/release-pipeline.sh")).unwrap();
    let guard_index = pipeline.find("pnpm release:check-version").unwrap();
    let cargo_build_index = pipeline.find("cargo build").unwrap();
    let pnpm_build_index = pipeline.find("pnpm build").unwrap();
    let tauri_build_index = pipeline.find("pnpm tauri build").unwrap();

    assert!(guard_index < cargo_build_index);
    assert!(guard_index < pnpm_build_index);
    assert!(guard_index < tauri_build_index);
}

#[test]
fn local_release_script_uses_full_dmg_pipeline() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let script_path = manifest_dir.join("../scripts/local-release.sh");
    let script = fs::read_to_string(&script_path).unwrap();

    assert!(script.contains("scripts/release-pipeline.sh --bundle dmg --clean-required"));
    assert!(!script.contains("cargo build"));
    assert!(!script.contains("pnpm test"));
    assert!(!script.contains("\npnpm tauri build"));
    assert!(script.contains("src-tauri/target/release/bundle/dmg"));
}

#[test]
fn local_release_script_prepares_dist_assets_without_remote_release() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = fs::read_to_string(manifest_dir.join("../scripts/local-release.sh")).unwrap();

    assert!(script.contains("release_dir=\"dist-app\""));
    assert!(script.contains("rm -rf \"$release_dir\""));
    assert!(script.contains("mkdir -p \"$release_dir\""));
    assert!(script.contains(".sha256"));
    assert!(script.contains("release-notes-v"));
    assert!(script.contains("shasum -a 256"));
    assert!(script.contains("git check-ignore -q \"${release_dir}/\""));
    assert!(!script.contains("corepack"));
    assert!(!script.contains("mapfile"));
    assert!(!script.contains("readarray"));
    assert!(!script.contains("gh release create"));
    assert!(!script.contains(".github/workflows"));
}

#[test]
fn local_release_script_cleans_frontend_dist_before_copying_release_assets() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = fs::read_to_string(manifest_dir.join("../scripts/local-release.sh")).unwrap();

    let release_pipeline = script
        .find("scripts/release-pipeline.sh --bundle dmg --clean-required")
        .unwrap();
    let cleanup = script
        .find("==> 清理 dist-app 发布目录")
        .expect("local release script should clean dist-app after release pipeline completes");
    let copy_assets = script.find("==> 复制发布资产").unwrap();

    assert!(release_pipeline < cleanup);
    assert!(cleanup < copy_assets);
    assert!(script[cleanup..copy_assets].contains("rm -rf \"$release_dir\""));
    assert!(script[cleanup..copy_assets].contains("mkdir -p \"$release_dir\""));
}

#[test]
fn release_pipeline_uses_shared_release_identity_checks() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let pipeline = fs::read_to_string(manifest_dir.join("../scripts/release-pipeline.sh")).unwrap();

    let version_guard = pipeline.find("pnpm release:check-version").unwrap();
    let build_identity_env = pipeline
        .find("pnpm --silent release:build-identity-env")
        .unwrap();
    let cargo_build = pipeline.find("cargo build").unwrap();
    let pnpm_build = pipeline.find("pnpm build").unwrap();
    let tauri_build = pipeline.find("pnpm tauri build").unwrap();

    assert!(pipeline.contains(
        "export TOKEN_FIRE_GIT_COMMIT TOKEN_FIRE_GIT_COMMIT_SHORT TOKEN_FIRE_GIT_DIRTY TOKEN_FIRE_BUILD_TIME"
    ));
    assert!(version_guard < cargo_build);
    assert!(version_guard < pnpm_build);
    assert!(version_guard < tauri_build);
    assert!(build_identity_env < cargo_build);
    assert!(build_identity_env < pnpm_build);
    assert!(build_identity_env < tauri_build);
    assert!(pipeline.contains("\"${app_bin_path}\" --version-json"));
    assert!(pipeline.contains("\"${app_hook_path}\" --version-json"));
    assert!(pipeline.contains("node scripts/check-build-identity-output.mjs"));
    assert!(pipeline.contains("\"${TOKEN_FIRE_GIT_COMMIT}\""));
    assert!(!pipeline.contains("extract_json_string"));
    assert!(!pipeline.contains("extract_cargo_version"));
}

#[test]
fn release_pipeline_dmg_mode_checks_exactly_one_dmg_artifact() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let pipeline = fs::read_to_string(manifest_dir.join("../scripts/release-pipeline.sh")).unwrap();

    assert!(pipeline.contains("dmg_dir=\"src-tauri/target/release/bundle/dmg\""));
    assert!(pipeline.contains("find \"${dmg_dir}\" -maxdepth 1 -type f -name \"*.dmg\""));
    assert!(pipeline.contains("if [ \"${bundle}\" = \"dmg\" ]; then"));
    assert!(pipeline.contains("if [[ \"${#dmgs[@]}\" -eq 0 ]]; then"));
    assert!(pipeline.contains("if [[ \"${#dmgs[@]}\" -gt 1 ]]; then"));
}

#[test]
fn legacy_smoke_script_entrypoints_are_removed_from_docs_and_skills() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest_dir.parent().unwrap();
    let legacy_app_smoke = ["app-bundle", "-smoke.sh"].concat();
    let legacy_release_smoke = ["release", "-smoke.sh"].concat();
    let checked_paths = [
        "AGENTS.md",
        "README.md",
        "agent-docs/release-versioning.md",
        ".agents/skills/release-version-build/SKILL.md",
    ];

    assert!(!repo_root.join("scripts").join(&legacy_app_smoke).exists());
    assert!(!repo_root
        .join("scripts")
        .join(&legacy_release_smoke)
        .exists());

    for path in checked_paths {
        let body = fs::read_to_string(repo_root.join(path)).unwrap();
        assert!(
            !body.contains(&legacy_app_smoke),
            "{path} still references removed app smoke script"
        );
        assert!(
            !body.contains(&legacy_release_smoke),
            "{path} still references removed release smoke script"
        );
        assert!(
            body.contains("scripts/release-pipeline.sh"),
            "{path} should reference scripts/release-pipeline.sh"
        );
    }
}

#[test]
fn readme_documents_local_release_and_free_distribution_limits() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let readme = fs::read_to_string(manifest_dir.join("../README.md")).unwrap();

    assert!(readme.contains("本地发布"));
    assert!(readme.contains("scripts/local-release.sh"));
    assert!(readme.contains("dist-app/"));
    assert!(readme.contains("TokenFire Profile 截图"));
    assert!(readme.contains("Developer ID"));
    assert!(readme.contains("Apple notarization"));
    assert!(readme.contains("xattr -dr com.apple.quarantine /Applications/TokenFire.app"));
    assert!(!readme.contains("What It Shows"));
    assert!(!readme.contains("Repository Layout"));
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

#[test]
fn main_runtime_registers_release_update_commands_and_startup_check() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let main_rs = std::fs::read_to_string(manifest_dir.join("src/main.rs")).unwrap();

    assert!(main_rs.contains("release_update_status"));
    assert!(main_rs.contains("open_latest_release"));
    assert!(main_rs.contains("ReleaseUpdateStateStore::default()"));
    assert!(main_rs.contains("start_release_check_on_startup"));
    assert!(!main_rs.contains("tauri_plugin_updater"));
}

// Task 7: profile_summary command 必须异步且把 SQLite 工作放进 spawn_blocking，不在 command
// body 直接 open store（否则同步 SQLite 会跑在 async executor 主线程上，导致 UI freeze）。
#[test]
fn profile_summary_command_is_async_and_offloads_sqlite() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let main_rs = fs::read_to_string(manifest_dir.join("src/main.rs")).unwrap();
    let profile_query_rs =
        fs::read_to_string(manifest_dir.join("src/app/profile_query.rs")).unwrap();

    // command 是 async。
    assert!(
        main_rs.contains("async fn profile_summary"),
        "profile_summary 必须是 async command"
    );
    // blocking 查询在 spawn_blocking 中执行。
    assert!(
        profile_query_rs.contains("tauri::async_runtime::spawn_blocking"),
        "profile_query 必须用 spawn_blocking 卸载 SQLite"
    );
    // command body 不直接 open store（SQLite open 应在 spawn_blocking closure 内）。
    assert!(
        !main_rs.contains("UsageStore::open"),
        "main.rs command body 不得直接 UsageStore::open"
    );
    assert!(
        profile_query_rs.contains("UsageStore::open"),
        "UsageStore::open 应位于 spawn_blocking closure 内 (profile_query.rs)"
    );
}

// Task 7: 通过 async_runtime::block_on 驱动 blocking 查询函数，验证它对临时数据库返回可用 DTO。
#[test]
fn profile_summary_query_returns_dto_via_block_on() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("token-fire.sqlite");
    let now_utc = Utc.with_ymd_and_hms(2026, 7, 4, 12, 0, 0).unwrap();
    {
        let store = UsageStore::open(&db_path).unwrap();
        let window_id = store
            .open_tracking_window(now_utc - Duration::days(1))
            .unwrap();
        let mut row = token_fire::core::observation::NormalizedObservation {
            source: "traex".to_string(),
            adapter_version: "traex-jsonl-v1".to_string(),
            source_record_id: "async-profile-1".to_string(),
            source_record_id_confidence:
                token_fire::core::observation::SourceRecordIdConfidence::Exact,
            session_id: Some("s".to_string()),
            turn_id: Some("t".to_string()),
            turn_boundary_id: Some("t".to_string()),
            source_path: Some("/tmp/async-profile.jsonl".to_string()),
            line_no: Some(1),
            byte_offset: Some(10),
            input_tokens: 1_000_000,
            output_tokens: 0,
            cached_input_tokens: 0,
            cache_creation_input_tokens: 0,
            reasoning_output_tokens: 0,
            total_tokens: 1_000_000,
            cumulative_total_tokens: Some(1_000_000),
            model: Some("gpt-5.5".to_string()),
            cwd: Some("~/p".to_string()),
            observed_at: now_utc - Duration::hours(2),
            token_payload_hash: "hash-async-1".to_string(),
        };
        row.total_tokens = 1_000_000;
        store
            .insert_observation_for_tracking_window(&row, window_id)
            .unwrap();
    }

    let outcome =
        tauri::async_runtime::block_on(token_fire::app::profile_query::query_profile_summary(
            db_path,
            token_fire::core::profile::ProfilePeriod::Today,
            now_utc,
        ))
        .unwrap();

    assert_eq!(
        outcome.summary.selected_period.period,
        token_fire::core::profile::ProfilePeriod::Today
    );
    assert_eq!(outcome.summary.year_profile.days.len(), 365);
    assert_eq!(outcome.summary.selected_period.total_tokens, 1_000_000);
}
