use std::fs;
use std::path::PathBuf;

use chrono::{Local, TimeZone, Utc};
use serde_json::json;
use tempfile::tempdir;
use token_fire::adapters::claude::hook_config::ClaudeHookConfigManager;
use token_fire::adapters::codex::hook_config::CodexHookConfigManager;
use token_fire::adapters::codex::status::CodexStatusSource;
use token_fire::adapters::cursor::hook_config::CursorHookConfigManager;
use token_fire::adapters::source::{SourceHookStatus, SourcePaths, SourceStatus, TokenSourceKind};
use token_fire::adapters::traex::hook_config::HookConfigManager;
use token_fire::adapters::traex::resolver::TraexPaths;
use token_fire::adapters::traex::status::{collect_traex_status, TraexStatus, TraexStatusSource};
use token_fire::app::debug_bundle::debug_bundle_sources_summary;
use token_fire::app::logging::write_jsonl_event;
use token_fire::app::menu::menu_labels;
use token_fire::app::paths::RuntimePaths;
use token_fire::app::source_diagnostics::{
    headline_for_chain, primary_break_for_chain, DiagnosticHeadline, DiagnosticStage,
    DiagnosticStageKey, DiagnosticStatus, SourceDiagnostic,
};
use token_fire::app::source_ingest::{SourceEmptyReason, SourceIngestEvent, SourceResolution};
use token_fire::app::source_signals::SourceSignalRecord;
use token_fire::app::state::{AppState, MenuAction, MenuActionOutcome};
use token_fire::app::status::{status_label, ui_status, ui_status_from_sources, UiStatus};
use token_fire::core::observation::{NormalizedObservation, SourceRecordIdConfidence};
use token_fire::core::pricing::PricingStatus;
use token_fire::core::usage_store::UsageStore;

fn diagnostic_stage(key: DiagnosticStageKey, status: DiagnosticStatus) -> DiagnosticStage {
    DiagnosticStage {
        key,
        label: key.label().to_string(),
        status,
        summary: status.summary_label().to_string(),
        evidence: None,
        checked_at: None,
    }
}

fn diagnostic_evidence_value<'a>(
    source: &'a SourceDiagnostic,
    group_title: &str,
    label: &str,
) -> Option<&'a str> {
    source
        .evidence
        .iter()
        .find(|group| group.title == group_title)
        .and_then(|group| group.items.iter().find(|item| item.label == label))
        .map(|item| item.value.as_str())
}

#[test]
fn diagnostics_headline_comes_from_first_meaningful_break() {
    let chain = vec![
        diagnostic_stage(DiagnosticStageKey::Participation, DiagnosticStatus::Ok),
        diagnostic_stage(DiagnosticStageKey::Capture, DiagnosticStatus::Ok),
        diagnostic_stage(DiagnosticStageKey::Signal, DiagnosticStatus::Ok),
        diagnostic_stage(DiagnosticStageKey::Extraction, DiagnosticStatus::Warning),
        diagnostic_stage(DiagnosticStageKey::Storage, DiagnosticStatus::Unknown),
    ];

    assert_eq!(
        headline_for_chain(&chain),
        DiagnosticHeadline::TokenNotExtracted
    );
    let primary_break = primary_break_for_chain(&chain).unwrap();
    assert_eq!(primary_break.stage, DiagnosticStageKey::Extraction);
    assert_eq!(primary_break.title, "未提取到 token");
}

#[test]
fn diagnostics_contract_serializes_snake_case_for_frontend() {
    let stage = diagnostic_stage(DiagnosticStageKey::Storage, DiagnosticStatus::NotApplicable);
    let value = serde_json::to_value(stage).unwrap();

    assert_eq!(value["key"], "storage");
    assert_eq!(value["label"], "写入统计");
    assert_eq!(value["status"], "not_applicable");
}

#[test]
fn token_source_kind_has_menu_order_and_display_names() {
    assert_eq!(
        TokenSourceKind::all_menu_sources(),
        [
            TokenSourceKind::Traex,
            TokenSourceKind::Codex,
            TokenSourceKind::Claude,
            TokenSourceKind::Cursor,
        ]
    );
    assert_eq!(TokenSourceKind::Traex.display_name(), "TraeX");
    assert_eq!(TokenSourceKind::Codex.display_name(), "Codex");
    assert_eq!(TokenSourceKind::Claude.display_name(), "Claude");
    assert_eq!(TokenSourceKind::Cursor.display_name(), "Cursor");
}

#[test]
fn source_hook_statuses_read_registration_without_health_semantics() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path().join("token-fire").as_path());
    let hook_path = dir
        .path()
        .join("TokenFire.app/Contents/MacOS/token-fire-hook");
    fs::create_dir_all(hook_path.parent().unwrap()).unwrap();
    fs::write(&hook_path, b"#!/bin/sh\n").unwrap();
    make_executable(&hook_path);

    let managers = token_fire::app::state::SourceHookManagers::new(
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
    managers.claude().install(&hook_path).unwrap();
    let app_state = AppState::new_with_source_hook_managers(paths, managers);
    let statuses = app_state.source_hook_statuses();

    assert!(statuses
        .iter()
        .any(|status| status.source == TokenSourceKind::Claude && status.hook_registered));
    assert_eq!(
        statuses
            .iter()
            .filter(|status| status.hook_registered)
            .map(|status| status.source)
            .collect::<Vec<_>>(),
        [TokenSourceKind::Claude]
    );
}

#[test]
fn source_hook_statuses_report_malformed_config_errors() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path().join("token-fire").as_path());
    let claude_config = dir.path().join("claude-settings.json");
    fs::write(&claude_config, r#"{"hooks": "#).unwrap();

    let managers = token_fire::app::state::SourceHookManagers::new(
        HookConfigManager::new(dir.path().join("traecli.toml"), paths.backups_dir.clone()),
        CodexHookConfigManager::new(
            dir.path().join("codex-hooks.json"),
            paths.backups_dir.clone(),
        ),
        ClaudeHookConfigManager::new(claude_config, paths.backups_dir.clone()),
        CursorHookConfigManager::new(
            dir.path().join("cursor-hooks.json"),
            paths.backups_dir.clone(),
        ),
    );
    let app_state = AppState::new_with_source_hook_managers(paths, managers);
    let statuses = app_state.source_hook_statuses();
    let claude = statuses
        .iter()
        .find(|status| status.source == TokenSourceKind::Claude)
        .unwrap();

    assert!(!claude.hook_registered);
    assert!(!claude.hook_executable_exists);
    assert!(claude.config_detected);
    assert!(claude.config_error.is_some());
}

#[test]
fn app_state_toggles_claude_hook_without_touching_other_sources() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path().join("token-fire").as_path());
    let hook_path = dir
        .path()
        .join("TokenFire.app/Contents/MacOS/token-fire-hook");
    fs::create_dir_all(hook_path.parent().unwrap()).unwrap();
    fs::write(&hook_path, b"#!/bin/sh\n").unwrap();
    make_executable(&hook_path);

    let managers = token_fire::app::state::SourceHookManagers::new(
        HookConfigManager::new(dir.path().join("traecli.toml"), paths.backups_dir.clone()),
        token_fire::adapters::codex::hook_config::CodexHookConfigManager::new(
            dir.path().join("codex-hooks.json"),
            paths.backups_dir.clone(),
        ),
        token_fire::adapters::claude::hook_config::ClaudeHookConfigManager::new(
            dir.path().join("claude-settings.json"),
            paths.backups_dir.clone(),
        ),
        token_fire::adapters::cursor::hook_config::CursorHookConfigManager::new(
            dir.path().join("cursor-hooks.json"),
            paths.backups_dir.clone(),
        ),
    );
    let app_state = AppState::new_with_source_hook_managers(paths, managers);

    app_state
        .handle_menu_action(token_fire::app::state::MenuAction::ToggleSourceHook(
            TokenSourceKind::Claude,
        ))
        .unwrap();

    let statuses = app_state.source_hook_statuses();
    assert!(statuses
        .iter()
        .any(|status| status.source == TokenSourceKind::Claude && status.hook_registered));
    assert!(fs::read_to_string(dir.path().join("claude-settings.json"))
        .unwrap()
        .contains("--source claude"));
    assert!(!dir.path().join("cursor-hooks.json").exists());

    app_state
        .handle_menu_action(token_fire::app::state::MenuAction::ToggleSourceHook(
            TokenSourceKind::Claude,
        ))
        .unwrap();

    let body = fs::read_to_string(dir.path().join("claude-settings.json")).unwrap();
    assert!(!body.contains("--source claude"));
}

#[test]
fn app_state_uninstalling_cursor_does_not_remove_claude_hook() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path().join("token-fire").as_path());
    let hook_path = dir
        .path()
        .join("TokenFire.app/Contents/MacOS/token-fire-hook");
    fs::create_dir_all(hook_path.parent().unwrap()).unwrap();
    fs::write(&hook_path, b"#!/bin/sh\n").unwrap();
    make_executable(&hook_path);

    let managers = token_fire::app::state::SourceHookManagers::new(
        HookConfigManager::new(dir.path().join("traecli.toml"), paths.backups_dir.clone()),
        token_fire::adapters::codex::hook_config::CodexHookConfigManager::new(
            dir.path().join("codex-hooks.json"),
            paths.backups_dir.clone(),
        ),
        token_fire::adapters::claude::hook_config::ClaudeHookConfigManager::new(
            dir.path().join("claude-settings.json"),
            paths.backups_dir.clone(),
        ),
        token_fire::adapters::cursor::hook_config::CursorHookConfigManager::new(
            dir.path().join("cursor-hooks.json"),
            paths.backups_dir.clone(),
        ),
    );
    managers.claude().install(&hook_path).unwrap();
    managers.cursor().install(&hook_path).unwrap();
    let app_state = AppState::new_with_source_hook_managers(paths, managers);

    app_state
        .handle_menu_action(token_fire::app::state::MenuAction::ToggleSourceHook(
            TokenSourceKind::Cursor,
        ))
        .unwrap();

    let claude_body = fs::read_to_string(dir.path().join("claude-settings.json")).unwrap();
    let cursor_body = fs::read_to_string(dir.path().join("cursor-hooks.json")).unwrap();
    assert!(claude_body.contains("--source claude"));
    assert!(!cursor_body.contains("--source cursor"));
}

#[test]
fn menu_labels_are_chinese_with_source_submenu() {
    let labels = menu_labels();

    assert_eq!(labels.source_submenu, "来源");
    assert_eq!(labels.traex_source, "TraeX");
    assert_eq!(labels.codex_source, "Codex");
    assert_eq!(labels.claude_source, "Claude");
    assert_eq!(labels.cursor_source, "Cursor");
    assert_eq!(labels.pause_tracking, "暂停统计");
    assert_eq!(labels.resume_tracking, "继续统计");
    assert_eq!(labels.open_logs, "打开日志目录");
    assert_eq!(labels.copy_debug_bundle, "复制诊断包");
    assert_eq!(labels.enable_debug_logging, "开启调试日志");
    assert_eq!(labels.quit, "退出");
}

#[test]
fn menu_labels_include_source_diagnostics_entry() {
    let labels = menu_labels();

    assert_eq!(labels.source_diagnostics, "接入诊断...");
}

#[test]
fn menu_action_from_id_maps_source_diagnostics() {
    assert_eq!(
        token_fire::app::tray::menu_action_from_id("open_source_diagnostics"),
        Some(token_fire::app::state::MenuAction::OpenSourceDiagnostics)
    );
}

#[test]
fn source_diagnostics_action_maps_only_safe_actions() {
    assert_eq!(
        token_fire::app::source_diagnostics::menu_action_for_diagnostic_action("open_logs"),
        Some(MenuAction::OpenLogs)
    );
    assert_eq!(
        token_fire::app::source_diagnostics::menu_action_for_diagnostic_action("copy_debug_bundle"),
        Some(MenuAction::CopyDebugBundle)
    );
    assert_eq!(
        token_fire::app::source_diagnostics::menu_action_for_diagnostic_action("refresh"),
        None
    );
    assert_eq!(
        token_fire::app::source_diagnostics::menu_action_for_diagnostic_action("reinstall_hook"),
        None
    );
}

