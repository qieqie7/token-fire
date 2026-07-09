use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime};

use chrono::{DateTime, Days, Local, Utc};
use notify::RecommendedWatcher;
use serde_json::json;
use walkdir::WalkDir;

use crate::adapters::source::{SourceContext, SourcePaths, SourceRegistry, TokenSourceKind};
use crate::adapters::traex::resolver::{is_allowed_transcript_candidate_for_source, TraexPaths};
use crate::adapters::traex::watcher::{watch_source_paths, SourceFileEvent};
use crate::adapters::transcript::TranscriptParser;
use crate::adapters::HookMetadata;
use crate::app::ingest_scheduler::{IngestReport, IngestScheduler};
use crate::app::logging::{append_app_log, DebugLogGate, LogFile, RuntimeLogSinks, RuntimeLogger};
use crate::app::paths::RuntimePaths;
use crate::app::socket_server::SocketServer;
use crate::app::source_ingest::{source_error_kind, SourceIngestPaths, SourceIngestRouter};
use crate::app::state::{AppState, RuntimeHealthReporter};
use crate::app::tracking::TrackingGate;
use crate::app::widget_events::WidgetEventEmitter;
use crate::core::usage_store::{
    RetentionOutcome, RetentionPolicy, RetentionSkipReason, UsageStore,
};

#[derive(Debug, Clone)]
pub enum RuntimeEvent {
    Hook {
        source: TokenSourceKind,
        metadata: HookMetadata,
    },
    TranscriptChanged {
        source: TokenSourceKind,
        path: PathBuf,
    },
}

pub struct AppRuntime {
    database: PathBuf,
    _tracking_gate: TrackingGate,
    _socket_server: SocketServer,
    _watchers: Vec<RecommendedWatcher>,
    poller_running: Arc<AtomicBool>,
    _hook_bridge: JoinHandle<()>,
    _watch_bridge: JoinHandle<()>,
    _poll_bridge: JoinHandle<()>,
    _worker: JoinHandle<()>,
    _widget_events: WidgetEventEmitter,
}

const RECENT_SOURCE_FILE_POLL_INTERVAL: Duration = Duration::from_secs(2);
const RECENT_SOURCE_FILE_POLL_LOOKBACK: Duration = Duration::from_secs(30 * 60);
const RECENT_SOURCE_FILE_POLL_DAYS: u64 = 7;

impl AppRuntime {
    pub fn start(
        paths: RuntimePaths,
        traex_paths: TraexPaths,
        tracking_gate: TrackingGate,
        debug_gate: DebugLogGate,
    ) -> anyhow::Result<Self> {
        Self::start_with_logger(
            paths.clone(),
            traex_paths,
            tracking_gate,
            RuntimeLogger::new(paths, debug_gate),
        )
    }

    pub fn start_with_logger(
        paths: RuntimePaths,
        traex_paths: TraexPaths,
        tracking_gate: TrackingGate,
        logger: RuntimeLogger,
    ) -> anyhow::Result<Self> {
        Self::start_with_logger_and_health(paths, traex_paths, tracking_gate, logger, None)
    }

    pub fn start_with_sources(
        paths: RuntimePaths,
        source_registry: SourceRegistry,
        tracking_gate: TrackingGate,
        debug_gate: DebugLogGate,
    ) -> anyhow::Result<Self> {
        Self::start_with_sources_and_logger(
            paths.clone(),
            source_registry,
            tracking_gate,
            RuntimeLogger::new(paths, debug_gate),
        )
    }

    fn start_with_sources_and_logger(
        paths: RuntimePaths,
        source_registry: SourceRegistry,
        tracking_gate: TrackingGate,
        logger: RuntimeLogger,
    ) -> anyhow::Result<Self> {
        Self::start_with_sources_logger_and_health(
            paths,
            source_registry,
            tracking_gate,
            logger,
            None,
        )
    }

