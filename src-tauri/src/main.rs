use std::sync::Mutex;

use tauri::Manager;
use tauri::State;
use token_fire::adapters::codex::hook_config::CodexHookConfigManager;
use token_fire::adapters::codex::paths::default_paths as default_codex_paths;
use token_fire::adapters::codex::status::CodexStatusSource;
use token_fire::adapters::traex::hook_config::default_config_path;
use token_fire::adapters::traex::resolver::TraexPaths;
use token_fire::adapters::traex::status::TraexStatusSource;
use token_fire::app::build_identity::{
    current_build_identity, has_version_json_arg, log_app_started, print_version_json,
    BuildIdentity,
};
use token_fire::app::logging::{DebugLogGate, RuntimeLogSinks};
use token_fire::app::paths::runtime_paths;
use token_fire::app::release_check::{
    open_release_url, start_release_check_on_startup, trusted_release_url_for_status,
    GithubReleaseHttpClient, ReleaseCheckStore, ReleaseUpdateStateStore, ReleaseUpdateStatus,
};
use token_fire::app::runtime::start_app_runtime_for_state_with_widget_events;
use token_fire::app::state::AppState;
use token_fire::app::tracking::TrackingGate;
use token_fire::app::tray::{install_tray, refresh_tray_title_from_app, start_tray_refresh_loop};
use token_fire::app::usage_invalidation::notify_usage_facts_invalidated;
use token_fire::app::widget_events::{emit_usage_fact_invalidation_events, WidgetEventEmitter};
use token_fire::core::profile::{ProfilePeriod, ProfileSummary};

#[tauri::command]
fn profile_summary(
    period: ProfilePeriod,
    state: State<'_, AppState>,
) -> Result<ProfileSummary, String> {
    let now_utc = chrono::Utc::now();
    let now_local = now_utc.with_timezone(&chrono::Local);
    state
        .profile_summary_at(period, now_utc, now_local)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn build_identity() -> BuildIdentity {
    current_build_identity()
}

#[tauri::command]
fn release_update_status(state: State<'_, ReleaseUpdateStateStore>) -> ReleaseUpdateStatus {
    state.get()
}

#[tauri::command]
fn open_latest_release(state: State<'_, ReleaseUpdateStateStore>) -> Result<(), String> {
    let url = trusted_release_url_for_status(&state.get());
    open_release_url(&url).map_err(|error| error.to_string())
}

fn main() {
    let build_identity_value = current_build_identity();
    if has_version_json_arg() {
        print_version_json(&build_identity_value).expect("failed to print TokenFire version JSON");
        return;
    }

    let paths = runtime_paths().expect("runtime paths");
    let _ = log_app_started(&paths, &build_identity_value);
    let traex_paths = TraexPaths::default_for_home().expect("Traex paths");
    let tracking_gate = TrackingGate::new();
    let debug_gate = DebugLogGate::default();
    let status_source = TraexStatusSource::new(
        default_config_path().expect("Traex config path"),
        paths.hook_log.clone(),
        traex_paths.clone(),
    );
    let codex_status_source = default_codex_paths().ok().and_then(|codex_paths| {
        CodexHookConfigManager::new_for_default_config(paths.backups_dir.clone())
            .ok()
            .map(|manager| {
                CodexStatusSource::new_with_hook_log(
                    manager.config_path().to_path_buf(),
                    paths.hook_log.clone(),
                    codex_paths,
                )
            })
    });
    let hook_config_manager =
        token_fire::adapters::traex::hook_config::HookConfigManager::new_for_default_config(
            paths.backups_dir.clone(),
        )
        .expect("Traex hook config path");
    let app_state = AppState::new_with_hook_config_manager_gates_and_source_statuses(
        paths.clone(),
        hook_config_manager,
        tracking_gate.clone(),
        debug_gate.clone(),
        Some(status_source),
        codex_status_source,
    );
    app_state.refresh_traex_status();
    let runtime_paths = paths.clone();
    let runtime_traex_paths = traex_paths.clone();
    let runtime_tracking_gate = tracking_gate.clone();
    let runtime_debug_gate = debug_gate.clone();
    let release_update_state = ReleaseUpdateStateStore::default();
    let release_check_store = ReleaseCheckStore::new(paths.home.join("release-check.json"));
    let release_checker = token_fire::app::release_check::ReleaseChecker::new(
        release_check_store,
        GithubReleaseHttpClient::default(),
        RuntimeLogSinks::new(paths.clone(), debug_gate.clone()),
    );
    let release_build_identity = build_identity_value.clone();

    tauri::Builder::default()
        .manage(app_state)
        .manage(release_update_state.clone())
        .invoke_handler(tauri::generate_handler![
            profile_summary,
            build_identity,
            release_update_status,
            open_latest_release
        ])
        .setup(move |app| {
            install_tray(app.handle())?;
            start_release_check_on_startup(
                app.handle().clone(),
                release_update_state.clone(),
                release_checker.clone(),
                release_build_identity.clone(),
            );
            let refresh_handle = start_tray_refresh_loop(app.handle().clone());
            app.manage(Mutex::new(refresh_handle));
            let state = app.state::<AppState>();
            let app_handle = app.handle().clone();
            let widget_events = WidgetEventEmitter::from_fn(move |payload| {
                notify_usage_facts_invalidated(
                    &payload,
                    |event_payload| emit_usage_fact_invalidation_events(&app_handle, event_payload),
                    || refresh_tray_title_from_app(&app_handle),
                );
            });
            let runtime = start_app_runtime_for_state_with_widget_events(
                &state,
                runtime_paths.clone(),
                runtime_traex_paths.clone(),
                runtime_tracking_gate.clone(),
                runtime_debug_gate.clone(),
                widget_events,
            );
            let _ = refresh_tray_title_from_app(app.handle());
            if let Some(runtime) = runtime {
                app.manage(Mutex::new(runtime));
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == "main" {
                if let tauri::WindowEvent::Focused(false) = event {
                    let _ = window.hide();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("failed to run TokenFire");
}