#[test]
fn source_diagnostics_action_passes_native_outcomes_to_handler() {
    let cases = [
        (
            "open_logs",
            MenuAction::OpenLogs,
            MenuActionOutcome::LogsDirectoryRequested(PathBuf::from("/tmp/token-fire-logs")),
        ),
        (
            "copy_debug_bundle",
            MenuAction::CopyDebugBundle,
            MenuActionOutcome::DebugBundleCreated(PathBuf::from("/tmp/token-fire-debug.zip")),
        ),
    ];

    for (action_id, expected_action, expected_outcome) in cases {
        let mut received = None;

        token_fire::app::source_diagnostics::handle_diagnostic_menu_action(
            action_id,
            |action| {
                assert_eq!(action, expected_action);
                Ok(expected_outcome.clone())
            },
            |outcome| {
                received = Some(outcome);
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(received, Some(expected_outcome));
    }
}

#[test]
fn tray_menu_contains_source_diagnostics_before_logs_actions() {
    let labels = menu_labels();
    let html_order = [
        labels.source_diagnostics,
        labels.open_logs,
        labels.copy_debug_bundle,
    ];

    assert_eq!(html_order[0], "接入诊断...");
}

#[test]
fn source_diagnostics_window_focused_event_contract() {
    assert_eq!(
        token_fire::app::tray::SOURCE_DIAGNOSTICS_WINDOW_FOCUSED_EVENT,
        "source_diagnostics_window_focused"
    );
}

#[test]
fn menu_action_from_id_maps_source_toggles() {
    assert_eq!(
        token_fire::app::tray::menu_action_from_id("toggle_source_traex_hook"),
        Some(token_fire::app::state::MenuAction::ToggleSourceHook(
            TokenSourceKind::Traex
        ))
    );
    assert_eq!(
        token_fire::app::tray::menu_action_from_id("toggle_source_codex_hook"),
        Some(token_fire::app::state::MenuAction::ToggleSourceHook(
            TokenSourceKind::Codex
        ))
    );
    assert_eq!(
        token_fire::app::tray::menu_action_from_id("toggle_source_claude_hook"),
        Some(token_fire::app::state::MenuAction::ToggleSourceHook(
            TokenSourceKind::Claude
        ))
    );
    assert_eq!(
        token_fire::app::tray::menu_action_from_id("toggle_source_cursor_hook"),
        Some(token_fire::app::state::MenuAction::ToggleSourceHook(
            TokenSourceKind::Cursor
        ))
    );
    assert_eq!(
        token_fire::app::tray::menu_action_from_id("install_traex_hook"),
        None
    );
    assert_eq!(
        token_fire::app::tray::menu_action_from_id("uninstall_codex_hook"),
        None
    );
}

#[test]
fn source_menu_models_are_ordered_and_checked_from_statuses() {
    let statuses = vec![
        SourceHookStatus {
            source: TokenSourceKind::Traex,
            hook_registered: true,
            hook_executable_exists: true,
            config_detected: true,
            config_error: None,
        },
        SourceHookStatus {
            source: TokenSourceKind::Claude,
            hook_registered: false,
            hook_executable_exists: false,
            config_detected: false,
            config_error: None,
        },
    ];

    let models = token_fire::app::tray::source_menu_item_models(&statuses);

    assert_eq!(
        models.iter().map(|model| model.label).collect::<Vec<_>>(),
        ["TraeX", "Codex", "Claude", "Cursor"]
    );
    assert_eq!(models[0].id, "toggle_source_traex_hook");
    assert!(models[0].checked);
    assert!(!models[2].checked);
}

#[test]
fn source_toggle_refreshes_tray_menu_after_success_or_failure() {
    use token_fire::app::state::MenuAction;
    use token_fire::app::tray::{should_refresh_tray_menu_after_action, MenuActionRefreshTrigger};

    let action = MenuAction::ToggleSourceHook(TokenSourceKind::Claude);

    assert!(should_refresh_tray_menu_after_action(
        action,
        MenuActionRefreshTrigger::ActionHandled
    ));
    assert!(should_refresh_tray_menu_after_action(
        action,
        MenuActionRefreshTrigger::ActionFailed
    ));
    assert!(!should_refresh_tray_menu_after_action(
        MenuAction::PauseTracking,
        MenuActionRefreshTrigger::ActionHandled
    ));
    assert!(!should_refresh_tray_menu_after_action(
        MenuAction::PauseTracking,
        MenuActionRefreshTrigger::ActionFailed
    ));
}

#[test]
fn status_aggregates_green_yellow_and_red_cases() {
    let healthy = TraexStatus {
        hook_installed: true,
        hook_executable_exists: true,
        hook_smoke_test_passed: true,
        sessions_readable: true,
        archived_sessions_readable: true,
        ..TraexStatus::default()
    };
    assert_eq!(ui_status(&healthy, true, true, true), UiStatus::Green);
    assert_eq!(status_label(UiStatus::Green), "正常");

    let missing_hook = TraexStatus {
        hook_installed: false,
        sessions_readable: true,
        archived_sessions_readable: true,
        ..TraexStatus::default()
    };
    assert_eq!(ui_status(&missing_hook, true, true, true), UiStatus::Yellow);
    assert_eq!(status_label(UiStatus::Yellow), "需处理");

    let unreadable = TraexStatus {
        sessions_readable: false,
        archived_sessions_readable: true,
        ..TraexStatus::default()
    };
    assert_eq!(ui_status(&unreadable, true, true, true), UiStatus::Yellow);
    assert_eq!(status_label(UiStatus::Yellow), "需处理");

    let archived_unreadable = TraexStatus {
        sessions_readable: true,
        archived_sessions_readable: false,
        ..TraexStatus::default()
    };
    assert_eq!(
        ui_status(&archived_unreadable, true, true, true),
        UiStatus::Yellow
    );
    assert_eq!(ui_status(&healthy, true, false, true), UiStatus::Red);
}

#[test]
fn source_status_folding_ignores_inactive_codex() {
    let traex = SourceStatus {
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
    };
    let codex_absent = SourceStatus {
        source: TokenSourceKind::Codex,
        enabled: false,
        detected: false,
        hook_installed: false,
        hook_executable_exists: false,
        hook_smoke_test_passed: false,
        sessions_readable: false,
        archived_sessions_readable: false,
        last_hook_seen_at: None,
        last_hook_error: None,
    };

    assert_eq!(
        ui_status_from_sources(&[traex, codex_absent], true, true, true),
        UiStatus::Green
    );
}

#[test]
fn optional_claude_and_cursor_without_hooks_do_not_degrade_status() {
    let traex = SourceStatus {
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
    };
    let claude = SourceStatus {
        source: TokenSourceKind::Claude,
        enabled: false,
        detected: false,
        hook_installed: false,
        hook_executable_exists: false,
        hook_smoke_test_passed: true,
        sessions_readable: true,
        archived_sessions_readable: true,
        last_hook_seen_at: None,
        last_hook_error: None,
    };
    let cursor = SourceStatus {
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
    };

    assert_eq!(
        ui_status_from_sources(&[traex, claude, cursor], true, true, true),
        UiStatus::Green
    );
}

#[test]
fn registered_optional_source_with_missing_executable_is_diagnosed() {
    let source = SourceStatus {
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
    };

    let bundle = debug_bundle_sources_summary(&[source]);
    assert_eq!(bundle[0].source, "claude");
    assert_eq!(bundle[0].hook_install_status, "installed");
    assert_eq!(bundle[0].hook_executable_status, "missing");
    assert!(bundle[0]
        .last_hook_error
        .as_deref()
        .is_some_and(|message| message.contains("missing")));
}

#[test]
fn enabled_codex_unreadable_is_yellow_not_red() {
    let codex = SourceStatus {
        source: TokenSourceKind::Codex,
        enabled: true,
        detected: true,
        hook_installed: false,
        hook_executable_exists: false,
        hook_smoke_test_passed: false,
        sessions_readable: false,
        archived_sessions_readable: true,
        last_hook_seen_at: None,
        last_hook_error: None,
    };

    assert_eq!(
        ui_status_from_sources(&[codex], true, true, true),
        UiStatus::Yellow
    );
}

#[test]
fn profile_source_labels_include_cursor() {
    assert_eq!(token_fire::core::profile::source_label("claude"), "Claude");
    assert_eq!(token_fire::core::profile::source_label("cursor"), "Cursor");
}

#[test]
fn source_diagnostics_marks_optional_disabled_sources_neutral() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path().join("token-fire").as_path());
    let managers = token_fire::app::state::SourceHookManagers::new(
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
    let app_state = AppState::new_with_source_hook_managers(paths, managers);

    let snapshot = app_state
        .source_diagnostics_snapshot_at(Utc.with_ymd_and_hms(2026, 7, 10, 10, 0, 0).unwrap())
        .unwrap();
    let claude = snapshot
        .sources
        .iter()
        .find(|source| source.source == TokenSourceKind::Claude)
        .unwrap();

    assert_eq!(claude.headline, DiagnosticHeadline::Disabled);
    assert!(claude.optional);
    assert_eq!(snapshot.summary.disabled, 2);
}

#[test]
fn source_diagnostics_marks_cursor_empty_collect_as_token_not_extracted() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path().join("token-fire").as_path());
    let app_state = AppState::new(paths);
    app_state
        .recent_source_signals()
        .record(SourceSignalRecord {
            source: TokenSourceKind::Cursor,
            event: SourceIngestEvent::Hook,
            resolution: SourceResolution::TranscriptPath,
            seen_at: Utc.with_ymd_and_hms(2026, 7, 10, 10, 0, 0).unwrap(),
            inserted: Some(0),
            duplicates: Some(0),
            skipped_outside_tracking: Some(0),
            empty_reason: Some(SourceEmptyReason::NoNewCompleteRound),
            error_kind: None,
        });

    let snapshot = app_state
        .source_diagnostics_snapshot_at(Utc.with_ymd_and_hms(2026, 7, 10, 10, 1, 0).unwrap())
        .unwrap();
    let cursor = snapshot
        .sources
        .iter()
        .find(|source| source.source == TokenSourceKind::Cursor)
        .unwrap();

    assert_eq!(cursor.headline, DiagnosticHeadline::TokenNotExtracted);
    assert_eq!(
        cursor.primary_break.as_ref().unwrap().stage,
        DiagnosticStageKey::Extraction
    );
    assert!(cursor.chain.iter().any(|stage| {
        stage.key == DiagnosticStageKey::Signal && stage.status == DiagnosticStatus::Ok
    }));
    assert!(cursor.chain.iter().any(|stage| {
        stage.key == DiagnosticStageKey::Extraction && stage.status == DiagnosticStatus::Warning
    }));
}

#[test]
fn source_diagnostics_marks_failed_signal_as_not_connected() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path().join("token-fire").as_path());
    let app_state = AppState::new(paths);
    app_state
        .recent_source_signals()
        .record(SourceSignalRecord {
            source: TokenSourceKind::Cursor,
            event: SourceIngestEvent::Hook,
            resolution: SourceResolution::None,
            seen_at: Utc.with_ymd_and_hms(2026, 7, 10, 10, 0, 0).unwrap(),
            inserted: None,
            duplicates: None,
            skipped_outside_tracking: None,
            empty_reason: None,
            error_kind: Some("source_adapter_failed".to_string()),
        });

    let snapshot = app_state
        .source_diagnostics_snapshot_at(Utc.with_ymd_and_hms(2026, 7, 10, 10, 1, 0).unwrap())
        .unwrap();
    let cursor = snapshot
        .sources
        .iter()
        .find(|source| source.source == TokenSourceKind::Cursor)
        .unwrap();

    assert!(matches!(
        cursor.headline,
        DiagnosticHeadline::TokenNotExtracted | DiagnosticHeadline::RuntimeError
    ));
    assert!(cursor.primary_break.is_some());
    assert!(cursor.chain.iter().any(|stage| {
        matches!(
            stage.key,
            DiagnosticStageKey::Extraction | DiagnosticStageKey::Storage
        ) && matches!(
            stage.status,
            DiagnosticStatus::Error | DiagnosticStatus::Warning
        )
    }));
}