    fn start_with_logger_and_health(
        paths: RuntimePaths,
        traex_paths: TraexPaths,
        tracking_gate: TrackingGate,
        logger: RuntimeLogger,
        health_reporter: Option<RuntimeHealthReporter>,
    ) -> anyhow::Result<Self> {
        Self::start_with_sources_logger_and_health(
            paths,
            SourceRegistry::new(vec![SourcePaths::from(&traex_paths)]),
            tracking_gate,
            logger,
            health_reporter,
        )
    }

    fn start_with_sources_logger_and_health(
        paths: RuntimePaths,
        source_registry: SourceRegistry,
        tracking_gate: TrackingGate,
        logger: RuntimeLogger,
        health_reporter: Option<RuntimeHealthReporter>,
    ) -> anyhow::Result<Self> {
        Self::start_with_sources_logger_health_and_widget_events(
            paths,
            source_registry,
            tracking_gate,
            logger,
            health_reporter,
            WidgetEventEmitter::noop(),
        )
    }

    fn start_with_sources_logger_health_and_widget_events(
        paths: RuntimePaths,
        source_registry: SourceRegistry,
        tracking_gate: TrackingGate,
        logger: RuntimeLogger,
        health_reporter: Option<RuntimeHealthReporter>,
        widget_events: WidgetEventEmitter,
    ) -> anyhow::Result<Self> {
        let (hook_tx, hook_rx) = mpsc::channel::<HookMetadata>();
        let (source_path_tx, source_path_rx) = mpsc::channel::<SourceFileEvent>();
        let (event_tx, event_rx) = mpsc::channel::<RuntimeEvent>();
        let active_sources = source_registry.active_sources();

        let debug_gate = logger.gate();
        let ((socket_server, watchers), mut store) =
            activate_runtime_tracking_window(&paths.database, Utc::now, |_| {
                let socket_server =
                    SocketServer::start_with_logger(paths.socket.clone(), hook_tx, logger.clone())?;
                let mut watchers = Vec::new();
                for source in active_sources.clone() {
                    watchers.push(watch_source_paths(source, source_path_tx.clone())?);
                }
                Ok((socket_server, watchers))
            })?;
        let log_sinks = RuntimeLogSinks::new(paths.clone(), debug_gate);
        run_startup_retention(
            &mut store,
            &log_sinks,
            Utc::now(),
            RetentionPolicy::default(),
        );
        let codex_sources = active_sources
            .iter()
            .filter(|source| source.kind == TokenSourceKind::Codex)
            .cloned()
            .collect::<Vec<_>>();
        baseline_existing_source_files_with_logs(&store, &codex_sources, &log_sinks)?;
        let scheduler = IngestScheduler::new_with_logs(store, log_sinks);
        tracking_gate.resume();

        let hook_event_tx = event_tx.clone();
        let hook_logger = logger.clone();
        let hook_bridge = thread::spawn(move || {
            for metadata in hook_rx {
                let Some(source) = parse_hook_source(metadata.source.as_deref()) else {
                    let _ = append_app_log(
                        &hook_logger,
                        "warn",
                        "hook_source_unknown",
                        serde_json::json!({ "source_present": metadata.source.is_some() }),
                    );
                    continue;
                };
                if hook_event_tx
                    .send(RuntimeEvent::Hook { source, metadata })
                    .is_err()
                {
                    break;
                }
            }
        });

        let watch_event_tx = event_tx.clone();
        let watch_bridge = thread::spawn(move || {
            for event in source_path_rx {
                if watch_event_tx
                    .send(RuntimeEvent::TranscriptChanged {
                        source: event.source,
                        path: event.path,
                    })
                    .is_err()
                {
                    break;
                }
            }
        });

        let poller_running = Arc::new(AtomicBool::new(true));
        let poll_bridge = spawn_recent_source_file_poller(
            active_sources.clone(),
            event_tx.clone(),
            Arc::clone(&poller_running),
        );

        let worker_registry = source_registry.clone();
        let worker_tracking_gate = tracking_gate.clone();
        let worker_widget_events = widget_events.clone();
        let worker = thread::spawn(move || {
            for event in event_rx {
                match handle_runtime_event_with_logger_and_widget_events(
                    event.clone(),
                    &worker_registry,
                    &scheduler,
                    &worker_tracking_gate,
                    &logger,
                    &worker_widget_events,
                ) {
                    Ok(Some(_)) => {
                        if let Some(health_reporter) = &health_reporter {
                            health_reporter.record_successful_ingest();
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        if let Some(health_reporter) = &health_reporter {
                            match runtime_event_error_kind(&event, &error) {
                                "sqlite_write_failed" => health_reporter.set_sqlite_ok(false),
                                "transcript_read_failed"
                                | "transcript_ingest_failed"
                                | "transcript_unreadable"
                                | "transcript_parse_failed"
                                | "watermark_write_failed"
                                | "source_resolver_failed"
                                | "source_adapter_failed" => health_reporter.set_watcher_ok(false),
                                _ => {}
                            }
                        }
                        log_runtime_event_failure(&logger, &event, &error);
                    }
                }
            }
        });

        Ok(Self {
            database: paths.database,
            _tracking_gate: tracking_gate,
            _socket_server: socket_server,
            _watchers: watchers,
            poller_running,
            _hook_bridge: hook_bridge,
            _watch_bridge: watch_bridge,
            _poll_bridge: poll_bridge,
            _worker: worker,
            _widget_events: widget_events,
        })
    }
}

pub fn parse_hook_source(source: Option<&str>) -> Option<TokenSourceKind> {
    match source {
        Some("traex") => Some(TokenSourceKind::Traex),
        Some("codex") => Some(TokenSourceKind::Codex),
        Some("claude") => Some(TokenSourceKind::Claude),
        Some("cursor") => Some(TokenSourceKind::Cursor),
        _ => None,
    }
}

pub fn baseline_existing_source_files(
    store: &UsageStore,
    sources: &[SourcePaths],
) -> anyhow::Result<()> {
    baseline_existing_source_files_inner(store, sources, None)
}

fn baseline_existing_source_files_with_logs(
    store: &UsageStore,
    sources: &[SourcePaths],
    sinks: &RuntimeLogSinks,
) -> anyhow::Result<()> {
    baseline_existing_source_files_inner(store, sources, Some(sinks))
}

fn baseline_existing_source_files_inner(
    store: &UsageStore,
    sources: &[SourcePaths],
    sinks: Option<&RuntimeLogSinks>,
) -> anyhow::Result<()> {
    for source in sources {
        for root in [&source.sessions_dir, &source.archived_sessions_dir] {
            if !root.exists() {
                continue;
            }
            for entry in walkdir::WalkDir::new(root)
                .follow_links(false)
                .into_iter()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().is_file())
            {
                let path = entry.path();
                if path.extension().and_then(|value| value.to_str()) != Some("jsonl") {
                    continue;
                }
                let source_path = path.to_string_lossy().to_string();
                if store.file_baseline(&source_path)?.is_none() {
                    let content = match std::fs::read_to_string(path) {
                        Ok(content) => content,
                        Err(error) => {
                            log_startup_baseline_skip(sinks, source.kind, path, &error);
                            continue;
                        }
                    };
                    let report = match TranscriptParser::new(SourceContext::new(source.kind))
                        .parse_str(path, &content)
                    {
                        Ok(report) => report,
                        Err(error) => {
                            log_startup_baseline_skip(sinks, source.kind, path, error.as_ref());
                            continue;
                        }
                    };
                    store.set_file_baseline(&source_path, report.safe_processed_offset)?;
                }
            }
        }
    }
    Ok(())
}

