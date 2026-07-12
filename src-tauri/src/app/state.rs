use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::Mutex;

use chrono::{DateTime, Local, Utc};

use crate::adapters::claude::hook_config::ClaudeHookConfigManager;
use crate::adapters::codex::hook_config::CodexHookConfigManager;
use crate::adapters::codex::status::CodexStatusSource;
use crate::adapters::cursor::hook_config::CursorHookConfigManager;
use crate::adapters::source::{SourceHookStatus, SourceStatus, TokenSourceKind};
use crate::adapters::traex::hook_config::HookConfigManager;
use crate::adapters::traex::status::{TraexStatus, TraexStatusSource};
use crate::app::debug_bundle::{
    create_debug_bundle_with_source_statuses_and_runtime_health, RuntimeHealth,
};
use crate::app::floating_widget::WidgetState;
use crate::app::logging::{write_jsonl_event, DebugLogGate, RuntimeLogSinks};
use crate::app::paths::RuntimePaths;
use crate::app::runtime::token_fire_hook_path_from_exe;
use crate::app::source_diagnostics::{
    build_source_diagnostics_snapshot, SourceDiagnosticsInput, SourceDiagnosticsSnapshot,
};
use crate::app::source_signals::RecentSourceSignals;
use crate::app::status::{ui_status_from_sources, UiStatus};
use crate::app::tracking::TrackingGate;
use crate::core::pricing::WidgetCostSummary;
use crate::core::profile::{ProfilePeriod, ProfileSummary};
use crate::core::usage_series::WidgetUsageSeries;
use crate::core::usage_store::UsageStore;

pub struct AppState {
    paths: RuntimePaths,
    hook_managers: SourceHookManagers,
    traex_status_source: Option<TraexStatusSource>,
    codex_status_source: Option<CodexStatusSource>,
    traex_status: Mutex<TraexStatus>,
    socket_ok: Arc<AtomicBool>,
    watcher_ok: Arc<AtomicBool>,
    sqlite_ok: Arc<AtomicBool>,
    last_unverified_hook_last_seen_at: Mutex<Option<DateTime<chrono::Utc>>>,
    tracking_gate: TrackingGate,
    debug_log_gate: DebugLogGate,
    recent_source_signals: RecentSourceSignals,
}

#[derive(Clone)]
pub struct SourceHookManagers {
    traex: HookConfigManager,
    codex: CodexHookConfigManager,
    claude: ClaudeHookConfigManager,
    cursor: CursorHookConfigManager,
}

impl SourceHookManagers {
    pub fn new(
        traex: HookConfigManager,
        codex: CodexHookConfigManager,
        claude: ClaudeHookConfigManager,
        cursor: CursorHookConfigManager,
    ) -> Self {
        Self {
            traex,
            codex,
            claude,
            cursor,
        }
    }

    pub fn new_for_default_config(backups_dir: PathBuf) -> anyhow::Result<Self> {
        Ok(Self::new(
            HookConfigManager::new_for_default_config(backups_dir.clone())?,
            CodexHookConfigManager::new_for_default_config(backups_dir.clone())?,
            ClaudeHookConfigManager::new_for_default_config(backups_dir.clone())?,
            CursorHookConfigManager::new_for_default_config(backups_dir)?,
        ))
    }

    pub fn traex(&self) -> &HookConfigManager {
        &self.traex
    }

    pub fn codex(&self) -> &CodexHookConfigManager {
        &self.codex
    }

    pub fn claude(&self) -> &ClaudeHookConfigManager {
        &self.claude
    }