#[test]
fn source_diagnostics_marks_zero_write_signal_as_not_connected() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path().join("token-fire").as_path());
    let app_state = AppState::new(paths);
    app_state
        .recent_source_signals()
        .record(SourceSignalRecord {
            source: TokenSourceKind::Cursor,
            event: SourceIngestEvent::Hook,
            resolution: SourceResolution::TranscriptPath,
            seen_at: Utc.with_ymd_and_hms(2026, 7, 10, 10, 0, 0).unwrap(),
            inserted: Some(0),
            duplicates: Some(0),
            skipped_outside_tracking: Some(0),
            empty_reason: None,
            error_kind: None,
        });

    let snapshot = app_state
        .source_diagnostics_snapshot_at(Utc.with_ymd_and_hms(2026, 7, 10, 10, 1, 0).unwrap())
        .unwrap();
    let cursor = snapshot
        .sources
        .iter()
        .find(|source| source.source == TokenSourceKind::Cursor)
        .unwrap();

    assert_ne!(cursor.headline, DiagnosticHeadline::Connected);
    assert!(cursor.primary_break.is_some());
    assert!(cursor.chain.iter().any(|stage| {
        matches!(
            stage.key,
            DiagnosticStageKey::Extraction | DiagnosticStageKey::Storage
        ) && matches!(
            stage.status,
            DiagnosticStatus::Warning | DiagnosticStatus::Unknown
        )
    }));
}

#[test]
fn source_diagnostics_keeps_success_trusted_after_signal_freshness_expires() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path().join("token-fire").as_path());
    let app_state = AppState::new(paths);
    app_state
        .recent_source_signals()
        .record(SourceSignalRecord {
            source: TokenSourceKind::Traex,
            event: SourceIngestEvent::Hook,
            resolution: SourceResolution::TranscriptPath,
            seen_at: Utc.with_ymd_and_hms(2026, 7, 10, 9, 0, 0).unwrap(),
            inserted: Some(3),
            duplicates: Some(0),
            skipped_outside_tracking: Some(0),
            empty_reason: None,
            error_kind: None,
        });

    let snapshot = app_state
        .source_diagnostics_snapshot_at(Utc.with_ymd_and_hms(2026, 7, 10, 10, 0, 0).unwrap())
        .unwrap();
    let traex = snapshot
        .sources
        .iter()
        .find(|source| source.source == TokenSourceKind::Traex)
        .unwrap();
    let signal = traex
        .chain
        .iter()
        .find(|stage| stage.key == DiagnosticStageKey::Signal)
        .unwrap();
    let storage = traex
        .chain
        .iter()
        .find(|stage| stage.key == DiagnosticStageKey::Storage)
        .unwrap();

    assert_eq!(traex.headline, DiagnosticHeadline::Connected);
    assert_eq!(signal.status, DiagnosticStatus::Ok);
    assert_eq!(storage.status, DiagnosticStatus::Ok);
    assert!(diagnostic_evidence_value(traex, "最近采集", "最近捕获").is_some());
    assert_eq!(
        diagnostic_evidence_value(traex, "数据库证据", "可信状态"),
        Some("可信")
    );
}

#[test]
fn source_diagnostics_keeps_optional_source_enabled_after_success_signal_expires() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path().join("token-fire").as_path());
    let managers = token_fire::app::state::SourceHookManagers::new(
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
    let app_state = AppState::new_with_source_hook_managers(paths, managers);
    app_state
        .recent_source_signals()
        .record(SourceSignalRecord {
            source: TokenSourceKind::Cursor,
            event: SourceIngestEvent::Hook,
            resolution: SourceResolution::TranscriptPath,
            seen_at: Utc.with_ymd_and_hms(2026, 7, 10, 9, 0, 0).unwrap(),
            inserted: Some(5),
            duplicates: Some(0),
            skipped_outside_tracking: Some(0),
            empty_reason: None,
            error_kind: None,
        });

    let snapshot = app_state
        .source_diagnostics_snapshot_at(Utc.with_ymd_and_hms(2026, 7, 10, 10, 0, 0).unwrap())
        .unwrap();
    let cursor = snapshot
        .sources
        .iter()
        .find(|source| source.source == TokenSourceKind::Cursor)
        .unwrap();

    assert_eq!(cursor.headline, DiagnosticHeadline::Connected);
    assert!(cursor
        .chain
        .iter()
        .all(|stage| stage.status == DiagnosticStatus::Ok));
    assert_eq!(
        diagnostic_evidence_value(cursor, "数据库证据", "可信状态"),
        Some("可信")
    );
}

#[test]
fn source_diagnostics_does_not_use_old_storage_to_prove_new_empty_signal() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path().join("token-fire").as_path());
    let store = UsageStore::open(&paths.database).unwrap();
    let old_observed_at = Utc.with_ymd_and_hms(2026, 7, 9, 9, 0, 0).unwrap();
    let old_row = normalized_observation(
        "old-cursor-row",
        "cursor",
        Some("cursor-model"),
        100,
        old_observed_at,
    );
    let window_id = store
        .open_tracking_window(old_observed_at - chrono::Duration::minutes(1))
        .unwrap();
    store
        .insert_observation_for_tracking_window(&old_row, window_id)
        .unwrap();

    let app_state = AppState::new(paths);
    app_state
        .recent_source_signals()
        .record(SourceSignalRecord {
            source: TokenSourceKind::Cursor,
            event: SourceIngestEvent::Hook,
            resolution: SourceResolution::TranscriptPath,
            seen_at: Utc.with_ymd_and_hms(2026, 7, 10, 10, 0, 0).unwrap(),
            inserted: Some(0),
            duplicates: Some(0),
            skipped_outside_tracking: Some(0),
            empty_reason: Some(SourceEmptyReason::NoNewCompleteRound),
            error_kind: None,
        });

    let snapshot = app_state
        .source_diagnostics_snapshot_at(Utc.with_ymd_and_hms(2026, 7, 10, 10, 1, 0).unwrap())
        .unwrap();
    let cursor = snapshot
        .sources
        .iter()
        .find(|source| source.source == TokenSourceKind::Cursor)
        .unwrap();
    let storage = cursor
        .chain
        .iter()
        .find(|stage| stage.key == DiagnosticStageKey::Storage)
        .unwrap();

    assert_ne!(storage.status, DiagnosticStatus::Ok);
}

#[test]
fn source_diagnostics_keeps_recent_success_when_followed_by_noop_signal() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path().join("token-fire").as_path());
    let store = UsageStore::open(&paths.database).unwrap();
    let observed_at = Utc.with_ymd_and_hms(2026, 7, 10, 10, 0, 0).unwrap();
    let row = normalized_observation(
        "codex-success-row",
        "codex",
        Some("codex-model"),
        100,
        observed_at,
    );
    let window_id = store
        .open_tracking_window(observed_at - chrono::Duration::minutes(1))
        .unwrap();
    store
        .insert_observation_for_tracking_window(&row, window_id)
        .unwrap();

    let app_state = AppState::new(paths);
    app_state
        .recent_source_signals()
        .record(SourceSignalRecord {
            source: TokenSourceKind::Codex,
            event: SourceIngestEvent::Hook,
            resolution: SourceResolution::TranscriptPath,
            seen_at: observed_at,
            inserted: Some(1),
            duplicates: Some(0),
            skipped_outside_tracking: Some(0),
            empty_reason: None,
            error_kind: None,
        });
    app_state
        .recent_source_signals()
        .record(SourceSignalRecord {
            source: TokenSourceKind::Codex,
            event: SourceIngestEvent::Hook,
            resolution: SourceResolution::TranscriptPath,
            seen_at: observed_at + chrono::Duration::minutes(2),
            inserted: Some(0),
            duplicates: Some(0),
            skipped_outside_tracking: Some(0),
            empty_reason: Some(SourceEmptyReason::NoNewCompleteRound),
            error_kind: None,
        });

    let snapshot = app_state
        .source_diagnostics_snapshot_at(observed_at + chrono::Duration::minutes(3))
        .unwrap();
    let codex = snapshot
        .sources
        .iter()
        .find(|source| source.source == TokenSourceKind::Codex)
        .unwrap();

    assert_eq!(codex.headline, DiagnosticHeadline::Connected);
    assert!(codex.primary_break.is_none());
    assert!(codex.chain.iter().any(|stage| {
        stage.key == DiagnosticStageKey::Storage && stage.status == DiagnosticStatus::Ok
    }));
    assert!(diagnostic_evidence_value(codex, "数据库证据", "数据库最近写入").is_some());
}

#[test]
fn source_diagnostics_shows_latest_noop_without_losing_recent_success() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path().join("token-fire").as_path());
    let store = UsageStore::open(&paths.database).unwrap();
    let observed_at = Utc.with_ymd_and_hms(2026, 7, 10, 10, 0, 0).unwrap();
    let row = normalized_observation(
        "codex-success-row",
        "codex",
        Some("codex-model"),
        100,
        observed_at,
    );
    let window_id = store
        .open_tracking_window(observed_at - chrono::Duration::minutes(1))
        .unwrap();
    store
        .insert_observation_for_tracking_window(&row, window_id)
        .unwrap();

    let app_state = AppState::new(paths);
    app_state
        .recent_source_signals()
        .record(SourceSignalRecord {
            source: TokenSourceKind::Codex,
            event: SourceIngestEvent::Hook,
            resolution: SourceResolution::TranscriptPath,
            seen_at: observed_at,
            inserted: Some(1),
            duplicates: Some(0),
            skipped_outside_tracking: Some(0),
            empty_reason: None,
            error_kind: None,
        });
    app_state
        .recent_source_signals()
        .record(SourceSignalRecord {
            source: TokenSourceKind::Codex,
            event: SourceIngestEvent::Hook,
            resolution: SourceResolution::TranscriptPath,
            seen_at: observed_at + chrono::Duration::minutes(2),
            inserted: Some(0),
            duplicates: Some(0),
            skipped_outside_tracking: Some(0),
            empty_reason: Some(SourceEmptyReason::NoNewCompleteRound),
            error_kind: None,
        });

    let snapshot = app_state
        .source_diagnostics_snapshot_at(observed_at + chrono::Duration::minutes(3))
        .unwrap();
    let codex = snapshot
        .sources
        .iter()
        .find(|source| source.source == TokenSourceKind::Codex)
        .unwrap();

    assert_eq!(codex.headline, DiagnosticHeadline::Connected);
    assert!(codex.trust_summary.contains("最新检查无新增"));
    assert_eq!(
        diagnostic_evidence_value(codex, "最近采集", "检查结果"),
        Some("无新增完整轮次")
    );
    assert_eq!(
        codex.display_summary.note_text.as_deref(),
        Some("最新检查无新增完整轮次")
    );
    assert_eq!(
        diagnostic_evidence_value(codex, "数据库证据", "可信状态"),
        Some("可信")
    );
}

#[test]
fn source_diagnostics_exposes_user_semantic_evidence_contract() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path().join("token-fire").as_path());
    let app_state = AppState::new(paths);
    let captured_at = Utc.with_ymd_and_hms(2026, 7, 10, 10, 0, 0).unwrap();
    app_state
        .recent_source_signals()
        .record(SourceSignalRecord {
            source: TokenSourceKind::Codex,
            event: SourceIngestEvent::Hook,
            resolution: SourceResolution::TranscriptPath,
            seen_at: captured_at,
            inserted: Some(1),
            duplicates: Some(0),
            skipped_outside_tracking: Some(895),
            empty_reason: None,
            error_kind: None,
        });

    let snapshot = app_state
        .source_diagnostics_snapshot_at(captured_at + chrono::Duration::minutes(3))
        .unwrap();
    let codex = snapshot
        .sources
        .iter()
        .find(|source| source.source == TokenSourceKind::Codex)
        .unwrap();

    assert_eq!(codex.display_summary.status_text, "已接入");
    assert!(codex
        .display_summary
        .detail_text
        .contains("最近成功写入 1 条"));
    assert!(codex
        .display_summary
        .note_text
        .as_deref()
        .is_some_and(|note| note.contains("895 条窗口外历史记录")));
    assert_eq!(
        codex
            .evidence
            .iter()
            .map(|group| group.title.as_str())
            .collect::<Vec<_>>(),
        vec!["当前判断", "接入状态", "最近采集", "数据库证据", "最新问题"]
    );
    let expected_capture_time = captured_at
        .with_timezone(&Local)
        .format("%H:%M")
        .to_string();
    assert_eq!(
        diagnostic_evidence_value(codex, "最近采集", "最近捕获"),
        Some(expected_capture_time.as_str())
    );
    assert_eq!(
        diagnostic_evidence_value(codex, "最近采集", "本次写入"),
        Some("1 条")
    );
    assert_eq!(
        diagnostic_evidence_value(codex, "最近采集", "重复记录"),
        Some("0 条")
    );
    assert_eq!(
        diagnostic_evidence_value(codex, "最近采集", "窗口外记录"),
        Some("895 条")
    );
    assert_eq!(
        diagnostic_evidence_value(codex, "数据库证据", "可信状态"),
        Some("可信")
    );
    assert_eq!(
        diagnostic_evidence_value(codex, "最新问题", "最新问题"),
        Some("无")
    );

    let serialized = serde_json::to_string(codex).unwrap();
    for raw_key in [
        "latest_signal_at",
        "last_signal_at",
        "latest_signal_result",
        "latest_success_trust",
        "latest_storage_by_source",
        "latest_hard_failure_reason",
    ] {
        assert!(
            !serialized.contains(raw_key),
            "raw evidence key leaked: {raw_key}"
        );
    }
}