fn spawn_recent_source_file_poller(
    sources: Vec<SourcePaths>,
    event_tx: mpsc::Sender<RuntimeEvent>,
    running: Arc<AtomicBool>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut seen = HashMap::<PathBuf, SourceFileFingerprint>::new();
        while running.load(Ordering::Relaxed) {
            poll_recent_source_files(&sources, &event_tx, &mut seen);
            thread::sleep(RECENT_SOURCE_FILE_POLL_INTERVAL);
        }
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SourceFileFingerprint {
    modified: Option<SystemTime>,
    len: u64,
}

fn poll_recent_source_files(
    sources: &[SourcePaths],
    event_tx: &mpsc::Sender<RuntimeEvent>,
    seen: &mut HashMap<PathBuf, SourceFileFingerprint>,
) {
    let cutoff = SystemTime::now()
        .checked_sub(RECENT_SOURCE_FILE_POLL_LOOKBACK)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    for source in sources
        .iter()
        .filter(|source| matches!(source.kind, TokenSourceKind::Traex | TokenSourceKind::Codex))
    {
        for path in recent_source_files(source, cutoff) {
            let Ok(metadata) = std::fs::metadata(&path) else {
                continue;
            };
            let fingerprint = SourceFileFingerprint {
                modified: metadata.modified().ok(),
                len: metadata.len(),
            };
            if seen.get(&path) == Some(&fingerprint) {
                continue;
            }
            seen.insert(path.clone(), fingerprint);
            if event_tx
                .send(RuntimeEvent::TranscriptChanged {
                    source: source.kind,
                    path,
                })
                .is_err()
            {
                return;
            }
        }
    }
}

fn recent_source_files(source: &SourcePaths, cutoff: SystemTime) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for root in recent_source_roots(source) {
        if !root.exists() {
            continue;
        }
        for entry in WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file())
        {
            let path = entry.path();
            if !is_allowed_transcript_candidate_for_source(source, path) {
                continue;
            }
            let Some(modified) = entry
                .metadata()
                .ok()
                .and_then(|metadata| metadata.modified().ok())
            else {
                continue;
            };
            if modified >= cutoff {
                files.push(path.to_path_buf());
            }
        }
    }
    files
}