    pub fn cursor(&self) -> &CursorHookConfigManager {
        &self.cursor
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    ToggleSourceHook(TokenSourceKind),
    OpenSourceDiagnostics,
    InstallHook,
    UninstallHook,
    InstallTraexHook,
    UninstallTraexHook,
    InstallCodexHook,
    UninstallCodexHook,
    PauseTracking,
    ResumeTracking,
    OpenLogs,
    CopyDebugBundle,
    EnableDebugLogging,
    Quit,
}

impl MenuAction {
    pub fn to_menu_id(self) -> &'static str {
        match self {
            MenuAction::ToggleSourceHook(TokenSourceKind::Traex) => "toggle_source_traex_hook",
            MenuAction::ToggleSourceHook(TokenSourceKind::Codex) => "toggle_source_codex_hook",
            MenuAction::ToggleSourceHook(TokenSourceKind::Claude) => "toggle_source_claude_hook",
            MenuAction::ToggleSourceHook(TokenSourceKind::Cursor) => "toggle_source_cursor_hook",
            MenuAction::OpenSourceDiagnostics => "open_source_diagnostics",
            MenuAction::InstallHook | MenuAction::InstallTraexHook => "install_traex_hook",
            MenuAction::UninstallHook | MenuAction::UninstallTraexHook => "uninstall_traex_hook",
            MenuAction::InstallCodexHook => "install_codex_hook",
            MenuAction::UninstallCodexHook => "uninstall_codex_hook",
            MenuAction::PauseTracking => "pause_tracking",
            MenuAction::ResumeTracking => "resume_tracking",
            MenuAction::OpenLogs => "open_logs",
            MenuAction::CopyDebugBundle => "copy_debug_bundle",
            MenuAction::EnableDebugLogging => "enable_debug_logging",
            MenuAction::Quit => "quit",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuActionOutcome {
    Handled,
    LogsDirectoryRequested(PathBuf),
    TrackingPaused,
    TrackingResumed,
    DebugLoggingEnabled,
    DebugBundleCreated(PathBuf),
    QuitRequested,
}

#[derive(Clone)]
pub struct RuntimeHealthReporter {
    socket_ok: Arc<AtomicBool>,
    watcher_ok: Arc<AtomicBool>,
    sqlite_ok: Arc<AtomicBool>,
}

impl RuntimeHealthReporter {
    pub fn set_socket_ok(&self, ok: bool) {
        self.socket_ok.store(ok, Ordering::Relaxed);
    }

    pub fn set_watcher_ok(&self, ok: bool) {
        self.watcher_ok.store(ok, Ordering::Relaxed);
    }

    pub fn set_sqlite_ok(&self, ok: bool) {
        self.sqlite_ok.store(ok, Ordering::Relaxed);
    }

    pub fn record_successful_ingest(&self) {
        self.watcher_ok.store(true, Ordering::Relaxed);
        self.sqlite_ok.store(true, Ordering::Relaxed);
    }
}

impl AppState {
    pub fn new(paths: RuntimePaths) -> Self {
        let hook_managers = SourceHookManagers::new_for_default_config(paths.backups_dir.clone())
            .expect("source hook config paths");
        Self::new_with_source_hook_managers(paths, hook_managers)
    }

    pub fn new_with_hook_config_manager(
        paths: RuntimePaths,
        hook_config_manager: HookConfigManager,
    ) -> Self {
        Self::new_with_hook_config_manager_and_tracking_gate(
            paths,
            hook_config_manager,
            TrackingGate::new(),
        )
    }

    pub fn new_with_source_hook_managers(
        paths: RuntimePaths,
        hook_managers: SourceHookManagers,
    ) -> Self {
        Self::new_with_source_hook_managers_gates_and_source_statuses(
            paths,
            hook_managers,
            TrackingGate::new(),
            DebugLogGate::default(),
            None,
            None,
        )
    }

    pub fn new_with_hook_config_manager_and_status_source(
        paths: RuntimePaths,
        hook_config_manager: HookConfigManager,
        traex_status_source: TraexStatusSource,
    ) -> Self {
        Self::new_with_hook_config_manager_gates_and_status_source(
            paths,
            hook_config_manager,
            TrackingGate::new(),
            DebugLogGate::default(),
            Some(traex_status_source),
            None,
        )
    }

    pub fn new_with_hook_config_manager_source_statuses(
        paths: RuntimePaths,
        hook_config_manager: HookConfigManager,
        traex_status_source: Option<TraexStatusSource>,
        codex_status_source: Option<CodexStatusSource>,
    ) -> Self {
        Self::new_with_hook_config_manager_gates_and_source_statuses(
            paths,
            hook_config_manager,
            TrackingGate::new(),
            DebugLogGate::default(),
            traex_status_source,
            codex_status_source,
        )
    }

    pub fn new_with_hook_config_manager_gates_and_source_statuses(
        paths: RuntimePaths,
        hook_config_manager: HookConfigManager,
        tracking_gate: TrackingGate,
        debug_log_gate: DebugLogGate,
        traex_status_source: Option<TraexStatusSource>,
        codex_status_source: Option<CodexStatusSource>,
    ) -> Self {
        let hook_managers =
            source_hook_managers_with_traex(hook_config_manager, paths.backups_dir.clone())
                .expect("source hook config paths");
        Self::new_with_source_hook_managers_gates_and_source_statuses(
            paths,
            hook_managers,
            tracking_gate,
            debug_log_gate,
            traex_status_source,
            codex_status_source,
        )
    }

    pub fn new_with_tracking_gate(paths: RuntimePaths, tracking_gate: TrackingGate) -> Self {
        let hook_managers = SourceHookManagers::new_for_default_config(paths.backups_dir.clone())
            .expect("source hook config paths");
        Self::new_with_source_hook_managers_gates_and_source_statuses(
            paths,
            hook_managers,
            tracking_gate,
            DebugLogGate::default(),
            None,
            None,
        )
    }

    pub fn new_with_gates(
        paths: RuntimePaths,
        tracking_gate: TrackingGate,
        debug_log_gate: DebugLogGate,
    ) -> Self {
        Self::new_with_runtime_gates(paths, tracking_gate, debug_log_gate)
    }

    pub fn new_with_runtime_gates(
        paths: RuntimePaths,
        tracking_gate: TrackingGate,
        debug_log_gate: DebugLogGate,
    ) -> Self {
        let hook_managers = SourceHookManagers::new_for_default_config(paths.backups_dir.clone())
            .expect("source hook config paths");
        Self::new_with_source_hook_managers_gates_and_source_statuses(
            paths,
            hook_managers,
            tracking_gate,
            debug_log_gate,
            None,
            None,
        )
    }

    pub fn new_with_runtime_gates_and_status_source(
        paths: RuntimePaths,
        tracking_gate: TrackingGate,
        debug_log_gate: DebugLogGate,
        traex_status_source: TraexStatusSource,
    ) -> Self {
        let hook_managers = SourceHookManagers::new_for_default_config(paths.backups_dir.clone())
            .expect("source hook config paths");
        Self::new_with_source_hook_managers_gates_and_source_statuses(
            paths,
            hook_managers,
            tracking_gate,
            debug_log_gate,
            Some(traex_status_source),
            None,
        )
    }

    pub fn new_with_hook_config_manager_and_tracking_gate(
        paths: RuntimePaths,
        hook_config_manager: HookConfigManager,
        tracking_gate: TrackingGate,
    ) -> Self {
        Self::new_with_hook_config_manager_and_gates(
            paths,
            hook_config_manager,
            tracking_gate,
            DebugLogGate::default(),
        )
    }

    pub fn new_with_hook_config_manager_and_gates(
        paths: RuntimePaths,
        hook_config_manager: HookConfigManager,
        tracking_gate: TrackingGate,
        debug_log_gate: DebugLogGate,
    ) -> Self {
        Self::new_with_hook_config_manager_gates_and_status_source(
            paths,
            hook_config_manager,
            tracking_gate,
            debug_log_gate,
            None,
            None,
        )
    }

    pub fn set_traex_status(&self, status: TraexStatus) {
        *self.traex_status.lock().expect("traex status lock") = status;
    }

    pub fn set_socket_ok(&self, socket_ok: bool) {
        self.socket_ok.store(socket_ok, Ordering::Relaxed);
    }

    pub fn socket_ok(&self) -> bool {
        self.socket_ok.load(Ordering::Relaxed)
    }

    pub fn set_watcher_ok(&self, watcher_ok: bool) {
        self.watcher_ok.store(watcher_ok, Ordering::Relaxed);
    }

    pub fn watcher_ok(&self) -> bool {
        self.watcher_ok.load(Ordering::Relaxed)
    }

    pub fn runtime_health(&self) -> RuntimeHealth {
        RuntimeHealth {
            socket_ok: self.socket_ok(),
            watcher_ok: self.watcher_ok(),
        }
    }

    pub fn runtime_health_reporter(&self) -> RuntimeHealthReporter {
        RuntimeHealthReporter {
            socket_ok: self.socket_ok.clone(),
            watcher_ok: self.watcher_ok.clone(),
            sqlite_ok: self.sqlite_ok.clone(),
        }
    }

    pub fn recent_source_signals(&self) -> RecentSourceSignals {
        self.recent_source_signals.clone()
    }

    pub fn refresh_traex_status(&self) -> TraexStatus {
        if let Some(source) = &self.traex_status_source {
            let status = source.collect();
            self.set_traex_status(status.clone());
            self.log_unverified_hook_execution(&status);
            return status;
        }
        let status = self.traex_status.lock().expect("traex status lock").clone();
        self.log_unverified_hook_execution(&status);
        status
    }

    pub fn tracking_gate(&self) -> TrackingGate {
        self.tracking_gate.clone()
    }

    pub fn logs_dir(&self) -> anyhow::Result<PathBuf> {
        std::fs::create_dir_all(&self.paths.logs_dir)?;
        Ok(self.paths.logs_dir.clone())
    }

    pub fn widget_state_at(&self, now: DateTime<Local>) -> WidgetState {
        let store = match UsageStore::open(&self.paths.database) {
            Ok(store) => store,
            Err(_) => return WidgetState::new(0, 0, UiStatus::Red),
        };
        let today_total = store.today_total(now);
        let latest_turn_delta = store.latest_turn_delta();
        let state_revision = store.state_revision();
        let last_observed_at = store.last_observed_at();
        let sqlite_ok = self.sqlite_ok.load(Ordering::Relaxed)
            && today_total.is_ok()
            && latest_turn_delta.is_ok()
            && state_revision.is_ok()
            && last_observed_at.is_ok();
        let today_total_tokens = today_total.unwrap_or(0);
        let latest_turn_delta_tokens = latest_turn_delta.unwrap_or(0);
        let state_revision = state_revision.unwrap_or(0);
        let last_observed_at = last_observed_at.unwrap_or(None);
        let source_statuses = self.refresh_source_statuses();
        WidgetState::new_with_revision(
            today_total_tokens,
            latest_turn_delta_tokens,
            ui_status_from_sources(
                &source_statuses,
                self.socket_ok.load(Ordering::Relaxed),
                self.watcher_ok(),
                sqlite_ok,
            ),
            state_revision,
            last_observed_at,
        )
    }

    pub fn widget_usage_series_at(&self, now: DateTime<Utc>) -> anyhow::Result<WidgetUsageSeries> {
        let store = UsageStore::open(&self.paths.database)?;
        store.usage_series_at(now)
    }

    pub fn widget_cost_summary_at(
        &self,
        now_utc: DateTime<Utc>,
        now_local: DateTime<Local>,
    ) -> anyhow::Result<WidgetCostSummary> {
        let store = UsageStore::open(&self.paths.database)?;
        store.widget_cost_summary_at(now_utc, now_local)
    }

    pub fn profile_summary_at(
        &self,
        period: ProfilePeriod,
        now_utc: DateTime<Utc>,
        now_local: DateTime<Local>,
    ) -> anyhow::Result<ProfileSummary> {
        let store = UsageStore::open(&self.paths.database)?;
        store.profile_summary_at(period, now_utc, now_local)
    }

    /// owned database path，供 async profile command 交给 spawn_blocking，
    /// 避免把借用的 `State<'_, AppState>` 跨越异步边界。
    pub fn profile_database_path(&self) -> PathBuf {
        self.paths.database.clone()
    }

    /// clone 出诊断日志 sink（owned），供 async profile command 在 await 返回后记录，
    /// 不进入 spawn_blocking closure。
    pub fn profile_log_sinks(&self) -> RuntimeLogSinks {
        RuntimeLogSinks::new(self.paths.clone(), self.debug_log_gate.clone())
    }

    pub fn handle_menu_action(&self, action: MenuAction) -> anyhow::Result<MenuActionOutcome> {
        match action {
            MenuAction::ToggleSourceHook(source) => {
                self.toggle_source_hook(source)?;
                Ok(MenuActionOutcome::Handled)
            }
            MenuAction::OpenSourceDiagnostics => Ok(MenuActionOutcome::Handled),
            MenuAction::InstallHook | MenuAction::InstallTraexHook => {
                self.hook_managers
                    .traex()
                    .install(&token_fire_hook_path()?)?;
                self.refresh_traex_status();
                Ok(MenuActionOutcome::Handled)
            }
            MenuAction::UninstallHook | MenuAction::UninstallTraexHook => {
                self.hook_managers.traex().uninstall()?;
                self.refresh_traex_status();
                Ok(MenuActionOutcome::Handled)
            }
            MenuAction::InstallCodexHook => {
                self.hook_managers
                    .codex()
                    .install(&token_fire_hook_path()?)?;
                Ok(MenuActionOutcome::Handled)
            }
            MenuAction::UninstallCodexHook => {
                self.hook_managers.codex().uninstall()?;
                Ok(MenuActionOutcome::Handled)
            }
            MenuAction::PauseTracking => {
                self.pause_tracking_at(chrono::Utc::now())?;
                Ok(MenuActionOutcome::TrackingPaused)
            }
            MenuAction::ResumeTracking => {
                self.resume_tracking_at(chrono::Utc::now())?;
                Ok(MenuActionOutcome::TrackingResumed)
            }
            MenuAction::OpenLogs => Ok(MenuActionOutcome::LogsDirectoryRequested(self.logs_dir()?)),
            MenuAction::CopyDebugBundle => {
                let source_statuses = self.debug_bundle_source_statuses();
                Ok(MenuActionOutcome::DebugBundleCreated(
                    create_debug_bundle_with_source_statuses_and_runtime_health(
                        &self.paths,
                        true,
                        &source_statuses,
                        self.runtime_health(),
                    )?,
                ))
            }
            MenuAction::EnableDebugLogging => {
                self.debug_log_gate
                    .enable_debug_for_30_minutes(chrono::Utc::now());
                Ok(MenuActionOutcome::DebugLoggingEnabled)
            }
            MenuAction::Quit => Ok(MenuActionOutcome::QuitRequested),
        }
    }

    fn toggle_source_hook(&self, source: TokenSourceKind) -> anyhow::Result<()> {
        let hook_path = token_fire_hook_path()?;
        let installed = self
            .source_hook_statuses()
            .into_iter()
            .find(|status| status.source == source)
            .is_some_and(|status| status.hook_registered);
        match (source, installed) {
            (TokenSourceKind::Traex, false) => {
                self.hook_managers.traex().install(&hook_path)?;
                self.refresh_traex_status();
            }
            (TokenSourceKind::Traex, true) => {
                self.hook_managers.traex().uninstall()?;
                self.refresh_traex_status();
            }
            (TokenSourceKind::Codex, false) => {
                self.hook_managers.codex().install(&hook_path)?;
            }
            (TokenSourceKind::Codex, true) => {
                self.hook_managers.codex().uninstall()?;
            }
            (TokenSourceKind::Claude, false) => {
                self.hook_managers.claude().install(&hook_path)?;
            }
            (TokenSourceKind::Claude, true) => {
                self.hook_managers.claude().uninstall()?;
            }
            (TokenSourceKind::Cursor, false) => {
                self.hook_managers.cursor().install(&hook_path)?;
            }
            (TokenSourceKind::Cursor, true) => {
                self.hook_managers.cursor().uninstall()?;
            }
        }
        Ok(())
    }

    pub fn resume_tracking_at(&self, now: chrono::DateTime<chrono::Utc>) -> anyhow::Result<()> {
        let store = UsageStore::open(&self.paths.database)?;
        if store.active_tracking_windows()?.is_empty() {
            store.open_tracking_window(now)?;
        }
        self.tracking_gate.resume();
        Ok(())
    }

    pub fn pause_tracking_at(&self, now: chrono::DateTime<chrono::Utc>) -> anyhow::Result<()> {
        let store = UsageStore::open(&self.paths.database)?;
        store.close_tracking_window(now)?;
        self.tracking_gate.pause();
        Ok(())
    }

    pub fn source_hook_statuses(&self) -> Vec<SourceHookStatus> {
        vec![
            status_or_error(TokenSourceKind::Traex, self.hook_managers.traex().status()),
            status_or_error(TokenSourceKind::Codex, self.hook_managers.codex().status()),
            status_or_error(
                TokenSourceKind::Claude,
                self.hook_managers.claude().status(),
            ),
            status_or_error(
                TokenSourceKind::Cursor,
                self.hook_managers.cursor().status(),
            ),
        ]
    }

    pub fn source_diagnostics_snapshot_at(
        &self,
        now: DateTime<Utc>,
    ) -> anyhow::Result<SourceDiagnosticsSnapshot> {
        let mut sqlite_ok = self.sqlite_ok.load(Ordering::Relaxed);
        let latest_storage_by_source = match UsageStore::open(&self.paths.database)
            .and_then(|store| store.latest_observation_created_at_by_source(20))
        {
            Ok(latest_storage_by_source) => latest_storage_by_source,
            Err(_) => {
                sqlite_ok = false;
                Default::default()
            }
        };
        Ok(build_source_diagnostics_snapshot(SourceDiagnosticsInput {
            generated_at: now,
            hook_statuses: self.source_hook_statuses(),
            source_signal_states: self.recent_source_signals.state_snapshot(),
            latest_storage_by_source,
            sqlite_ok,
        }))
    }

    fn debug_bundle_source_statuses(&self) -> Vec<SourceStatus> {
        let mut statuses = self.refresh_source_statuses();
        for hook_status in self.source_hook_statuses() {
            if let Some(status) = statuses
                .iter_mut()
                .find(|status| status.source == hook_status.source)
            {
                merge_hook_registration_error(status, &hook_status);
            } else {
                statuses.push(source_status_from_hook_registration(&hook_status));
            }
        }
        statuses
    }

    fn refresh_source_statuses(&self) -> Vec<SourceStatus> {
        let mut statuses = vec![SourceStatus::from_traex(&self.refresh_traex_status())];
        if let Some(source) = &self.codex_status_source {
            statuses.push(source.collect());
        }
        statuses
    }

    fn new_with_hook_config_manager_gates_and_status_source(
        paths: RuntimePaths,
        hook_config_manager: HookConfigManager,
        tracking_gate: TrackingGate,
        debug_log_gate: DebugLogGate,
        traex_status_source: Option<TraexStatusSource>,
        codex_status_source: Option<CodexStatusSource>,
    ) -> Self {
        let hook_managers =
            source_hook_managers_with_traex(hook_config_manager, paths.backups_dir.clone())
                .expect("source hook config paths");
        Self::new_with_source_hook_managers_gates_and_source_statuses(
            paths,
            hook_managers,
            tracking_gate,
            debug_log_gate,
            traex_status_source,
            codex_status_source,
        )
    }

    pub fn new_with_source_hook_managers_gates_and_source_statuses(
        paths: RuntimePaths,
        hook_managers: SourceHookManagers,
        tracking_gate: TrackingGate,
        debug_log_gate: DebugLogGate,
        traex_status_source: Option<TraexStatusSource>,
        codex_status_source: Option<CodexStatusSource>,
    ) -> Self {
        Self {
            paths,
            hook_managers,
            traex_status_source,
            codex_status_source,
            traex_status: Mutex::new(TraexStatus::default()),
            socket_ok: Arc::new(AtomicBool::new(true)),
            watcher_ok: Arc::new(AtomicBool::new(false)),
            sqlite_ok: Arc::new(AtomicBool::new(true)),
            last_unverified_hook_last_seen_at: Mutex::new(None),
            tracking_gate,
            debug_log_gate,
            recent_source_signals: RecentSourceSignals::default(),
        }
    }

    fn log_unverified_hook_execution(&self, status: &TraexStatus) {
        if status.hook_last_seen_at.is_some() && !status.hook_smoke_test_passed {
            let mut last_logged = self
                .last_unverified_hook_last_seen_at
                .lock()
                .expect("last unverified hook lock");
            if *last_logged == status.hook_last_seen_at {
                return;
            }
            *last_logged = status.hook_last_seen_at;
            let _ = write_jsonl_event(
                &self.paths.app_log,
                "app",
                "warn",
                "hook_execution_unverified",
                serde_json::json!({
                    "hook_last_seen_at": status.hook_last_seen_at.map(|value| value.to_rfc3339())
                }),
            );
        } else {
            *self
                .last_unverified_hook_last_seen_at
                .lock()
                .expect("last unverified hook lock") = None;
        }
    }
}

fn token_fire_hook_path() -> anyhow::Result<PathBuf> {
    let app_exe = std::env::current_exe()?;
    Ok(token_fire_hook_path_from_exe(&app_exe))
}

fn source_hook_managers_with_traex(
    traex: HookConfigManager,
    backups_dir: PathBuf,
) -> anyhow::Result<SourceHookManagers> {
    Ok(SourceHookManagers::new(
        traex,
        CodexHookConfigManager::new_for_default_config(backups_dir.clone())?,
        ClaudeHookConfigManager::new_for_default_config(backups_dir.clone())?,
        CursorHookConfigManager::new_for_default_config(backups_dir)?,
    ))
}

fn status_or_error(
    source: TokenSourceKind,
    result: anyhow::Result<SourceHookStatus>,
) -> SourceHookStatus {
    result.unwrap_or_else(|error| SourceHookStatus {
        source,
        hook_registered: false,
        hook_executable_exists: false,
        config_detected: true,
        config_error: Some(error.to_string()),
    })
}

fn source_status_from_hook_registration(status: &SourceHookStatus) -> SourceStatus {
    SourceStatus {
        source: status.source,
        enabled: status.hook_registered,
        detected: status.config_detected,
        hook_installed: status.hook_registered,
        hook_executable_exists: status.hook_registered && status.hook_executable_exists,
        hook_smoke_test_passed: !status.hook_registered || status.hook_executable_exists,
        sessions_readable: true,
        archived_sessions_readable: true,
        last_hook_seen_at: None,
        last_hook_error: hook_registration_error(status),
    }
}

fn hook_registration_error(status: &SourceHookStatus) -> Option<String> {
    if let Some(error) = &status.config_error {
        Some(error.clone())
    } else if status.hook_registered && !status.hook_executable_exists {
        Some("registered hook executable is missing".to_string())
    } else {
        None
    }
}

fn merge_hook_registration_error(status: &mut SourceStatus, hook_status: &SourceHookStatus) {
    if let Some(error) = hook_registration_error(hook_status) {
        status.last_hook_error = match status.last_hook_error.take() {
            Some(existing) if existing == error => Some(existing),
            Some(existing) => Some(format!("{existing}; {error}")),
            None => Some(error),
        };
    }
}