#[test]
fn source_diagnostics_keeps_recent_success_after_outside_window_only_signal() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path().join("token-fire").as_path());
    let app_state = AppState::new(paths);
    let success_at = Utc.with_ymd_and_hms(2026, 7, 10, 10, 0, 0).unwrap();
    app_state
        .recent_source_signals()
        .record(SourceSignalRecord {
            source: TokenSourceKind::Codex,
            event: SourceIngestEvent::Hook,
            resolution: SourceResolution::TranscriptPath,
            seen_at: success_at,
            inserted: Some(1),
            duplicates: Some(0),
            skipped_outside_tracking: Some(0),
            empty_reason: None,
            error_kind: None,
        });
    app_state
        .recent_source_signals()
        .record(SourceSignalRecord {
            source: TokenSourceKind::Codex,
            event: SourceIngestEvent::Hook,
            resolution: SourceResolution::TranscriptPath,
            seen_at: success_at + chrono::Duration::minutes(2),
            inserted: Some(0),
            duplicates: Some(0),
            skipped_outside_tracking: Some(895),
            empty_reason: Some(SourceEmptyReason::OutsideTrackingWindow),
            error_kind: None,
        });

    let snapshot = app_state
        .source_diagnostics_snapshot_at(success_at + chrono::Duration::minutes(3))
        .unwrap();
    let codex = snapshot
        .sources
        .iter()
        .find(|source| source.source == TokenSourceKind::Codex)
        .unwrap();

    assert_eq!(codex.headline, DiagnosticHeadline::Connected);
    assert_eq!(
        diagnostic_evidence_value(codex, "最近采集", "窗口外记录"),
        Some("895 条")
    );
    assert_eq!(
        diagnostic_evidence_value(codex, "最新问题", "最新问题"),
        Some("无")
    );
    assert!(codex
        .display_summary
        .note_text
        .as_deref()
        .is_some_and(|note| note.contains("895 条窗口外历史记录")));
}

#[test]
fn source_diagnostics_does_not_treat_outside_window_only_as_runtime_error() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path().join("token-fire").as_path());
    let app_state = AppState::new(paths);
    let seen_at = Utc.with_ymd_and_hms(2026, 7, 10, 10, 0, 0).unwrap();
    app_state
        .recent_source_signals()
        .record(SourceSignalRecord {
            source: TokenSourceKind::Codex,
            event: SourceIngestEvent::Hook,
            resolution: SourceResolution::TranscriptPath,
            seen_at,
            inserted: Some(0),
            duplicates: Some(0),
            skipped_outside_tracking: Some(895),
            empty_reason: Some(SourceEmptyReason::OutsideTrackingWindow),
            error_kind: None,
        });

    let snapshot = app_state
        .source_diagnostics_snapshot_at(seen_at + chrono::Duration::minutes(1))
        .unwrap();
    let codex = snapshot
        .sources
        .iter()
        .find(|source| source.source == TokenSourceKind::Codex)
        .unwrap();

    assert_ne!(codex.headline, DiagnosticHeadline::RuntimeError);
    assert_eq!(
        diagnostic_evidence_value(codex, "数据库证据", "可信状态"),
        Some("缺少")
    );
    assert_eq!(
        diagnostic_evidence_value(codex, "最新问题", "最新问题"),
        Some("无")
    );
}

#[test]
fn source_diagnostics_trusts_single_runtime_success_for_six_hours() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path().join("token-fire").as_path());
    let app_state = AppState::new(paths);
    let success_at = Utc.with_ymd_and_hms(2026, 7, 10, 10, 0, 0).unwrap();
    app_state
        .recent_source_signals()
        .record(SourceSignalRecord {
            source: TokenSourceKind::Codex,
            event: SourceIngestEvent::Hook,
            resolution: SourceResolution::TranscriptPath,
            seen_at: success_at,
            inserted: Some(1),
            duplicates: Some(0),
            skipped_outside_tracking: Some(0),
            empty_reason: None,
            error_kind: None,
        });

    let snapshot = app_state
        .source_diagnostics_snapshot_at(success_at + chrono::Duration::minutes(30))
        .unwrap();
    let codex = snapshot
        .sources
        .iter()
        .find(|source| source.source == TokenSourceKind::Codex)
        .unwrap();

    assert_eq!(codex.headline, DiagnosticHeadline::Connected);
    assert_eq!(
        diagnostic_evidence_value(codex, "数据库证据", "可信状态"),
        Some("可信")
    );
}

#[test]
fn source_diagnostics_expires_success_after_six_hours() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path().join("token-fire").as_path());
    let app_state = AppState::new(paths);
    let success_at = Utc.with_ymd_and_hms(2026, 7, 10, 4, 40, 0).unwrap();
    app_state
        .recent_source_signals()
        .record(SourceSignalRecord {
            source: TokenSourceKind::Codex,
            event: SourceIngestEvent::Hook,
            resolution: SourceResolution::TranscriptPath,
            seen_at: success_at,
            inserted: Some(1),
            duplicates: Some(0),
            skipped_outside_tracking: Some(0),
            empty_reason: None,
            error_kind: None,
        });
    app_state
        .recent_source_signals()
        .record(SourceSignalRecord {
            source: TokenSourceKind::Codex,
            event: SourceIngestEvent::Hook,
            resolution: SourceResolution::TranscriptPath,
            seen_at: success_at + chrono::Duration::minutes(3),
            inserted: Some(0),
            duplicates: Some(0),
            skipped_outside_tracking: Some(0),
            empty_reason: Some(SourceEmptyReason::NoNewCompleteRound),
            error_kind: None,
        });

    let snapshot = app_state
        .source_diagnostics_snapshot_at(success_at + chrono::Duration::hours(7))
        .unwrap();
    let codex = snapshot
        .sources
        .iter()
        .find(|source| source.source == TokenSourceKind::Codex)
        .unwrap();

    assert_eq!(codex.headline, DiagnosticHeadline::PendingVerification);
    assert!(codex.trust_summary.contains("最近成功已过期"));
    assert_eq!(
        diagnostic_evidence_value(codex, "数据库证据", "可信状态"),
        Some("已过期")
    );
}

#[test]
fn source_diagnostics_hard_failure_after_success_overrides_trust() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path().join("token-fire").as_path());
    let app_state = AppState::new(paths);
    let success_at = Utc.with_ymd_and_hms(2026, 7, 10, 10, 0, 0).unwrap();
    app_state
        .recent_source_signals()
        .record(SourceSignalRecord {
            source: TokenSourceKind::Codex,
            event: SourceIngestEvent::Hook,
            resolution: SourceResolution::TranscriptPath,
            seen_at: success_at,
            inserted: Some(1),
            duplicates: Some(0),
            skipped_outside_tracking: Some(0),
            empty_reason: None,
            error_kind: None,
        });
    app_state
        .recent_source_signals()
        .record(SourceSignalRecord {
            source: TokenSourceKind::Codex,
            event: SourceIngestEvent::Hook,
            resolution: SourceResolution::None,
            seen_at: success_at + chrono::Duration::minutes(2),
            inserted: None,
            duplicates: None,
            skipped_outside_tracking: None,
            empty_reason: Some(SourceEmptyReason::TranscriptUnreadable),
            error_kind: None,
        });

    let snapshot = app_state
        .source_diagnostics_snapshot_at(success_at + chrono::Duration::minutes(3))
        .unwrap();
    let codex = snapshot
        .sources
        .iter()
        .find(|source| source.source == TokenSourceKind::Codex)
        .unwrap();

    assert_ne!(codex.headline, DiagnosticHeadline::Connected);
    assert!(codex.trust_summary.contains("最新问题"));
    assert_eq!(
        diagnostic_evidence_value(codex, "最新问题", "最新问题"),
        Some("transcript 不可读")
    );
}

#[test]
fn source_diagnostics_success_after_hard_failure_restores_trust() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path().join("token-fire").as_path());
    let app_state = AppState::new(paths);
    let failure_at = Utc.with_ymd_and_hms(2026, 7, 10, 10, 0, 0).unwrap();
    app_state
        .recent_source_signals()
        .record(SourceSignalRecord {
            source: TokenSourceKind::Codex,
            event: SourceIngestEvent::Hook,
            resolution: SourceResolution::None,
            seen_at: failure_at,
            inserted: None,
            duplicates: None,
            skipped_outside_tracking: None,
            empty_reason: Some(SourceEmptyReason::TranscriptUnreadable),
            error_kind: None,
        });
    app_state
        .recent_source_signals()
        .record(SourceSignalRecord {
            source: TokenSourceKind::Codex,
            event: SourceIngestEvent::Hook,
            resolution: SourceResolution::TranscriptPath,
            seen_at: failure_at + chrono::Duration::minutes(5),
            inserted: Some(1),
            duplicates: Some(0),
            skipped_outside_tracking: Some(0),
            empty_reason: None,
            error_kind: None,
        });

    let snapshot = app_state
        .source_diagnostics_snapshot_at(failure_at + chrono::Duration::minutes(6))
        .unwrap();
    let codex = snapshot
        .sources
        .iter()
        .find(|source| source.source == TokenSourceKind::Codex)
        .unwrap();

    assert_eq!(codex.headline, DiagnosticHeadline::Connected);
    assert!(codex.primary_break.is_none());
    assert_eq!(
        diagnostic_evidence_value(codex, "数据库证据", "可信状态"),
        Some("可信")
    );
}

#[test]
fn source_diagnostics_keeps_trusted_success_when_latest_signal_is_stale_noop() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path().join("token-fire").as_path());
    let app_state = AppState::new(paths);
    let success_at = Utc.with_ymd_and_hms(2026, 7, 10, 10, 0, 0).unwrap();
    app_state
        .recent_source_signals()
        .record(SourceSignalRecord {
            source: TokenSourceKind::Codex,
            event: SourceIngestEvent::Hook,
            resolution: SourceResolution::TranscriptPath,
            seen_at: success_at,
            inserted: Some(1),
            duplicates: Some(0),
            skipped_outside_tracking: Some(0),
            empty_reason: None,
            error_kind: None,
        });
    app_state
        .recent_source_signals()
        .record(SourceSignalRecord {
            source: TokenSourceKind::Codex,
            event: SourceIngestEvent::Hook,
            resolution: SourceResolution::TranscriptPath,
            seen_at: success_at + chrono::Duration::minutes(20),
            inserted: Some(0),
            duplicates: Some(0),
            skipped_outside_tracking: Some(0),
            empty_reason: Some(SourceEmptyReason::NoNewCompleteRound),
            error_kind: None,
        });

    let snapshot = app_state
        .source_diagnostics_snapshot_at(success_at + chrono::Duration::minutes(40))
        .unwrap();
    let codex = snapshot
        .sources
        .iter()
        .find(|source| source.source == TokenSourceKind::Codex)
        .unwrap();

    assert_eq!(codex.headline, DiagnosticHeadline::Connected);
    assert!(codex.trust_summary.contains("最近成功"));
}