fn recent_source_roots(source: &SourcePaths) -> Vec<PathBuf> {
    let today = Local::now().date_naive();
    (0..RECENT_SOURCE_FILE_POLL_DAYS)
        .filter_map(|days_ago| today.checked_sub_days(Days::new(days_ago)))
        .map(|day| {
            source
                .sessions_dir
                .join(day.format("%Y").to_string())
                .join(day.format("%m").to_string())
                .join(day.format("%d").to_string())
        })
        .filter(|path| path.exists())
        .collect()
}

fn log_startup_baseline_skip(
    sinks: Option<&RuntimeLogSinks>,
    source: TokenSourceKind,
    _path: &Path,
    error: &(dyn std::error::Error + 'static),
) {
    let Some(sinks) = sinks else {
        return;
    };
    let error_kind = if error.downcast_ref::<std::io::Error>().is_some() {
        "transcript_read_failed"
    } else {
        "transcript_parse_failed"
    };
    sinks.write(
        LogFile::App,
        "app",
        "warn",
        "startup_baseline_file_skipped",
        json!({
            "source": source.as_str(),
            "source_path_present": true,
            "error_kind": error_kind
        }),
    );
}

pub fn run_startup_retention(
    store: &mut UsageStore,
    sinks: &RuntimeLogSinks,
    now: DateTime<Utc>,
    policy: RetentionPolicy,
) {
    match store.run_retention_if_due(now, policy) {
        Ok(outcome) => log_retention_outcome(sinks, policy, &outcome),
        Err(_) => {
            let _ = store.record_retention_failure(now, "sqlite_retention_failed");
            sinks.write(
                LogFile::Db,
                "db",
                "warn",
                "retention_failed",
                json!({
                    "error_kind": "sqlite_retention_failed",
                    "policy_days": policy.observation_retention_days
                }),
            );
        }
    }
}

fn log_retention_outcome(
    sinks: &RuntimeLogSinks,
    policy: RetentionPolicy,
    outcome: &RetentionOutcome,
) {
    if outcome.ran {
        sinks.write(
            LogFile::Db,
            "db",
            "info",
            "retention_completed",
            json!({
                "cutoff": outcome.cutoff.to_rfc3339(),
                "policy_days": policy.observation_retention_days,
                "deleted_observations": outcome.deleted_observations
            }),
        );
        return;
    }

    let reason = match outcome.skipped_reason {
        Some(RetentionSkipReason::RecentlySucceeded) => "recently_succeeded",
        None => "not_due",
    };
    sinks.write(
        LogFile::Db,
        "db",
        "info",
        "retention_skipped",
        json!({
            "reason": reason,
            "policy_days": policy.observation_retention_days
        }),
    );
}

pub fn start_app_runtime_for_state(
    state: &AppState,
    paths: RuntimePaths,
    traex_paths: TraexPaths,
    tracking_gate: TrackingGate,
    debug_gate: DebugLogGate,
) -> Option<AppRuntime> {
    start_app_runtime_for_state_with_widget_events(
        state,
        paths,
        traex_paths,
        tracking_gate,
        debug_gate,
        WidgetEventEmitter::noop(),
    )
}

pub fn start_app_runtime_for_state_with_widget_events(
    state: &AppState,
    paths: RuntimePaths,
    traex_paths: TraexPaths,
    tracking_gate: TrackingGate,
    debug_gate: DebugLogGate,
    widget_events: WidgetEventEmitter,
) -> Option<AppRuntime> {
    let logger = RuntimeLogger::new(paths.clone(), debug_gate);
    let mut sources = vec![SourcePaths::from(traex_paths.clone())];
    match crate::adapters::codex::paths::default_paths() {
        Ok(codex_paths) => sources.push(codex_paths),
        Err(error) => {
            let _ = append_app_log(
                &logger,
                "warn",
                "codex_source_unavailable",
                serde_json::json!({ "reason": error.to_string() }),
            );
        }
    }
    let registry = SourceRegistry::new(sources);
    match AppRuntime::start_with_sources_logger_health_and_widget_events(
        paths,
        registry,
        tracking_gate,
        logger.clone(),
        Some(state.runtime_health_reporter()),
        widget_events,
    ) {
        Ok(runtime) => {
            state.set_socket_ok(true);
            state.set_watcher_ok(true);
            Some(runtime)
        }
        Err(error) => {
            state.set_socket_ok(false);
            state.set_watcher_ok(false);
            let _ = append_app_log(
                &logger,
                "error",
                "runtime_start_failed",
                serde_json::json!({ "reason": error.to_string() }),
            );
            None
        }
    }
}

pub fn activate_runtime_tracking_window<T>(
    database: &Path,
    capture_started_at: impl FnOnce() -> DateTime<Utc>,
    initialize: impl FnOnce(DateTime<Utc>) -> anyhow::Result<T>,
) -> anyhow::Result<(T, UsageStore)> {
    let started_at = capture_started_at();
    let initialized = initialize(started_at)?;
    let store = UsageStore::open(database)?;
    store.close_tracking_window(started_at)?;
    store.open_tracking_window(started_at)?;
    Ok((initialized, store))
}

impl Drop for AppRuntime {
    fn drop(&mut self) {
        self.poller_running.store(false, Ordering::Relaxed);
        if let Ok(store) = UsageStore::open(&self.database) {
            let _ = store.close_tracking_window(Utc::now());
        }
    }
}

pub fn handle_runtime_event(
    event: RuntimeEvent,
    registry: &SourceRegistry,
    scheduler: &IngestScheduler,
    tracking_gate: &TrackingGate,
) -> anyhow::Result<Option<IngestReport>> {
    handle_runtime_event_inner(
        event,
        registry,
        scheduler,
        tracking_gate,
        None,
        SourceIngestPaths::default(),
    )
}

pub fn handle_runtime_event_with_logger(
    event: RuntimeEvent,
    registry: &SourceRegistry,
    scheduler: &IngestScheduler,
    tracking_gate: &TrackingGate,
    logger: &RuntimeLogger,
) -> anyhow::Result<Option<IngestReport>> {
    let event_kind = match &event {
        RuntimeEvent::Hook { .. } => "hook",
        RuntimeEvent::TranscriptChanged { .. } => "transcript_changed",
    };
    let _ = append_app_log(
        logger,
        "debug",
        "runtime_event_received",
        serde_json::json!({ "runtime_event": event_kind }),
    );
    handle_runtime_event_inner(
        event,
        registry,
        scheduler,
        tracking_gate,
        Some(logger),
        SourceIngestPaths::default(),
    )
}

pub fn handle_runtime_event_with_cursor_home(
    event: RuntimeEvent,
    registry: &SourceRegistry,
    scheduler: &IngestScheduler,
    tracking_gate: &TrackingGate,
    cursor_home: Option<&Path>,
    cursor_watermark_dir: Option<&Path>,
) -> anyhow::Result<Option<IngestReport>> {
    handle_runtime_event_inner(
        event,
        registry,
        scheduler,
        tracking_gate,
        None,
        SourceIngestPaths {
            cursor_home: cursor_home.map(Path::to_path_buf),
            cursor_watermark_dir: cursor_watermark_dir.map(Path::to_path_buf),
        },
    )
}

pub fn handle_runtime_event_with_logger_and_widget_events(
    event: RuntimeEvent,
    registry: &SourceRegistry,
    scheduler: &IngestScheduler,
    tracking_gate: &TrackingGate,
    logger: &RuntimeLogger,
    widget_events: &WidgetEventEmitter,
) -> anyhow::Result<Option<IngestReport>> {
    let result = handle_runtime_event_with_logger(
        event.clone(),
        registry,
        scheduler,
        tracking_gate,
        logger,
    )?;
    if let Some(report) = &result {
        if report.inserted > 0 {
            match scheduler.widget_state_changed_event(report.inserted) {
                Ok(payload) => widget_events.emit_usage_facts_invalidated(payload),
                Err(error) => log_runtime_event_failure(logger, &event, &error),
            }
        }
    }
    Ok(result)
}

pub fn log_runtime_event_failure(
    logger: &RuntimeLogger,
    event: &RuntimeEvent,
    error: &anyhow::Error,
) {
    let runtime_event = match event {
        RuntimeEvent::Hook { .. } => "hook",
        RuntimeEvent::TranscriptChanged { .. } => "transcript_changed",
    };
    let error_kind = runtime_event_error_kind(event, error);
    let _ = append_app_log(
        logger,
        "error",
        "runtime_event_failed",
        serde_json::json!({
            "runtime_event": runtime_event,
            "error_kind": error_kind
        }),
    );
}

fn runtime_event_error_kind(event: &RuntimeEvent, error: &anyhow::Error) -> &'static str {
    if let Some(kind) = source_error_kind(error) {
        return kind.as_str();
    }

    match event {
        RuntimeEvent::TranscriptChanged { .. } => {
            if error.downcast_ref::<std::io::Error>().is_some() {
                "transcript_read_failed"
            } else if error.downcast_ref::<rusqlite::Error>().is_some() {
                "sqlite_write_failed"
            } else {
                "transcript_ingest_failed"
            }
        }
        RuntimeEvent::Hook { .. } => {
            if error.downcast_ref::<rusqlite::Error>().is_some() {
                "sqlite_write_failed"
            } else {
                "hook_ingest_failed"
            }
        }
    }
}

fn handle_runtime_event_inner(
    event: RuntimeEvent,
    registry: &SourceRegistry,
    scheduler: &IngestScheduler,
    tracking_gate: &TrackingGate,
    logger: Option<&RuntimeLogger>,
    source_paths: SourceIngestPaths,
) -> anyhow::Result<Option<IngestReport>> {
    if tracking_gate.is_paused() {
        return Ok(None);
    }

    let router = SourceIngestRouter {
        registry,
        scheduler,
        logger,
        paths: source_paths,
    };

    let outcome = match event {
        RuntimeEvent::Hook { source, metadata } => router.ingest_hook(source, metadata)?,
        RuntimeEvent::TranscriptChanged { source, path } => {
            scheduler.log(
                LogFile::App,
                "app",
                "info",
                "file_changed",
                json!({ "source": source.as_str(), "source_path_present": true }),
            );
            router.ingest_file_changed(source, path)?
        }
    };

    Ok(outcome.report)
}

pub fn token_fire_hook_path_from_exe(app_exe: &Path) -> PathBuf {
    app_exe.with_file_name("token-fire-hook")
}