#[test]
fn source_diagnostics_config_error_overrides_recent_success() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path().join("token-fire").as_path());
    let claude_config = dir.path().join("claude-settings.json");
    fs::write(&claude_config, r#"{"hooks": "#).unwrap();
    let managers = token_fire::app::state::SourceHookManagers::new(
        HookConfigManager::new(dir.path().join("traecli.toml"), paths.backups_dir.clone()),
        CodexHookConfigManager::new(
            dir.path().join("codex-hooks.json"),
            paths.backups_dir.clone(),
        ),
        ClaudeHookConfigManager::new(claude_config, paths.backups_dir.clone()),
        CursorHookConfigManager::new(
            dir.path().join("cursor-hooks.json"),
            paths.backups_dir.clone(),
        ),
    );
    let app_state = AppState::new_with_source_hook_managers(paths, managers);
    let success_at = Utc.with_ymd_and_hms(2026, 7, 10, 10, 0, 0).unwrap();
    app_state
        .recent_source_signals()
        .record(SourceSignalRecord {
            source: TokenSourceKind::Claude,
            event: SourceIngestEvent::Hook,
            resolution: SourceResolution::TranscriptPath,
            seen_at: success_at,
            inserted: Some(1),
            duplicates: Some(0),
            skipped_outside_tracking: Some(0),
            empty_reason: None,
            error_kind: None,
        });
    app_state
        .recent_source_signals()
        .record(SourceSignalRecord {
            source: TokenSourceKind::Claude,
            event: SourceIngestEvent::Hook,
            resolution: SourceResolution::None,
            seen_at: success_at + chrono::Duration::minutes(1),
            inserted: None,
            duplicates: None,
            skipped_outside_tracking: None,
            empty_reason: None,
            error_kind: Some("transcript_unreadable".to_string()),
        });

    let snapshot = app_state
        .source_diagnostics_snapshot_at(success_at + chrono::Duration::minutes(3))
        .unwrap();
    let claude = snapshot
        .sources
        .iter()
        .find(|source| source.source == TokenSourceKind::Claude)
        .unwrap();

    assert_eq!(claude.headline, DiagnosticHeadline::ConfigurationError);
    assert!(claude.trust_summary.contains("最新问题"));
    assert_eq!(
        diagnostic_evidence_value(claude, "数据库证据", "可信状态"),
        Some("被当前问题覆盖")
    );
    assert_eq!(
        diagnostic_evidence_value(claude, "最新问题", "最新问题"),
        Some("采集配置错误")
    );
}

#[test]
fn source_diagnostics_uses_recent_storage_success_but_not_old_storage() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path().join("token-fire").as_path());
    let store = UsageStore::open(&paths.database).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 7, 10, 10, 0, 0).unwrap();
    let recent_row =
        normalized_observation("recent-codex-row", "codex", Some("codex-model"), 100, now);
    let window_id = store
        .open_tracking_window(now - chrono::Duration::minutes(1))
        .unwrap();
    store
        .insert_observation_for_tracking_window(&recent_row, window_id)
        .unwrap();
    let conn = rusqlite::Connection::open(&paths.database).unwrap();
    conn.execute(
        "update token_observations set created_at = ?1 where source_record_id = ?2",
        rusqlite::params![now.to_rfc3339(), recent_row.source_record_id],
    )
    .unwrap();
    let app_state = AppState::new(paths.clone());

    let snapshot = app_state
        .source_diagnostics_snapshot_at(now + chrono::Duration::minutes(1))
        .unwrap();
    let codex = snapshot
        .sources
        .iter()
        .find(|source| source.source == TokenSourceKind::Codex)
        .unwrap();
    assert_eq!(codex.headline, DiagnosticHeadline::Connected);

    let old_snapshot = app_state
        .source_diagnostics_snapshot_at(now + chrono::Duration::hours(7))
        .unwrap();
    let old_codex = old_snapshot
        .sources
        .iter()
        .find(|source| source.source == TokenSourceKind::Codex)
        .unwrap();
    assert_ne!(old_codex.headline, DiagnosticHeadline::Connected);
}

#[test]
fn source_diagnostics_prefers_newer_storage_evidence_over_older_runtime_success() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path().join("token-fire").as_path());
    let store = UsageStore::open(&paths.database).unwrap();
    let generated_at = Utc.with_ymd_and_hms(2026, 7, 10, 10, 30, 0).unwrap();
    let storage_created_at = generated_at - chrono::Duration::minutes(30);
    let row = normalized_observation(
        "codex-storage-only-row",
        "codex",
        Some("codex-model"),
        100,
        generated_at - chrono::Duration::minutes(40),
    );
    let window_id = store
        .open_tracking_window(row.observed_at - chrono::Duration::minutes(1))
        .unwrap();
    store
        .insert_observation_for_tracking_window(&row, window_id)
        .unwrap();
    // `created_at` is the storage success clock, independent from `observed_at`.
    let conn = rusqlite::Connection::open(&paths.database).unwrap();
    conn.execute(
        "update token_observations set created_at = ?1 where source_record_id = ?2",
        rusqlite::params![storage_created_at.to_rfc3339(), row.source_record_id],
    )
    .unwrap();
    let app_state = AppState::new(paths);
    let runtime_success_at = generated_at - chrono::Duration::hours(1);
    app_state
        .recent_source_signals()
        .record(SourceSignalRecord {
            source: TokenSourceKind::Codex,
            event: SourceIngestEvent::Hook,
            resolution: SourceResolution::TranscriptPath,
            seen_at: runtime_success_at,
            inserted: Some(7),
            duplicates: Some(0),
            skipped_outside_tracking: Some(0),
            empty_reason: None,
            error_kind: None,
        });
    let expected_storage_time = storage_created_at
        .with_timezone(&Local)
        .format("%H:%M")
        .to_string();

    let snapshot = app_state
        .source_diagnostics_snapshot_at(generated_at)
        .unwrap();
    let codex = snapshot
        .sources
        .iter()
        .find(|source| source.source == TokenSourceKind::Codex)
        .unwrap();

    assert_eq!(codex.headline, DiagnosticHeadline::Connected);
    assert_eq!(
        codex.display_summary.detail_text,
        format!("数据库最近写入 · {expected_storage_time}")
    );
    assert!(!codex.display_summary.detail_text.contains("7 条"));
}

#[test]
fn source_diagnostics_trusts_recent_storage_success_when_latest_signal_is_noop() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path().join("token-fire").as_path());
    let store = UsageStore::open(&paths.database).unwrap();
    let generated_at = Utc.with_ymd_and_hms(2026, 7, 10, 10, 30, 0).unwrap();
    let storage_created_at = generated_at - chrono::Duration::minutes(30);
    let row = normalized_observation(
        "codex-storage-plus-noop-row",
        "codex",
        Some("codex-model"),
        100,
        generated_at - chrono::Duration::minutes(40),
    );
    let window_id = store
        .open_tracking_window(row.observed_at - chrono::Duration::minutes(1))
        .unwrap();
    store
        .insert_observation_for_tracking_window(&row, window_id)
        .unwrap();
    let conn = rusqlite::Connection::open(&paths.database).unwrap();
    conn.execute(
        "update token_observations set created_at = ?1 where source_record_id = ?2",
        rusqlite::params![storage_created_at.to_rfc3339(), row.source_record_id],
    )
    .unwrap();
    let app_state = AppState::new(paths);
    app_state
        .recent_source_signals()
        .record(SourceSignalRecord {
            source: TokenSourceKind::Codex,
            event: SourceIngestEvent::Hook,
            resolution: SourceResolution::TranscriptPath,
            seen_at: generated_at - chrono::Duration::minutes(5),
            inserted: Some(0),
            duplicates: Some(0),
            skipped_outside_tracking: Some(0),
            empty_reason: Some(SourceEmptyReason::NoNewCompleteRound),
            error_kind: None,
        });

    let snapshot = app_state
        .source_diagnostics_snapshot_at(generated_at)
        .unwrap();
    let codex = snapshot
        .sources
        .iter()
        .find(|source| source.source == TokenSourceKind::Codex)
        .unwrap();

    assert_eq!(codex.headline, DiagnosticHeadline::Connected);
    assert!(codex.trust_summary.contains("已入库"));
    assert_eq!(
        diagnostic_evidence_value(codex, "最近采集", "检查结果"),
        Some("无新增完整轮次")
    );
    assert_eq!(
        codex.display_summary.note_text.as_deref(),
        Some("最新检查无新增完整轮次")
    );
    assert_eq!(
        diagnostic_evidence_value(codex, "数据库证据", "可信状态"),
        Some("可信")
    );
}

#[test]
fn source_diagnostics_rejects_storage_only_future_created_at_trust() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path().join("token-fire").as_path());
    let store = UsageStore::open(&paths.database).unwrap();
    let generated_at = Utc.with_ymd_and_hms(2026, 7, 10, 10, 30, 0).unwrap();
    let future_storage_created_at = generated_at + chrono::Duration::minutes(30);
    let row = normalized_observation(
        "codex-storage-only-future-row",
        "codex",
        Some("codex-model"),
        100,
        generated_at - chrono::Duration::minutes(40),
    );
    let window_id = store
        .open_tracking_window(row.observed_at - chrono::Duration::minutes(1))
        .unwrap();
    store
        .insert_observation_for_tracking_window(&row, window_id)
        .unwrap();
    // Future storage clocks are rejected instead of trusted as synthetic success evidence.
    let conn = rusqlite::Connection::open(&paths.database).unwrap();
    conn.execute(
        "update token_observations set created_at = ?1 where source_record_id = ?2",
        rusqlite::params![future_storage_created_at.to_rfc3339(), row.source_record_id],
    )
    .unwrap();
    let app_state = AppState::new(paths);

    let snapshot = app_state
        .source_diagnostics_snapshot_at(generated_at)
        .unwrap();
    let codex = snapshot
        .sources
        .iter()
        .find(|source| source.source == TokenSourceKind::Codex)
        .unwrap();

    assert_ne!(codex.headline, DiagnosticHeadline::Connected);
    assert_eq!(
        diagnostic_evidence_value(codex, "数据库证据", "可信状态"),
        Some("缺少")
    );
}

#[test]
fn source_diagnostics_optional_source_with_recent_success_is_not_disabled() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path().join("token-fire").as_path());
    let managers = token_fire::app::state::SourceHookManagers::new(
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
    let app_state = AppState::new_with_source_hook_managers(paths, managers);
    let success_at = Utc.with_ymd_and_hms(2026, 7, 10, 10, 0, 0).unwrap();
    app_state
        .recent_source_signals()
        .record(SourceSignalRecord {
            source: TokenSourceKind::Cursor,
            event: SourceIngestEvent::Hook,
            resolution: SourceResolution::TranscriptPath,
            seen_at: success_at,
            inserted: Some(1),
            duplicates: Some(0),
            skipped_outside_tracking: Some(0),
            empty_reason: None,
            error_kind: None,
        });

    let snapshot = app_state
        .source_diagnostics_snapshot_at(success_at + chrono::Duration::minutes(3))
        .unwrap();
    let cursor = snapshot
        .sources
        .iter()
        .find(|source| source.source == TokenSourceKind::Cursor)
        .unwrap();

    assert_ne!(cursor.headline, DiagnosticHeadline::Disabled);
}

#[test]
fn source_diagnostics_sqlite_unavailable_overrides_recent_success() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path().join("token-fire").as_path());
    fs::create_dir_all(paths.database.parent().unwrap()).unwrap();
    fs::write(&paths.database, b"not a sqlite database").unwrap();
    let app_state = AppState::new(paths.clone());
    let success_at = Utc.with_ymd_and_hms(2026, 7, 10, 10, 0, 0).unwrap();
    app_state
        .recent_source_signals()
        .record(SourceSignalRecord {
            source: TokenSourceKind::Codex,
            event: SourceIngestEvent::Hook,
            resolution: SourceResolution::TranscriptPath,
            seen_at: success_at,
            inserted: Some(1),
            duplicates: Some(0),
            skipped_outside_tracking: Some(0),
            empty_reason: None,
            error_kind: None,
        });
    app_state
        .recent_source_signals()
        .record(SourceSignalRecord {
            source: TokenSourceKind::Codex,
            event: SourceIngestEvent::Hook,
            resolution: SourceResolution::None,
            seen_at: success_at + chrono::Duration::minutes(1),
            inserted: None,
            duplicates: None,
            skipped_outside_tracking: None,
            empty_reason: None,
            error_kind: Some("transcript_unreadable".to_string()),
        });

    let snapshot = app_state
        .source_diagnostics_snapshot_at(success_at + chrono::Duration::minutes(3))
        .unwrap();
    let codex = snapshot
        .sources
        .iter()
        .find(|source| source.source == TokenSourceKind::Codex)
        .unwrap();

    assert_ne!(codex.headline, DiagnosticHeadline::Connected);
    assert_eq!(
        diagnostic_evidence_value(codex, "数据库证据", "数据库最近写入"),
        Some("无法确认")
    );
    assert_eq!(
        diagnostic_evidence_value(codex, "数据库证据", "可信状态"),
        Some("被当前问题覆盖")
    );
    assert_eq!(
        diagnostic_evidence_value(codex, "最新问题", "最新问题"),
        Some("SQLite 不可用")
    );
}

#[test]
fn source_diagnostics_returns_storage_error_snapshot_when_sqlite_open_fails() {
    let dir = tempdir().unwrap();
    let mut paths = paths(dir.path().join("token-fire").as_path());
    fs::create_dir_all(paths.database.parent().unwrap()).unwrap();
    fs::write(&paths.database, b"not a sqlite database").unwrap();
    let app_state = AppState::new(paths.clone());
    app_state
        .recent_source_signals()
        .record(SourceSignalRecord {
            source: TokenSourceKind::Traex,
            event: SourceIngestEvent::Hook,
            resolution: SourceResolution::TranscriptPath,
            seen_at: Utc.with_ymd_and_hms(2026, 7, 10, 9, 59, 0).unwrap(),
            inserted: Some(1),
            duplicates: Some(0),
            skipped_outside_tracking: Some(0),
            empty_reason: None,
            error_kind: None,
        });

    let snapshot = app_state
        .source_diagnostics_snapshot_at(Utc.with_ymd_and_hms(2026, 7, 10, 10, 0, 0).unwrap())
        .unwrap();
    let traex = snapshot
        .sources
        .iter()
        .find(|source| source.source == TokenSourceKind::Traex)
        .unwrap();
    let storage = traex
        .chain
        .iter()
        .find(|stage| stage.key == DiagnosticStageKey::Storage)
        .unwrap();
    let body = serde_json::to_string(&snapshot).unwrap();

    assert_eq!(storage.status, DiagnosticStatus::Error);
    assert_eq!(
        traex.primary_break.as_ref().unwrap().stage,
        DiagnosticStageKey::Storage
    );
    assert!(body.contains("sqlite_unavailable"));
    assert!(!body.contains(paths.database.to_str().unwrap()));
    paths.database = dir.path().join("another-private-name.sqlite");
    assert!(!body.contains(paths.database.to_str().unwrap()));
}

#[test]
fn source_diagnostics_ignores_append_only_hook_log_for_signal_status() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path().join("token-fire").as_path());
    let config = dir.path().join("traecli.toml");
    let sessions_dir = dir.path().join("sessions");
    let archived_sessions_dir = dir.path().join("archived_sessions");
    fs::create_dir_all(&sessions_dir).unwrap();
    fs::create_dir_all(&archived_sessions_dir).unwrap();
    let hook_path = dir
        .path()
        .join("TokenFire.app")
        .join("Contents/MacOS/token-fire-hook");
    fs::create_dir_all(hook_path.parent().unwrap()).unwrap();
    fs::write(&hook_path, b"#!/bin/sh\n").unwrap();
    make_executable(&hook_path);
    let manager = HookConfigManager::new(config.clone(), paths.backups_dir.clone());
    manager.install(&hook_path).unwrap();
    write_jsonl_event(
        &paths.hook_log,
        "hook",
        "info",
        "hook_received",
        json!({
            "source": "traex",
            "hook_path": hook_path.to_string_lossy(),
            "ts": "2026-07-10T09:59:00Z"
        }),
    )
    .unwrap();
    let hook_log = paths.hook_log.clone();
    let app_state = AppState::new_with_hook_config_manager_source_statuses(
        paths,
        manager,
        Some(TraexStatusSource::new(
            config,
            hook_log,
            TraexPaths {
                sessions_dir,
                archived_sessions_dir,
            },
        )),
        None,
    );

    let snapshot = app_state
        .source_diagnostics_snapshot_at(Utc.with_ymd_and_hms(2026, 7, 10, 10, 0, 0).unwrap())
        .unwrap();
    let traex = snapshot
        .sources
        .iter()
        .find(|source| source.source == TokenSourceKind::Traex)
        .unwrap();
    let signal = traex
        .chain
        .iter()
        .find(|stage| stage.key == DiagnosticStageKey::Signal)
        .unwrap();

    assert_eq!(signal.status, DiagnosticStatus::Unknown);
    assert!(signal.evidence.is_none());
}

#[test]
fn source_diagnostics_marks_optional_config_without_hook_as_actionable() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path().join("token-fire").as_path());
    let claude_config = dir.path().join("claude-settings.json");
    fs::write(&claude_config, r#"{"hooks": {}}"#).unwrap();
    let managers = token_fire::app::state::SourceHookManagers::new(
        HookConfigManager::new(dir.path().join("traecli.toml"), paths.backups_dir.clone()),
        CodexHookConfigManager::new(
            dir.path().join("codex-hooks.json"),
            paths.backups_dir.clone(),
        ),
        ClaudeHookConfigManager::new(claude_config, paths.backups_dir.clone()),
        CursorHookConfigManager::new(
            dir.path().join("cursor-hooks.json"),
            paths.backups_dir.clone(),
        ),
    );
    let app_state = AppState::new_with_source_hook_managers(paths, managers);

    let snapshot = app_state
        .source_diagnostics_snapshot_at(Utc.with_ymd_and_hms(2026, 7, 10, 10, 0, 0).unwrap())
        .unwrap();
    let claude = snapshot
        .sources
        .iter()
        .find(|source| source.source == TokenSourceKind::Claude)
        .unwrap();

    assert_ne!(claude.headline, DiagnosticHeadline::Disabled);
    assert!(claude.primary_break.is_some());
    assert!(claude.chain.iter().any(|stage| {
        stage.key == DiagnosticStageKey::Participation && stage.status == DiagnosticStatus::Unknown
    }));
}

#[test]
fn source_diagnostics_sanitizes_config_error_paths() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path().join("token-fire").as_path());
    let secret_dir = dir.path().join("private-config-root");
    fs::create_dir_all(&secret_dir).unwrap();
    let managers = token_fire::app::state::SourceHookManagers::new(
        HookConfigManager::new(dir.path().join("traecli.toml"), paths.backups_dir.clone()),
        CodexHookConfigManager::new(
            dir.path().join("codex-hooks.json"),
            paths.backups_dir.clone(),
        ),
        ClaudeHookConfigManager::new(secret_dir.clone(), paths.backups_dir.clone()),
        CursorHookConfigManager::new(
            dir.path().join("cursor-hooks.json"),
            paths.backups_dir.clone(),
        ),
    );
    let app_state = AppState::new_with_source_hook_managers(paths, managers);

    let snapshot = app_state
        .source_diagnostics_snapshot_at(Utc.with_ymd_and_hms(2026, 7, 10, 10, 0, 0).unwrap())
        .unwrap();

    let body = serde_json::to_string(&snapshot).unwrap();
    assert!(body.contains("config_error") || body.contains("配置读取失败"));
    assert!(!body.contains(secret_dir.to_str().unwrap()));
    assert!(!body.contains(dir.path().to_str().unwrap()));
}

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

fn runtime_paths(home: &std::path::Path) -> RuntimePaths {
    paths(home)
}

fn assert_cost_close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 0.000_001,
        "expected {expected}, got {actual}"
    );
}

fn normalized_observation(
    source_record_id: &str,
    source: &str,
    model: Option<&str>,
    total_tokens: i64,
    observed_at: chrono::DateTime<Utc>,
) -> token_fire::core::observation::NormalizedObservation {
    token_fire::core::observation::NormalizedObservation {
        source: source.to_string(),
        adapter_version: "test-v1".to_string(),
        source_record_id: source_record_id.to_string(),
        source_record_id_confidence: token_fire::core::observation::SourceRecordIdConfidence::Exact,
        session_id: Some("session-profile".to_string()),
        turn_id: Some("turn-profile".to_string()),
        turn_boundary_id: Some("turn-profile".to_string()),
        source_path: Some("/tmp/profile.jsonl".to_string()),
        line_no: Some(1),
        byte_offset: Some(1),
        input_tokens: total_tokens,
        output_tokens: 0,
        cached_input_tokens: 0,
        cache_creation_input_tokens: 0,
        reasoning_output_tokens: 0,
        total_tokens,
        cumulative_total_tokens: Some(total_tokens),
        model: model.map(str::to_string),
        cwd: Some("~/project".to_string()),
        observed_at,
        token_payload_hash: format!("hash-{source_record_id}"),
    }
}

#[test]
fn app_state_profile_summary_reads_store_contract() {
    let dir = tempdir().unwrap();
    let paths = runtime_paths(dir.path());
    let store = UsageStore::open(&paths.database).unwrap();
    let now_utc = Utc.with_ymd_and_hms(2026, 7, 4, 12, 0, 0).unwrap();
    let now_local = now_utc.with_timezone(&Local);
    let row = normalized_observation(
        "profile-app-state",
        "traex",
        Some("gpt-5.5"),
        1_000_000,
        now_utc - chrono::Duration::hours(2),
    );
    let window_id = store
        .open_tracking_window(now_utc - chrono::Duration::days(1))
        .unwrap();
    store
        .insert_observation_for_tracking_window(&row, window_id)
        .unwrap();

    let state = AppState::new(paths);
    let summary = state
        .profile_summary_at(
            token_fire::core::profile::ProfilePeriod::Today,
            now_utc,
            now_local,
        )
        .unwrap();

    assert_eq!(
        summary.selected_period.period,
        token_fire::core::profile::ProfilePeriod::Today
    );
    assert_eq!(summary.year_profile.days.len(), 365);
    assert_eq!(summary.selected_period.source_breakdown[0].label, "TraeX");
}

#[test]
fn widget_state_reads_sqlite_aggregates() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path());
    let store = UsageStore::open(&paths.database).unwrap();
    let observed_at = Utc.with_ymd_and_hms(2026, 6, 20, 3, 0, 0).unwrap();
    let window_id = store
        .open_tracking_window(observed_at - chrono::Duration::minutes(1))
        .unwrap();
    store
        .insert_observation_for_tracking_window(
            &NormalizedObservation {
                source: "traex".to_string(),
                adapter_version: "traex-jsonl-v1".to_string(),
                source_record_id: "session-a:10".to_string(),
                source_record_id_confidence: SourceRecordIdConfidence::Exact,
                session_id: Some("session-a".to_string()),
                turn_id: Some("turn-a".to_string()),
                turn_boundary_id: Some("turn-a".to_string()),
                source_path: Some("/tmp/rollout-session-a.jsonl".to_string()),
                line_no: Some(1),
                byte_offset: Some(10),
                input_tokens: 100,
                output_tokens: 28,
                cached_input_tokens: 0,
                cache_creation_input_tokens: 0,
                reasoning_output_tokens: 0,
                total_tokens: 128,
                cumulative_total_tokens: Some(128),
                model: Some("model-a".to_string()),
                cwd: Some("~/project".to_string()),
                observed_at,
                token_payload_hash: "hash-1".to_string(),
            },
            window_id,
        )
        .unwrap();

    let app_state = AppState::new(paths);
    app_state.set_watcher_ok(true);
    app_state.set_traex_status(TraexStatus {
        hook_installed: true,
        hook_executable_exists: true,
        hook_smoke_test_passed: true,
        sessions_readable: true,
        archived_sessions_readable: true,
        ..TraexStatus::default()
    });

    let widget = app_state.widget_state_at(Local.with_ymd_and_hms(2026, 6, 20, 12, 0, 0).unwrap());

    assert_eq!(widget.today_total_tokens, 128);
    assert_eq!(widget.latest_turn_delta_tokens, 128);
    assert_eq!(widget.status, UiStatus::Green);
}

#[test]
fn widget_state_exposes_revision_and_last_observed_at() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path());
    let store = UsageStore::open(&paths.database).unwrap();
    let observed_at = Utc.with_ymd_and_hms(2026, 6, 28, 3, 0, 0).unwrap();
    let window_id = store
        .open_tracking_window(observed_at - chrono::Duration::minutes(1))
        .unwrap();
    store
        .insert_observation_for_tracking_window(
            &NormalizedObservation {
                source: "traex".to_string(),
                adapter_version: "traex-jsonl-v1".to_string(),
                source_record_id: "state-revision:10".to_string(),
                source_record_id_confidence: SourceRecordIdConfidence::Exact,
                session_id: Some("state-revision".to_string()),
                turn_id: Some("turn-a".to_string()),
                turn_boundary_id: Some("turn-a".to_string()),
                source_path: Some("/tmp/state-revision.jsonl".to_string()),
                line_no: Some(1),
                byte_offset: Some(10),
                input_tokens: 40,
                output_tokens: 60,
                cached_input_tokens: 0,
                cache_creation_input_tokens: 0,
                reasoning_output_tokens: 0,
                total_tokens: 100,
                cumulative_total_tokens: Some(100),
                model: Some("model-a".to_string()),
                cwd: Some("~/project".to_string()),
                observed_at,
                token_payload_hash: "state-revision-hash".to_string(),
            },
            window_id,
        )
        .unwrap();
    let app_state = AppState::new(paths);
    app_state.set_watcher_ok(true);
    app_state.set_traex_status(healthy_traex_status());

    let widget = app_state.widget_state_at(observed_at.with_timezone(&Local));

    assert_eq!(widget.today_total_tokens, 100);
    assert_eq!(widget.state_revision, 1);
    assert_eq!(widget.last_observed_at, Some(observed_at));
}

#[test]
fn widget_usage_series_at_returns_storage_contract() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path());
    let store = UsageStore::open(&paths.database).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 6, 28, 12, 0, 0).unwrap();
    let observed_at = now - chrono::Duration::minutes(2);
    let row = NormalizedObservation {
        source: "traex".to_string(),
        adapter_version: "traex-jsonl-v1".to_string(),
        source_record_id: "series-state:10".to_string(),
        source_record_id_confidence: SourceRecordIdConfidence::Exact,
        session_id: Some("series-state".to_string()),
        turn_id: Some("turn-a".to_string()),
        turn_boundary_id: Some("turn-a".to_string()),
        source_path: Some("/tmp/series-state.jsonl".to_string()),
        line_no: Some(1),
        byte_offset: Some(10),
        input_tokens: 10,
        output_tokens: 15,
        cached_input_tokens: 0,
        cache_creation_input_tokens: 0,
        reasoning_output_tokens: 0,
        total_tokens: 25,
        cumulative_total_tokens: Some(25),
        model: Some("model-a".to_string()),
        cwd: Some("~/project".to_string()),
        observed_at,
        token_payload_hash: "series-state-hash".to_string(),
    };
    let window_id = store
        .open_tracking_window(now - chrono::Duration::hours(1))
        .unwrap();
    store
        .insert_observation_for_tracking_window(&row, window_id)
        .unwrap();
    let previous_day_row = NormalizedObservation {
        source: "traex".to_string(),
        adapter_version: "traex-jsonl-v1".to_string(),
        source_record_id: "series-state-previous-day:10".to_string(),
        source_record_id_confidence: SourceRecordIdConfidence::Exact,
        session_id: Some("series-state-previous-day".to_string()),
        turn_id: Some("turn-b".to_string()),
        turn_boundary_id: Some("turn-b".to_string()),
        source_path: Some("/tmp/series-state-previous-day.jsonl".to_string()),
        line_no: Some(2),
        byte_offset: Some(20),
        input_tokens: 30,
        output_tokens: 45,
        cached_input_tokens: 0,
        cache_creation_input_tokens: 0,
        reasoning_output_tokens: 0,
        total_tokens: 75,
        cumulative_total_tokens: Some(75),
        model: Some("model-a".to_string()),
        cwd: Some("~/project".to_string()),
        observed_at: now - chrono::Duration::hours(24) - chrono::Duration::minutes(2),
        token_payload_hash: "series-state-previous-day-hash".to_string(),
    };
    let previous_day_window_id = store
        .open_tracking_window(previous_day_row.observed_at - chrono::Duration::hours(1))
        .unwrap();
    store
        .insert_observation_for_tracking_window(&previous_day_row, previous_day_window_id)
        .unwrap();
    let app_state = AppState::new(paths);

    let series = app_state.widget_usage_series_at(now).unwrap();

    assert_eq!(series.window_minutes, 360);
    assert_eq!(series.bucket_minutes, 30);
    assert_eq!(series.buckets.len(), 12);
    assert_eq!(series.previous_day_buckets.len(), 12);
    assert_eq!(series.previous_day_buckets[11].total_tokens, 75);
    assert_eq!(series.latest_bucket_tokens, 25);
    assert!(series.latest_bucket_active);
}

#[test]
fn widget_cost_summary_at_returns_cost_contract() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path());
    let store = UsageStore::open(&paths.database).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 7, 2, 12, 0, 0).unwrap();
    let observed_at = now - chrono::Duration::minutes(2);
    let row = NormalizedObservation {
        source: "traex".to_string(),
        adapter_version: "traex-jsonl-v1".to_string(),
        source_record_id: "cost-state:10".to_string(),
        source_record_id_confidence: SourceRecordIdConfidence::Exact,
        session_id: Some("cost-state".to_string()),
        turn_id: Some("turn-a".to_string()),
        turn_boundary_id: Some("turn-a".to_string()),
        source_path: Some("/tmp/cost-state.jsonl".to_string()),
        line_no: Some(1),
        byte_offset: Some(10),
        input_tokens: 1_000_000,
        output_tokens: 0,
        cached_input_tokens: 0,
        cache_creation_input_tokens: 0,
        reasoning_output_tokens: 0,
        total_tokens: 1_000_000,
        cumulative_total_tokens: Some(1_000_000),
        model: Some("unknown-model".to_string()),
        cwd: Some("~/project".to_string()),
        observed_at,
        token_payload_hash: "cost-state-hash".to_string(),
    };
    let window_id = store
        .open_tracking_window(now - chrono::Duration::hours(1))
        .unwrap();
    store
        .insert_observation_for_tracking_window(&row, window_id)
        .unwrap();
    let app_state = AppState::new(paths);

    let summary = app_state
        .widget_cost_summary_at(now, now.with_timezone(&Local))
        .unwrap();

    assert_eq!(summary.currency, "CNY");
    assert_eq!(summary.today.total_tokens, 1_000_000);
    assert_eq!(summary.today.pricing_status, PricingStatus::Fallback);
    assert_cost_close(summary.today.estimated_cost, 3.0);
}

#[test]
fn widget_state_reports_red_when_watcher_is_unhealthy() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path());
    let app_state = AppState::new(paths);
    app_state.set_traex_status(healthy_traex_status());
    app_state.set_socket_ok(true);
    app_state.set_watcher_ok(false);

    let widget = app_state.widget_state_at(Local.with_ymd_and_hms(2026, 6, 20, 12, 0, 0).unwrap());

    assert_eq!(widget.status, UiStatus::Red);
}

#[test]
fn runtime_health_reporter_recovers_watcher_and_sqlite_after_success() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path());
    let app_state = AppState::new(paths);
    app_state.set_traex_status(healthy_traex_status());
    app_state.set_socket_ok(true);
    let reporter = app_state.runtime_health_reporter();

    reporter.set_watcher_ok(false);
    reporter.set_sqlite_ok(false);
    assert_eq!(
        app_state
            .widget_state_at(Local.with_ymd_and_hms(2026, 6, 20, 12, 0, 0).unwrap())
            .status,
        UiStatus::Red
    );

    reporter.record_successful_ingest();

    assert_eq!(
        app_state
            .widget_state_at(Local.with_ymd_and_hms(2026, 6, 20, 12, 0, 0).unwrap())
            .status,
        UiStatus::Green
    );
}

#[test]
fn traex_status_reads_hook_installation_from_toml_command_fields_only() {
    let dir = tempdir().unwrap();
    let config = dir.path().join("traecli.toml");
    let hook_log = dir.path().join("hook.log");
    let sessions_dir = dir.path().join("sessions");
    let archived_sessions_dir = dir.path().join("archived_sessions");
    fs::create_dir_all(&sessions_dir).unwrap();
    fs::create_dir_all(&archived_sessions_dir).unwrap();
    let traex_paths = TraexPaths {
        sessions_dir,
        archived_sessions_dir,
    };

    fs::write(
        &config,
        r#"
# command = "'/Applications/TokenFire.app/Contents/MacOS/token-fire-hook' --owner token-fire"
notes = "token-fire-hook --owner token-fire"

[[Stop.hooks]]
type = "command"
command = "echo not-token-fire"
"#,
    )
    .unwrap();

    let status = collect_traex_status(&config, &hook_log, &traex_paths);
    assert!(!status.hook_installed);

    fs::write(
        &config,
        r#"
notes = "token-fire-hook --owner token-fire"

[[Stop.hooks]]
type = "command"
command = "'/Applications/TokenFire.app/Contents/MacOS/token-fire-hook' --source traex --owner token-fire"
"#,
    )
    .unwrap();

    let status = collect_traex_status(&config, &hook_log, &traex_paths);
    assert!(status.hook_installed);
}

#[test]
fn traex_status_reads_hook_installation_when_stop_is_array_of_tables() {
    let dir = tempdir().unwrap();
    let config = dir.path().join("traecli.toml");
    let hook_log = dir.path().join("hook.log");
    let sessions_dir = dir.path().join("sessions");
    let archived_sessions_dir = dir.path().join("archived_sessions");
    fs::create_dir_all(&sessions_dir).unwrap();
    fs::create_dir_all(&archived_sessions_dir).unwrap();

    fs::write(
        &config,
        r#"[hooks]
[[hooks.Stop.hooks]]
type = "command"
command = "'/Applications/TokenFire.app/Contents/MacOS/token-fire-hook' --source traex --owner token-fire"
"#,
    )
    .unwrap();

    let status = collect_traex_status(
        &config,
        &hook_log,
        &TraexPaths {
            sessions_dir,
            archived_sessions_dir,
        },
    );

    assert!(status.hook_installed);
}

#[test]
fn traex_status_unescapes_single_quoted_hook_paths_with_apostrophes() {
    let dir = tempdir().unwrap();
    let config = dir.path().join("traecli.toml");
    let hook_log = dir.path().join("hook.log");
    let sessions_dir = dir.path().join("sessions");
    let archived_sessions_dir = dir.path().join("archived_sessions");
    fs::create_dir_all(&sessions_dir).unwrap();
    fs::create_dir_all(&archived_sessions_dir).unwrap();
    let hook_path = dir
        .path()
        .join("Token'Fire.app")
        .join("Contents/MacOS/token-fire-hook");
    fs::create_dir_all(hook_path.parent().unwrap()).unwrap();
    fs::write(&hook_path, b"#!/bin/sh\n").unwrap();
    make_executable(&hook_path);
    let manager = HookConfigManager::new(config.clone(), dir.path().join("backups"));
    manager.install(&hook_path).unwrap();

    let status = collect_traex_status(
        &config,
        &hook_log,
        &TraexPaths {
            sessions_dir,
            archived_sessions_dir,
        },
    );

    assert!(status.hook_installed);
    assert!(status.hook_executable_exists);
}

#[test]
fn traex_status_requires_hook_file_to_be_executable() {
    let dir = tempdir().unwrap();
    let config = dir.path().join("traecli.toml");
    let hook_log = dir.path().join("hook.log");
    let sessions_dir = dir.path().join("sessions");
    let archived_sessions_dir = dir.path().join("archived_sessions");
    fs::create_dir_all(&sessions_dir).unwrap();
    fs::create_dir_all(&archived_sessions_dir).unwrap();
    let hook_path = dir
        .path()
        .join("TokenFire.app")
        .join("Contents/MacOS/token-fire-hook");
    fs::create_dir_all(hook_path.parent().unwrap()).unwrap();
    fs::write(&hook_path, b"#!/bin/sh\n").unwrap();
    let manager = HookConfigManager::new(config.clone(), dir.path().join("backups"));
    manager.install(&hook_path).unwrap();

    let status = collect_traex_status(
        &config,
        &hook_log,
        &TraexPaths {
            sessions_dir,
            archived_sessions_dir,
        },
    );

    assert!(status.hook_installed);
    assert!(!status.hook_executable_exists);
}

#[test]
fn widget_state_refreshes_traex_status_without_restarting_app_state() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path().join("token-fire").as_path());
    let config = dir.path().join("traecli.toml");
    let sessions_dir = dir.path().join("sessions");
    let archived_sessions_dir = dir.path().join("archived_sessions");
    fs::create_dir_all(&sessions_dir).unwrap();
    fs::create_dir_all(&archived_sessions_dir).unwrap();
    let source = TraexStatusSource::new(
        config.clone(),
        paths.hook_log.clone(),
        TraexPaths {
            sessions_dir,
            archived_sessions_dir,
        },
    );
    let manager = HookConfigManager::new(config, paths.backups_dir.clone());
    let app_state = AppState::new_with_hook_config_manager_and_status_source(
        paths.clone(),
        manager.clone(),
        source,
    );
    app_state.set_watcher_ok(true);

    let now = Local.with_ymd_and_hms(2026, 6, 20, 12, 0, 0).unwrap();
    assert_eq!(app_state.widget_state_at(now).status, UiStatus::Yellow);

    let hook_path = dir
        .path()
        .join("TokenFire.app")
        .join("Contents/MacOS/token-fire-hook");
    fs::create_dir_all(hook_path.parent().unwrap()).unwrap();
    fs::write(&hook_path, b"#!/bin/sh\n").unwrap();
    make_executable(&hook_path);
    manager.install(&hook_path).unwrap();
    write_jsonl_event(
        &paths.hook_log,
        "hook",
        "info",
        "hook_received",
        json!({ "source": "traex", "hook_path": hook_path.to_string_lossy() }),
    )
    .unwrap();

    assert_eq!(app_state.widget_state_at(now).status, UiStatus::Green);
}

#[test]
fn widget_state_folds_enabled_codex_status_into_production_status() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path().join("token-fire").as_path());
    let config = dir.path().join("traecli.toml");
    let sessions_dir = dir.path().join("traex/sessions");
    let archived_sessions_dir = dir.path().join("traex/archived_sessions");
    fs::create_dir_all(&sessions_dir).unwrap();
    fs::create_dir_all(&archived_sessions_dir).unwrap();
    let hook_path = dir
        .path()
        .join("TokenFire.app")
        .join("Contents/MacOS/token-fire-hook");
    fs::create_dir_all(hook_path.parent().unwrap()).unwrap();
    fs::write(&hook_path, b"#!/bin/sh\n").unwrap();
    make_executable(&hook_path);
    let manager = HookConfigManager::new(config.clone(), paths.backups_dir.clone());
    manager.install(&hook_path).unwrap();
    write_jsonl_event(
        &paths.hook_log,
        "hook",
        "info",
        "hook_received",
        json!({ "source": "traex", "hook_path": hook_path.to_string_lossy() }),
    )
    .unwrap();
    let traex_source = TraexStatusSource::new(
        config,
        paths.hook_log.clone(),
        TraexPaths {
            sessions_dir,
            archived_sessions_dir,
        },
    );
    let codex_sessions = dir.path().join("codex/sessions");
    fs::create_dir_all(&codex_sessions).unwrap();
    let codex_source = CodexStatusSource::new(
        dir.path().join("codex/hooks.json"),
        SourcePaths::new(
            TokenSourceKind::Codex,
            codex_sessions,
            dir.path().join("codex/archived_sessions"),
        ),
    );
    let app_state = AppState::new_with_hook_config_manager_source_statuses(
        paths,
        manager,
        Some(traex_source),
        Some(codex_source),
    );
    app_state.set_watcher_ok(true);

    let widget = app_state.widget_state_at(Local.with_ymd_and_hms(2026, 6, 20, 12, 0, 0).unwrap());

    assert_eq!(widget.status, UiStatus::Yellow);
}

#[test]
fn codex_status_reads_hook_installation_from_command_fields_only() {
    let dir = tempdir().unwrap();
    let config = dir.path().join("hooks.json");
    let hook_log = dir.path().join("hook.log");
    let sessions_dir = dir.path().join("sessions");
    let archived_sessions_dir = dir.path().join("archived_sessions");
    fs::create_dir_all(&sessions_dir).unwrap();
    fs::create_dir_all(&archived_sessions_dir).unwrap();
    let source = CodexStatusSource::new_with_hook_log(
        config.clone(),
        hook_log,
        SourcePaths::new(TokenSourceKind::Codex, sessions_dir, archived_sessions_dir),
    );

    fs::write(
        &config,
        r#"{
  "notes": "token-fire-hook --owner token-fire",
  "hooks": {
    "Stop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "echo not-token-fire"
          }
        ]
      }
    ]
  }
}"#,
    )
    .unwrap();

    assert!(!source.collect().hook_installed);

    fs::write(
        &config,
        r#"{
  "notes": "token-fire-hook --owner token-fire",
  "hooks": {
    "Stop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "'/Applications/TokenFire.app/Contents/MacOS/token-fire-hook' --source codex --owner token-fire"
          }
        ]
      }
    ]
  }
}"#,
    )
    .unwrap();

    assert!(source.collect().hook_installed);
}

#[test]
fn codex_status_requires_installed_hook_file_to_be_executable() {
    let dir = tempdir().unwrap();
    let (source, hook_path) = codex_status_source_with_installed_hook(dir.path());

    let missing_status = source.collect();
    assert!(missing_status.hook_installed);
    assert!(!missing_status.hook_executable_exists);
    assert!(!missing_status.hook_smoke_test_passed);

    fs::create_dir_all(hook_path.parent().unwrap()).unwrap();
    fs::write(&hook_path, b"#!/bin/sh\n").unwrap();
    let status = source.collect();

    assert!(status.hook_installed);
    assert!(!status.hook_executable_exists);
    assert!(!status.hook_smoke_test_passed);
}

#[test]
fn codex_status_requires_recent_codex_hook_log_for_smoke_pass() {
    let dir = tempdir().unwrap();
    let (source, hook_path) = codex_status_source_with_installed_hook(dir.path());
    fs::create_dir_all(hook_path.parent().unwrap()).unwrap();
    fs::write(&hook_path, b"#!/bin/sh\n").unwrap();
    make_executable(&hook_path);

    write_jsonl_event(
        &dir.path().join("hook.log"),
        "hook",
        "info",
        "hook_received",
        json!({ "source": "traex", "hook_path": hook_path.to_string_lossy() }),
    )
    .unwrap();
    assert!(!source.collect().hook_smoke_test_passed);

    write_jsonl_event(
        &dir.path().join("hook.log"),
        "hook",
        "info",
        "hook_forwarded",
        json!({ "source": "codex", "hook_path": hook_path.to_string_lossy() }),
    )
    .unwrap();

    let status = source.collect();
    assert!(status.hook_executable_exists);
    assert!(status.hook_smoke_test_passed);
    assert!(status.last_hook_seen_at.is_some());
}

#[test]
fn codex_status_rejects_hook_log_older_than_config() {
    let dir = tempdir().unwrap();
    let (source, hook_path) = codex_status_source_with_installed_hook(dir.path());
    fs::create_dir_all(hook_path.parent().unwrap()).unwrap();
    fs::write(&hook_path, b"#!/bin/sh\n").unwrap();
    make_executable(&hook_path);
    fs::write(
        dir.path().join("hook.log"),
        r#"{"ts":"2000-01-01T00:00:00Z","event":"hook_received","source":"codex"}"#,
    )
    .unwrap();

    let status = source.collect();

    assert!(status.hook_executable_exists);
    assert!(!status.hook_smoke_test_passed);
    assert!(status.last_hook_seen_at.is_none());
}

#[test]
fn unverified_hook_execution_warning_is_deduplicated_by_last_seen_time() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path().join("token-fire").as_path());
    let config = dir.path().join("missing-traecli.toml");
    let sessions_dir = dir.path().join("sessions");
    let archived_sessions_dir = dir.path().join("archived_sessions");
    fs::create_dir_all(&sessions_dir).unwrap();
    fs::create_dir_all(&archived_sessions_dir).unwrap();
    fs::create_dir_all(&paths.logs_dir).unwrap();
    fs::write(
        &paths.hook_log,
        r#"{"ts":"2000-01-01T00:00:00Z","event":"hook_received"}"#,
    )
    .unwrap();
    let source = TraexStatusSource::new(
        config,
        paths.hook_log.clone(),
        TraexPaths {
            sessions_dir,
            archived_sessions_dir,
        },
    );
    let manager = HookConfigManager::new(
        dir.path().join("unused-traecli.toml"),
        paths.backups_dir.clone(),
    );
    let app_state =
        AppState::new_with_hook_config_manager_and_status_source(paths.clone(), manager, source);

    app_state.refresh_traex_status();
    app_state.refresh_traex_status();

    let app_log = fs::read_to_string(paths.app_log).unwrap();
    assert_eq!(app_log.matches("hook_execution_unverified").count(), 1);
}

#[test]
fn widget_state_reports_red_when_database_path_cannot_be_queried() {
    let dir = tempdir().unwrap();
    let mut paths = paths(dir.path());
    paths.database = dir.path().join("database-directory");
    fs::create_dir(&paths.database).unwrap();

    let app_state = AppState::new(paths);
    app_state.set_traex_status(healthy_traex_status());

    let widget = app_state.widget_state_at(Local.with_ymd_and_hms(2026, 6, 20, 12, 0, 0).unwrap());

    assert_eq!(widget.today_total_tokens, 0);
    assert_eq!(widget.latest_turn_delta_tokens, 0);
    assert_eq!(widget.status, UiStatus::Red);
}

#[test]
fn widget_state_reports_red_when_sqlite_aggregate_queries_fail() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path());
    let conn = rusqlite::Connection::open(&paths.database).unwrap();
    conn.execute_batch(
        r#"
        create table token_observations (
          id integer primary key,
          observed_at text not null,
          total_tokens integer not null,
          source text not null,
          session_id text,
          turn_boundary_id text
        );
        "#,
    )
    .unwrap();
    drop(conn);

    let app_state = AppState::new(paths);
    app_state.set_traex_status(healthy_traex_status());

    let widget = app_state.widget_state_at(Local.with_ymd_and_hms(2026, 6, 20, 12, 0, 0).unwrap());

    assert_eq!(widget.today_total_tokens, 0);
    assert_eq!(widget.latest_turn_delta_tokens, 0);
    assert_eq!(widget.status, UiStatus::Red);
}

#[test]
fn app_layer_does_not_own_traex_native_paths() {
    let app_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/app");
    for entry in fs::read_dir(app_dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|value| value.to_str()) != Some("rs") {
            continue;
        }
        let content = fs::read_to_string(&path).unwrap();
        assert!(
            !content.contains("\".trae\"")
                && !content.contains("\"~/.trae\"")
                && !content.contains("traecli.toml"),
            "{} contains Traex-native path knowledge",
            path.display()
        );
    }
}

fn healthy_traex_status() -> TraexStatus {
    TraexStatus {
        hook_installed: true,
        hook_executable_exists: true,
        hook_smoke_test_passed: true,
        sessions_readable: true,
        archived_sessions_readable: true,
        ..TraexStatus::default()
    }
}

fn codex_status_source_with_installed_hook(root: &std::path::Path) -> (CodexStatusSource, PathBuf) {
    let config = root.join("hooks.json");
    let hook_log = root.join("hook.log");
    let sessions_dir = root.join("sessions");
    let archived_sessions_dir = root.join("archived_sessions");
    fs::create_dir_all(&sessions_dir).unwrap();
    fs::create_dir_all(&archived_sessions_dir).unwrap();
    let hook_path = root
        .join("TokenFire.app")
        .join("Contents/MacOS/token-fire-hook");
    fs::write(
        &config,
        format!(
            r#"{{
  "hooks": {{
    "Stop": [
      {{
        "hooks": [
          {{
            "type": "command",
            "command": "'{}' --source codex --owner token-fire"
          }}
        ]
      }}
    ]
  }}
}}"#,
            hook_path.display()
        ),
    )
    .unwrap();
    (
        CodexStatusSource::new_with_hook_log(
            config,
            hook_log,
            SourcePaths::new(TokenSourceKind::Codex, sessions_dir, archived_sessions_dir),
        ),
        hook_path,
    )
}

#[cfg(unix)]
fn make_executable(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

#[cfg(not(unix))]
fn make_executable(_path: &std::path::Path) {}
