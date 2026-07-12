use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Datelike, Duration, Local, Utc};
use serde::Serialize;

use crate::adapters::source::{SourceHookStatus, TokenSourceKind};
use crate::app::source_ingest::SourceEmptyReason;
use crate::app::source_signals::{SourceSignalRecord, SourceSignalState};
use crate::app::state::{MenuAction, MenuActionOutcome};

const RECENT_SIGNAL_TTL_SECONDS: i64 = 15 * 60;
const SUCCESS_TRUST_TTL_SECONDS: i64 = 6 * 60 * 60;

fn serialize_token_source_kind<S>(
    source: &TokenSourceKind,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(source.as_str())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticStageKey {
    Participation,
    Capture,
    Signal,
    Extraction,
    Storage,
}

impl DiagnosticStageKey {
    pub fn label(self) -> &'static str {
        match self {
            Self::Participation => "参与采集",
            Self::Capture => "捕获就绪",
            Self::Signal => "看到信号",
            Self::Extraction => "提取 token",
            Self::Storage => "写入统计",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticStatus {
    Ok,
    Warning,
    Error,
    Disabled,
    Unknown,
    NotApplicable,
}

impl DiagnosticStatus {
    pub fn summary_label(self) -> &'static str {
        match self {
            Self::Ok => "有证据",
            Self::Warning => "需要关注",
            Self::Error => "异常",
            Self::Disabled => "未启用",
            Self::Unknown => "未知",
            Self::NotApplicable => "不适用",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticHeadline {
    Connected,
    Disabled,
    PendingVerification,
    CaptureNotReady,
    SignalNotSeen,
    TokenNotExtracted,
    NotStored,
    ConfigurationError,
    RuntimeError,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticStage {
    pub key: DiagnosticStageKey,
    pub label: String,
    pub status: DiagnosticStatus,
    pub summary: String,
    pub evidence: Option<String>,
    pub checked_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticBreak {
    pub stage: DiagnosticStageKey,
    pub title: String,
    pub evidence: String,
    pub impact: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticEvidenceItem {
    pub label: String,
    pub value: String,
    pub status: Option<DiagnosticEvidenceStatus>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticEvidenceStatus {
    Ok,
    Warning,
    Error,
    Muted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticEvidenceGroup {
    pub title: String,
    pub items: Vec<DiagnosticEvidenceItem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticActionId {
    Refresh,
    OpenLogs,
    CopyDebugBundle,
    ReinstallHook,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticAction {
    pub id: DiagnosticActionId,
    pub label: String,
    pub enabled: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticDisplaySummary {
    pub status_text: String,
    pub detail_text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note_text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceDiagnostic {
    #[serde(serialize_with = "serialize_token_source_kind")]
    pub source: TokenSourceKind,
    pub display_name: String,
    pub optional: bool,
    pub headline: DiagnosticHeadline,
    pub display_summary: DiagnosticDisplaySummary,
    pub trust_summary: String,
    pub primary_break: Option<DiagnosticBreak>,
    pub chain: Vec<DiagnosticStage>,
    pub evidence: Vec<DiagnosticEvidenceGroup>,
    pub actions: Vec<DiagnosticAction>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticSummary {
    pub connected: usize,
    pub attention: usize,
    pub disabled: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceDiagnosticsSnapshot {
    pub generated_at: DateTime<Utc>,
    pub summary: DiagnosticSummary,
    pub sources: Vec<SourceDiagnostic>,
}

pub struct SourceDiagnosticsInput {
    pub generated_at: DateTime<Utc>,
    pub hook_statuses: Vec<SourceHookStatus>,
    pub source_signal_states: HashMap<TokenSourceKind, SourceSignalState>,
    pub latest_storage_by_source: BTreeMap<String, DateTime<Utc>>,
    pub sqlite_ok: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SuccessTrust {
    Trusted,
    Expired,
    Missing,
    OverriddenByCurrentBlocker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CurrentTrustBlocker {
    ConfigError,
    CaptureNotReady,
    StorageUnavailable,
    SignalHardFailure,
}

impl CurrentTrustBlocker {
    fn headline(self) -> DiagnosticHeadline {
        match self {
            Self::ConfigError => DiagnosticHeadline::ConfigurationError,
            Self::CaptureNotReady => DiagnosticHeadline::CaptureNotReady,
            Self::StorageUnavailable | Self::SignalHardFailure => DiagnosticHeadline::RuntimeError,
        }
    }

    fn evidence(self) -> &'static str {
        match self {
            Self::ConfigError => "config_error",
            Self::CaptureNotReady => "capture_not_ready",
            Self::StorageUnavailable => "sqlite_unavailable",
            Self::SignalHardFailure => "signal_hard_failure",
        }
    }
}

pub fn menu_action_for_diagnostic_action(action_id: &str) -> Option<MenuAction> {
    match action_id {
        "open_logs" => Some(MenuAction::OpenLogs),
        "copy_debug_bundle" => Some(MenuAction::CopyDebugBundle),
        "refresh" | "reinstall_hook" => None,
        _ => None,
    }
}

pub fn handle_diagnostic_menu_action(
    action_id: &str,
    handle_action: impl FnOnce(MenuAction) -> anyhow::Result<MenuActionOutcome>,
    handle_outcome: impl FnOnce(MenuActionOutcome) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let Some(action) = menu_action_for_diagnostic_action(action_id) else {
        anyhow::bail!("unsupported source diagnostics action: {action_id}");
    };
    let outcome = handle_action(action)?;
    handle_outcome(outcome)
}

pub fn build_source_diagnostics_snapshot(
    input: SourceDiagnosticsInput,
) -> SourceDiagnosticsSnapshot {
    let sources = TokenSourceKind::all_menu_sources()
        .into_iter()
        .map(|source| build_source_diagnostic(source, &input))
        .collect::<Vec<_>>();
    let summary = DiagnosticSummary {
        connected: sources
            .iter()
            .filter(|source| source.headline == DiagnosticHeadline::Connected)
            .count(),
        disabled: sources
            .iter()
            .filter(|source| source.headline == DiagnosticHeadline::Disabled)
            .count(),
        attention: sources
            .iter()
            .filter(|source| {
                !matches!(
                    source.headline,
                    DiagnosticHeadline::Connected | DiagnosticHeadline::Disabled
                )
            })
            .count(),
    };

    SourceDiagnosticsSnapshot {
        generated_at: input.generated_at,
        summary,
        sources,
    }
}

fn build_source_diagnostic(
    source: TokenSourceKind,
    input: &SourceDiagnosticsInput,
) -> SourceDiagnostic {
    let optional = matches!(source, TokenSourceKind::Claude | TokenSourceKind::Cursor);
    let hook = input
        .hook_statuses
        .iter()
        .find(|status| status.source == source);
    let state = input
        .source_signal_states
        .get(&source)
        .cloned()
        .unwrap_or_default();
    let latest_signal = state.latest_signal.as_ref();
    let signal = current_signal(&state, input.generated_at);
    let latest_storage = input.latest_storage_by_source.get(source.as_str());
    let signal_hard_failure = current_hard_failure(&state, latest_storage, input.generated_at);
    let participation_disabled = optional
        && hook.is_none_or(|status| {
            !status.hook_registered && !status.config_detected && status.config_error.is_none()
        })
        && signal.is_none()
        && state.latest_success.is_none();
    let blocker = current_trust_blocker(
        hook,
        signal_hard_failure,
        input.sqlite_ok,
        participation_disabled,
    );
    let trust = success_trust(&state, latest_storage, input.generated_at, blocker);
    let trusted_success = trust == SuccessTrust::Trusted;

    let chain = vec![
        build_participation_stage(
            hook,
            signal,
            trusted_success,
            blocker,
            participation_disabled,
        ),
        build_capture_stage(
            hook,
            signal,
            trusted_success,
            blocker,
            participation_disabled,
        ),
        build_signal_stage(
            signal,
            latest_signal,
            trusted_success,
            participation_disabled,
        ),
        build_extraction_stage(
            signal,
            state.latest_success.as_ref(),
            trust,
            blocker,
            participation_disabled,
        ),
        build_storage_stage(
            signal,
            state.latest_success.as_ref(),
            latest_storage,
            trust,
            input.sqlite_ok,
            blocker,
            participation_disabled,
        ),
    ];
    let headline = headline_for_trust(&chain, trust, blocker);
    let primary_break = primary_break_for_headline(headline, &chain);
    let accepted_success =
        latest_accepted_success_evidence(&state, latest_storage, input.generated_at);
    let display_summary = build_display_summary(
        headline,
        signal,
        accepted_success,
        blocker,
        input.generated_at,
    );
    let trust_summary = build_trust_summary(signal, state.latest_success.as_ref(), blocker, trust);
    let evidence = build_evidence(
        &display_summary,
        headline,
        input.generated_at,
        hook,
        signal,
        latest_signal,
        signal_hard_failure,
        latest_storage,
        trust,
        blocker,
        input.sqlite_ok,
        participation_disabled,
    );

    SourceDiagnostic {
        source,
        display_name: source.display_name().to_string(),
        optional,
        headline,
        display_summary,
        trust_summary,
        primary_break,
        chain,
        evidence,
        actions: diagnostic_actions(),
    }
}

fn within_ttl(at: DateTime<Utc>, generated_at: DateTime<Utc>, ttl_seconds: i64) -> bool {
    let age = generated_at.signed_duration_since(at);
    age >= Duration::zero() && age <= Duration::seconds(ttl_seconds)
}

fn current_signal(
    state: &SourceSignalState,
    generated_at: DateTime<Utc>,
) -> Option<&SourceSignalRecord> {
    state
        .latest_signal
        .as_ref()
        .filter(|record| within_ttl(record.seen_at, generated_at, RECENT_SIGNAL_TTL_SECONDS))
}

fn current_hard_failure<'a>(
    state: &'a SourceSignalState,
    latest_storage: Option<&DateTime<Utc>>,
    generated_at: DateTime<Utc>,
) -> Option<&'a SourceSignalRecord> {
    let latest_success_at = latest_accepted_success_at(state, latest_storage, generated_at);
    state
        .latest_hard_failure
        .as_ref()
        .filter(|record| latest_success_at.is_none_or(|success_at| record.seen_at > success_at))
        .filter(|record| within_ttl(record.seen_at, generated_at, RECENT_SIGNAL_TTL_SECONDS))
}

fn current_trust_blocker(
    hook: Option<&SourceHookStatus>,
    signal_hard_failure: Option<&SourceSignalRecord>,
    sqlite_ok: bool,
    participation_disabled: bool,
) -> Option<CurrentTrustBlocker> {
    if participation_disabled {
        return None;
    }
    if hook
        .and_then(|status| status.config_error.as_ref())
        .is_some()
    {
        return Some(CurrentTrustBlocker::ConfigError);
    }
    if !sqlite_ok {
        return Some(CurrentTrustBlocker::StorageUnavailable);
    }
    if signal_hard_failure.is_some() {
        return Some(CurrentTrustBlocker::SignalHardFailure);
    }
    if hook.is_some_and(|status| status.hook_registered && !status.hook_executable_exists) {
        return Some(CurrentTrustBlocker::CaptureNotReady);
    }
    None
}

fn success_trust(
    state: &SourceSignalState,
    latest_storage: Option<&DateTime<Utc>>,
    generated_at: DateTime<Utc>,
    blocker: Option<CurrentTrustBlocker>,
) -> SuccessTrust {
    if blocker.is_some() {
        return SuccessTrust::OverriddenByCurrentBlocker;
    }
    let accepted_success_at = latest_accepted_success_at(state, latest_storage, generated_at);
    match accepted_success_at {
        Some(at) if within_ttl(at, generated_at, SUCCESS_TRUST_TTL_SECONDS) => {
            SuccessTrust::Trusted
        }
        Some(_) => SuccessTrust::Expired,
        None => SuccessTrust::Missing,
    }
}

fn latest_accepted_success_at(
    state: &SourceSignalState,
    latest_storage: Option<&DateTime<Utc>>,
    generated_at: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    latest_accepted_success_evidence(state, latest_storage, generated_at)
        .map(AcceptedSuccessEvidence::at)
}

#[derive(Debug, Clone, Copy)]
enum AcceptedSuccessEvidence<'a> {
    Runtime(&'a SourceSignalRecord),
    Storage(DateTime<Utc>),
}

impl AcceptedSuccessEvidence<'_> {
    fn at(self) -> DateTime<Utc> {
        match self {
            Self::Runtime(record) => record.seen_at,
            Self::Storage(at) => at,
        }
    }
}

fn latest_accepted_success_evidence<'a>(
    state: &'a SourceSignalState,
    latest_storage: Option<&DateTime<Utc>>,
    generated_at: DateTime<Utc>,
) -> Option<AcceptedSuccessEvidence<'a>> {
    let runtime = state
        .latest_success
        .as_ref()
        .filter(|record| record.seen_at <= generated_at)
        .map(AcceptedSuccessEvidence::Runtime);
    let storage = latest_storage
        .copied()
        .filter(|at| *at <= generated_at)
        .map(AcceptedSuccessEvidence::Storage);

    match (runtime, storage) {
        (Some(runtime), Some(storage)) if storage.at() > runtime.at() => Some(storage),
        (Some(runtime), _) => Some(runtime),
        (None, storage) => storage,
    }
}

fn build_participation_stage(
    hook: Option<&SourceHookStatus>,
    signal: Option<&SourceSignalRecord>,
    trusted_success: bool,
    blocker: Option<CurrentTrustBlocker>,
    participation_disabled: bool,
) -> DiagnosticStage {
    let config_error = hook.and_then(|status| status.config_error.as_ref());
    let status = if matches!(blocker, Some(CurrentTrustBlocker::ConfigError)) {
        DiagnosticStatus::Error
    } else if hook.is_some_and(|status| status.hook_registered)
        || signal.is_some()
        || trusted_success
    {
        DiagnosticStatus::Ok
    } else if participation_disabled {
        DiagnosticStatus::Disabled
    } else {
        DiagnosticStatus::Unknown
    };
    let evidence = config_error
        .map(|error| sanitize_config_error_evidence(error.as_str()))
        .or_else(|| {
            hook.map(|status| {
                if status.hook_registered {
                    "hook 已注册".to_string()
                } else if status.config_detected {
                    "配置文件存在但未注册 hook".to_string()
                } else {
                    "未检测到 hook 配置".to_string()
                }
            })
        });
    diagnostic_stage(DiagnosticStageKey::Participation, status, evidence, None)
}

fn build_capture_stage(
    hook: Option<&SourceHookStatus>,
    signal: Option<&SourceSignalRecord>,
    trusted_success: bool,
    blocker: Option<CurrentTrustBlocker>,
    participation_disabled: bool,
) -> DiagnosticStage {
    let config_error = hook.and_then(|status| status.config_error.as_ref());
    let status = if participation_disabled {
        DiagnosticStatus::NotApplicable
    } else if config_error.is_some()
        || matches!(
            blocker,
            Some(CurrentTrustBlocker::SignalHardFailure | CurrentTrustBlocker::StorageUnavailable)
        )
    {
        DiagnosticStatus::Error
    } else if hook.is_some_and(|status| status.hook_registered && !status.hook_executable_exists) {
        DiagnosticStatus::Warning
    } else if hook.is_some_and(|status| status.hook_executable_exists)
        || signal.is_some()
        || trusted_success
    {
        DiagnosticStatus::Ok
    } else {
        DiagnosticStatus::Warning
    };
    let evidence = config_error
        .map(|error| sanitize_config_error_evidence(error.as_str()))
        .or_else(|| {
            hook.map(|status| {
                if signal.is_some() {
                    "最近来源信号已进入采集链路".to_string()
                } else if status.hook_executable_exists {
                    "hook executable exists".to_string()
                } else {
                    "未确认 hook executable".to_string()
                }
            })
        });
    diagnostic_stage(DiagnosticStageKey::Capture, status, evidence, None)
}

fn build_signal_stage(
    signal: Option<&SourceSignalRecord>,
    latest_signal: Option<&SourceSignalRecord>,
    trusted_success: bool,
    participation_disabled: bool,
) -> DiagnosticStage {
    let status = if participation_disabled {
        DiagnosticStatus::Disabled
    } else if signal.is_some() || trusted_success {
        DiagnosticStatus::Ok
    } else {
        DiagnosticStatus::Unknown
    };
    let evidence = signal
        .or(latest_signal)
        .map(|record| format!("最近信号：{}", record.seen_at.to_rfc3339()));
    let checked_at = signal.or(latest_signal).map(|record| record.seen_at);
    diagnostic_stage(DiagnosticStageKey::Signal, status, evidence, checked_at)
}

fn build_extraction_stage(
    signal: Option<&SourceSignalRecord>,
    latest_success: Option<&SourceSignalRecord>,
    trust: SuccessTrust,
    blocker: Option<CurrentTrustBlocker>,
    participation_disabled: bool,
) -> DiagnosticStage {
    let inserted = signal.and_then(|record| record.inserted);
    let status = if participation_disabled {
        DiagnosticStatus::Disabled
    } else if matches!(
        blocker,
        Some(CurrentTrustBlocker::SignalHardFailure | CurrentTrustBlocker::StorageUnavailable)
    ) {
        DiagnosticStatus::Error
    } else if trust == SuccessTrust::Trusted {
        DiagnosticStatus::Ok
    } else if trust == SuccessTrust::Expired {
        DiagnosticStatus::Warning
    } else if signal.is_some_and(|record| record.error_kind.is_some()) {
        DiagnosticStatus::Error
    } else if signal.is_some_and(|record| {
        record.empty_reason.is_some() && record.inserted.is_none_or(|inserted| inserted == 0)
    }) {
        DiagnosticStatus::Warning
    } else if inserted.is_some_and(|inserted| inserted > 0) {
        DiagnosticStatus::Ok
    } else {
        DiagnosticStatus::Unknown
    };
    let evidence = signal
        .map(|record| {
            if let Some(reason) = record.empty_reason {
                format!("empty_reason={}", empty_reason_label(reason))
            } else if let Some(error_kind) = &record.error_kind {
                format!("error_kind={error_kind}")
            } else {
                format!("inserted={}", record.inserted.unwrap_or(0))
            }
        })
        .or_else(|| {
            latest_success
                .map(|record| format!("latest_success_at={}", record.seen_at.to_rfc3339()))
        });
    let checked_at = signal
        .map(|record| record.seen_at)
        .or_else(|| latest_success.map(|record| record.seen_at));
    diagnostic_stage(DiagnosticStageKey::Extraction, status, evidence, checked_at)
}

fn build_storage_stage(
    signal: Option<&SourceSignalRecord>,
    latest_success: Option<&SourceSignalRecord>,
    latest_storage: Option<&DateTime<Utc>>,
    trust: SuccessTrust,
    sqlite_ok: bool,
    blocker: Option<CurrentTrustBlocker>,
    participation_disabled: bool,
) -> DiagnosticStage {
    let status = if participation_disabled {
        DiagnosticStatus::Disabled
    } else if !sqlite_ok
        || matches!(
            blocker,
            Some(CurrentTrustBlocker::SignalHardFailure | CurrentTrustBlocker::StorageUnavailable)
        )
    {
        DiagnosticStatus::Error
    } else if trust == SuccessTrust::Trusted {
        DiagnosticStatus::Ok
    } else if trust == SuccessTrust::Expired {
        DiagnosticStatus::Warning
    } else if signal.is_some_and(|record| record.error_kind.is_some()) {
        DiagnosticStatus::Error
    } else if signal
        .and_then(|record| record.inserted)
        .is_some_and(|inserted| inserted > 0)
    {
        DiagnosticStatus::Ok
    } else if signal.is_some_and(|record| {
        record.inserted == Some(0)
            && (record.empty_reason.is_none()
                || record.duplicates.is_some_and(|duplicates| duplicates > 0)
                || record
                    .skipped_outside_tracking
                    .is_some_and(|skipped| skipped > 0))
    }) {
        DiagnosticStatus::Warning
    } else {
        DiagnosticStatus::Unknown
    };
    let signal_text = signal.map(|record| {
        if let Some(error_kind) = &record.error_kind {
            format!("error_kind={error_kind}")
        } else {
            format!(
                "inserted={}, duplicates={}, skipped_outside_tracking={}",
                record.inserted.unwrap_or(0),
                record.duplicates.unwrap_or(0),
                record.skipped_outside_tracking.unwrap_or(0)
            )
        }
    });
    let mut evidence_parts = Vec::new();
    if !sqlite_ok && !participation_disabled {
        evidence_parts.push("sqlite_unavailable".to_string());
    }
    if let Some(signal_text) = signal_text {
        evidence_parts.push(signal_text);
    }
    if let Some(success) = latest_success {
        evidence_parts.push(format!(
            "latest success at {}",
            success.seen_at.to_rfc3339()
        ));
    }
    if let Some(stored_at) = latest_storage {
        evidence_parts.push(format!("last stored at {}", stored_at.to_rfc3339()));
    }
    let evidence = (!evidence_parts.is_empty()).then(|| evidence_parts.join("; "));
    let checked_at = signal
        .map(|record| record.seen_at)
        .or_else(|| latest_success.map(|record| record.seen_at))
        .or_else(|| latest_storage.copied());
    diagnostic_stage(DiagnosticStageKey::Storage, status, evidence, checked_at)
}

fn diagnostic_stage(
    key: DiagnosticStageKey,
    status: DiagnosticStatus,
    evidence: Option<String>,
    checked_at: Option<DateTime<Utc>>,
) -> DiagnosticStage {
    DiagnosticStage {
        key,
        label: key.label().to_string(),
        status,
        summary: status.summary_label().to_string(),
        evidence,
        checked_at,
    }
}

fn format_diagnostic_time(at: DateTime<Utc>, generated_at: DateTime<Utc>) -> String {
    let local_at = at.with_timezone(&Local);
    let local_generated_at = generated_at.with_timezone(&Local);
    if local_at.date_naive() == local_generated_at.date_naive() {
        local_at.format("%H:%M").to_string()
    } else {
        format!(
            "{} 月 {} 日 {}",
            local_at.month(),
            local_at.day(),
            local_at.format("%H:%M")
        )
    }
}

fn headline_user_label(headline: DiagnosticHeadline) -> &'static str {
    match headline {
        DiagnosticHeadline::Connected => "已接入",
        DiagnosticHeadline::Disabled => "未启用",
        DiagnosticHeadline::PendingVerification => "待验证",
        DiagnosticHeadline::CaptureNotReady => "捕获未就绪",
        DiagnosticHeadline::SignalNotSeen => "未看到信号",
        DiagnosticHeadline::TokenNotExtracted => "未提取到 token",
        DiagnosticHeadline::NotStored => "未写入统计",
        DiagnosticHeadline::ConfigurationError => "配置异常",
        DiagnosticHeadline::RuntimeError => "运行异常",
    }
}

fn trust_user_label(trust: SuccessTrust) -> &'static str {
    match trust {
        SuccessTrust::Trusted => "可信",
        SuccessTrust::Expired => "已过期",
        SuccessTrust::Missing => "缺少",
        SuccessTrust::OverriddenByCurrentBlocker => "被当前问题覆盖",
    }
}

fn empty_reason_user_label(reason: SourceEmptyReason) -> &'static str {
    match reason {
        SourceEmptyReason::NoNewCompleteRound => "无新增完整轮次",
        SourceEmptyReason::WatermarkAtEof => "已处理到文件末尾",
        SourceEmptyReason::DuplicateOnly => "只有重复记录",
        SourceEmptyReason::OutsideTrackingWindow => "不在当前统计周期内",
        SourceEmptyReason::UnsupportedHookEvent => "不支持的 hook 事件",
        SourceEmptyReason::InputMissing => "hook 输入缺失",
        SourceEmptyReason::TranscriptPathMissing => "transcript 路径缺失",
        SourceEmptyReason::TranscriptUnreadable => "transcript 不可读",
        SourceEmptyReason::NoCompleteJsonlRows => "没有完整 JSONL 记录",
    }
}

fn error_kind_user_label(error_kind: &str) -> &'static str {
    match error_kind {
        "transcript_unreadable" => "transcript 不可读",
        "transcript_parse_failed" => "transcript 解析失败",
        "sqlite_write_failed" => "SQLite 写入失败",
        "watermark_write_failed" => "读取进度保存失败",
        "source_resolver_failed" => "来源定位失败",
        "source_adapter_failed" => "来源适配失败",
        _ => "来源处理失败",
    }
}

fn blocker_user_label(blocker: CurrentTrustBlocker) -> &'static str {
    match blocker {
        CurrentTrustBlocker::ConfigError => "采集配置错误",
        CurrentTrustBlocker::CaptureNotReady => "采集程序不可用",
        CurrentTrustBlocker::StorageUnavailable => "SQLite 不可用",
        CurrentTrustBlocker::SignalHardFailure => "来源处理失败",
    }
}

fn current_judgment_user_label(headline: DiagnosticHeadline, trust: SuccessTrust) -> &'static str {
    if headline == DiagnosticHeadline::Connected && trust == SuccessTrust::Trusted {
        "可信"
    } else {
        headline_user_label(headline)
    }
}

fn signal_note_text(signal: Option<&SourceSignalRecord>) -> Option<String> {
    signal.and_then(|record| {
        if let Some(skipped) = record.skipped_outside_tracking.filter(|count| *count > 0) {
            Some(format!(
                "扫描到 {skipped} 条窗口外历史记录，未计入当前统计周期"
            ))
        } else {
            record
                .empty_reason
                .map(|reason| format!("最新检查{}", empty_reason_user_label(reason)))
        }
    })
}

fn build_display_summary(
    headline: DiagnosticHeadline,
    signal: Option<&SourceSignalRecord>,
    accepted_success: Option<AcceptedSuccessEvidence<'_>>,
    blocker: Option<CurrentTrustBlocker>,
    generated_at: DateTime<Utc>,
) -> DiagnosticDisplaySummary {
    let status_text = headline_user_label(headline).to_string();

    if blocker.is_some() {
        let detail_text = match headline {
            DiagnosticHeadline::ConfigurationError => "采集配置需要处理",
            DiagnosticHeadline::CaptureNotReady => "采集程序不可用",
            _ => "最新问题影响统计链路",
        }
        .to_string();
        return DiagnosticDisplaySummary {
            status_text,
            detail_text,
            note_text: Some("展开查看问题证据".to_string()),
        };
    }

    match headline {
        DiagnosticHeadline::Connected => {
            let detail_text = match accepted_success {
                Some(AcceptedSuccessEvidence::Runtime(record)) => format!(
                    "最近成功写入 {inserted} 条 · {}",
                    format_diagnostic_time(record.seen_at, generated_at),
                    inserted = record.inserted.unwrap_or(0),
                ),
                Some(AcceptedSuccessEvidence::Storage(at)) => format!(
                    "数据库最近写入 · {}",
                    format_diagnostic_time(at, generated_at)
                ),
                None => "最近成功已入库".to_string(),
            };
            DiagnosticDisplaySummary {
                status_text,
                detail_text,
                note_text: signal_note_text(signal),
            }
        }
        DiagnosticHeadline::PendingVerification => DiagnosticDisplaySummary {
            status_text,
            detail_text: "最近成功已过期".to_string(),
            note_text: Some("需要新的成功写入验证".to_string()),
        },
        DiagnosticHeadline::Disabled => DiagnosticDisplaySummary {
            status_text,
            detail_text: "该来源尚未启用".to_string(),
            note_text: None,
        },
        DiagnosticHeadline::SignalNotSeen => DiagnosticDisplaySummary {
            status_text,
            detail_text: "近期尚未捕获来源事件".to_string(),
            note_text: Some("展开查看接入状态".to_string()),
        },
        DiagnosticHeadline::TokenNotExtracted => DiagnosticDisplaySummary {
            status_text,
            detail_text: "最近采集未产生 token usage".to_string(),
            note_text: signal_note_text(signal).or_else(|| Some("展开查看采集结果".to_string())),
        },
        DiagnosticHeadline::NotStored => DiagnosticDisplaySummary {
            status_text,
            detail_text: "token usage 尚未写入统计".to_string(),
            note_text: Some("展开查看数据库证据".to_string()),
        },
        DiagnosticHeadline::CaptureNotReady => DiagnosticDisplaySummary {
            status_text,
            detail_text: "采集程序不可用".to_string(),
            note_text: Some("展开查看接入状态".to_string()),
        },
        // 正常不变量下 blocker 分支已提前返回；这里做保守兜底而非 panic，
        // 避免后续 chain/blocker 逻辑漂移时崩溃用户面诊断快照。
        DiagnosticHeadline::ConfigurationError | DiagnosticHeadline::RuntimeError => {
            DiagnosticDisplaySummary {
                status_text,
                detail_text: "最新问题影响统计链路".to_string(),
                note_text: Some("展开查看问题证据".to_string()),
            }
        }
    }
}

fn build_evidence(
    display_summary: &DiagnosticDisplaySummary,
    headline: DiagnosticHeadline,
    generated_at: DateTime<Utc>,
    hook: Option<&SourceHookStatus>,
    signal: Option<&SourceSignalRecord>,
    latest_signal: Option<&SourceSignalRecord>,
    hard_failure: Option<&SourceSignalRecord>,
    latest_storage: Option<&DateTime<Utc>>,
    trust: SuccessTrust,
    blocker: Option<CurrentTrustBlocker>,
    sqlite_ok: bool,
    participation_disabled: bool,
) -> Vec<DiagnosticEvidenceGroup> {
    let basis = display_summary
        .note_text
        .as_ref()
        .map(|note| format!("{}；{note}", display_summary.detail_text))
        .unwrap_or_else(|| display_summary.detail_text.clone());
    let current_items = vec![
        evidence_item(
            "状态",
            current_judgment_user_label(headline, trust).to_string(),
            Some(if blocker.is_some() {
                DiagnosticEvidenceStatus::Error
            } else if trust == SuccessTrust::Trusted {
                DiagnosticEvidenceStatus::Ok
            } else {
                DiagnosticEvidenceStatus::Warning
            }),
        ),
        evidence_item("依据", basis, Some(DiagnosticEvidenceStatus::Muted)),
    ];

    let mut access_items = Vec::new();
    match hook {
        Some(status) => {
            let (config_value, config_tone) = if status.config_error.is_some() {
                ("配置读取失败", DiagnosticEvidenceStatus::Error)
            } else if status.hook_registered {
                ("已安装", DiagnosticEvidenceStatus::Ok)
            } else if participation_disabled {
                ("未启用", DiagnosticEvidenceStatus::Muted)
            } else {
                ("未安装", DiagnosticEvidenceStatus::Warning)
            };
            access_items.push(evidence_item(
                "采集配置",
                config_value.to_string(),
                Some(config_tone),
            ));
            access_items.push(evidence_item(
                "采集程序",
                if status.hook_executable_exists {
                    "可用"
                } else {
                    "缺失"
                }
                .to_string(),
                Some(if status.hook_executable_exists {
                    DiagnosticEvidenceStatus::Ok
                } else {
                    DiagnosticEvidenceStatus::Error
                }),
            ));
        }
        None => access_items.push(evidence_item(
            "采集配置",
            if participation_disabled {
                "未启用"
            } else {
                "未检测到"
            }
            .to_string(),
            Some(DiagnosticEvidenceStatus::Muted),
        )),
    }

    let displayed_signal = signal.or(latest_signal);
    let mut capture_items = vec![evidence_item(
        "最近捕获",
        displayed_signal
            .map(|record| format_diagnostic_time(record.seen_at, generated_at))
            .unwrap_or_else(|| "暂无".to_string()),
        Some(DiagnosticEvidenceStatus::Muted),
    )];
    if let Some(record) = displayed_signal {
        if let Some(inserted) = record.inserted {
            capture_items.push(evidence_item(
                "本次写入",
                format!("{inserted} 条"),
                Some(if inserted > 0 {
                    DiagnosticEvidenceStatus::Ok
                } else {
                    DiagnosticEvidenceStatus::Muted
                }),
            ));
        }
        if let Some(duplicates) = record.duplicates {
            capture_items.push(evidence_item(
                "重复记录",
                format!("{duplicates} 条"),
                Some(DiagnosticEvidenceStatus::Muted),
            ));
        }
        if let Some(skipped) = record.skipped_outside_tracking {
            capture_items.push(evidence_item(
                "窗口外记录",
                format!("{skipped} 条"),
                Some(DiagnosticEvidenceStatus::Muted),
            ));
        }
        if let Some(reason) = record.empty_reason {
            capture_items.push(evidence_item(
                "检查结果",
                empty_reason_user_label(reason).to_string(),
                Some(DiagnosticEvidenceStatus::Muted),
            ));
        }
    }

    let storage_value = if !sqlite_ok {
        "无法确认".to_string()
    } else {
        latest_storage
            .map(|at| format_diagnostic_time(*at, generated_at))
            .unwrap_or_else(|| "暂无".to_string())
    };
    let storage_items = vec![
        evidence_item(
            "数据库最近写入",
            storage_value,
            Some(if sqlite_ok {
                DiagnosticEvidenceStatus::Muted
            } else {
                DiagnosticEvidenceStatus::Error
            }),
        ),
        evidence_item(
            "可信状态",
            trust_user_label(trust).to_string(),
            Some(if trust == SuccessTrust::Trusted {
                DiagnosticEvidenceStatus::Ok
            } else if trust == SuccessTrust::OverriddenByCurrentBlocker {
                DiagnosticEvidenceStatus::Error
            } else {
                DiagnosticEvidenceStatus::Warning
            }),
        ),
    ];

    let (problem_value, problem_tone) = match blocker {
        Some(CurrentTrustBlocker::SignalHardFailure) => {
            let value = hard_failure
                .and_then(|failure| {
                    failure
                        .empty_reason
                        .map(empty_reason_user_label)
                        .or_else(|| failure.error_kind.as_deref().map(error_kind_user_label))
                })
                .unwrap_or("来源处理失败");
            (value.to_string(), DiagnosticEvidenceStatus::Error)
        }
        Some(blocker) => (
            blocker_user_label(blocker).to_string(),
            DiagnosticEvidenceStatus::Error,
        ),
        None => ("无".to_string(), DiagnosticEvidenceStatus::Muted),
    };

    vec![
        DiagnosticEvidenceGroup {
            title: "当前判断".to_string(),
            items: current_items,
        },
        DiagnosticEvidenceGroup {
            title: "接入状态".to_string(),
            items: access_items,
        },
        DiagnosticEvidenceGroup {
            title: "最近采集".to_string(),
            items: capture_items,
        },
        DiagnosticEvidenceGroup {
            title: "数据库证据".to_string(),
            items: storage_items,
        },
        DiagnosticEvidenceGroup {
            title: "最新问题".to_string(),
            items: vec![evidence_item("最新问题", problem_value, Some(problem_tone))],
        },
    ]
}

fn evidence_item(
    label: &str,
    value: String,
    status: Option<DiagnosticEvidenceStatus>,
) -> DiagnosticEvidenceItem {
    DiagnosticEvidenceItem {
        label: label.to_string(),
        value,
        status,
    }
}

fn empty_reason_label(reason: SourceEmptyReason) -> &'static str {
    match reason {
        SourceEmptyReason::UnsupportedHookEvent => "unsupported_hook_event",
        SourceEmptyReason::InputMissing => "input_missing",
        SourceEmptyReason::TranscriptPathMissing => "transcript_path_missing",
        SourceEmptyReason::TranscriptUnreadable => "transcript_unreadable",
        SourceEmptyReason::NoCompleteJsonlRows => "no_complete_jsonl_rows",
        SourceEmptyReason::NoNewCompleteRound => "no_new_complete_round",
        SourceEmptyReason::WatermarkAtEof => "watermark_at_eof",
        SourceEmptyReason::DuplicateOnly => "duplicate_only",
        SourceEmptyReason::OutsideTrackingWindow => "outside_tracking_window",
    }
}

fn sanitize_config_error_evidence(_error: &str) -> String {
    "配置读取失败：config_error".to_string()
}

fn diagnostic_actions() -> Vec<DiagnosticAction> {
    vec![
        DiagnosticAction {
            id: DiagnosticActionId::Refresh,
            label: "刷新".to_string(),
            enabled: true,
            reason: None,
        },
        DiagnosticAction {
            id: DiagnosticActionId::OpenLogs,
            label: "打开日志".to_string(),
            enabled: true,
            reason: None,
        },
        DiagnosticAction {
            id: DiagnosticActionId::CopyDebugBundle,
            label: "复制诊断包".to_string(),
            enabled: true,
            reason: None,
        },
    ]
}

pub fn headline_for_chain(chain: &[DiagnosticStage]) -> DiagnosticHeadline {
    let stage_status = |key| {
        chain
            .iter()
            .find(|stage| stage.key == key)
            .map(|stage| stage.status)
            .unwrap_or(DiagnosticStatus::Unknown)
    };

    match stage_status(DiagnosticStageKey::Participation) {
        DiagnosticStatus::Disabled | DiagnosticStatus::NotApplicable => {
            return DiagnosticHeadline::Disabled;
        }
        DiagnosticStatus::Error => return DiagnosticHeadline::ConfigurationError,
        _ => {}
    }

    match stage_status(DiagnosticStageKey::Capture) {
        DiagnosticStatus::Warning | DiagnosticStatus::Error | DiagnosticStatus::Unknown => {
            return DiagnosticHeadline::CaptureNotReady;
        }
        _ => {}
    }

    match stage_status(DiagnosticStageKey::Signal) {
        DiagnosticStatus::Warning | DiagnosticStatus::Unknown | DiagnosticStatus::Error => {
            return DiagnosticHeadline::SignalNotSeen;
        }
        _ => {}
    }

    match stage_status(DiagnosticStageKey::Extraction) {
        DiagnosticStatus::Warning | DiagnosticStatus::Error | DiagnosticStatus::Unknown => {
            return DiagnosticHeadline::TokenNotExtracted;
        }
        _ => {}
    }

    match stage_status(DiagnosticStageKey::Storage) {
        DiagnosticStatus::Warning | DiagnosticStatus::Error | DiagnosticStatus::Unknown => {
            return DiagnosticHeadline::NotStored
        }
        _ => {}
    }

    DiagnosticHeadline::Connected
}

fn headline_for_trust(
    chain: &[DiagnosticStage],
    trust: SuccessTrust,
    blocker: Option<CurrentTrustBlocker>,
) -> DiagnosticHeadline {
    if let Some(blocker) = blocker {
        return blocker.headline();
    }
    let chain_headline = headline_for_chain(chain);
    if matches!(
        chain_headline,
        DiagnosticHeadline::Disabled
            | DiagnosticHeadline::ConfigurationError
            | DiagnosticHeadline::CaptureNotReady
            | DiagnosticHeadline::RuntimeError
    ) {
        return chain_headline;
    }
    if trust == SuccessTrust::Expired {
        return DiagnosticHeadline::PendingVerification;
    }
    chain_headline
}

pub fn primary_break_for_chain(chain: &[DiagnosticStage]) -> Option<DiagnosticBreak> {
    let headline = headline_for_chain(chain);
    primary_break_for_headline(headline, chain)
}

fn primary_break_for_headline(
    headline: DiagnosticHeadline,
    chain: &[DiagnosticStage],
) -> Option<DiagnosticBreak> {
    let (stage, title, impact) = match headline {
        DiagnosticHeadline::Connected => return None,
        DiagnosticHeadline::Disabled => (
            DiagnosticStageKey::Participation,
            "未启用",
            "该来源不会影响整体接入状态",
        ),
        DiagnosticHeadline::PendingVerification => {
            (DiagnosticStageKey::Storage, "待验证", "近期未验证成功写入")
        }
        DiagnosticHeadline::ConfigurationError => (
            DiagnosticStageKey::Participation,
            "配置异常",
            "无法确认该来源是否参与采集",
        ),
        DiagnosticHeadline::CaptureNotReady => (
            DiagnosticStageKey::Capture,
            "捕获未就绪",
            "TokenFire 暂时不能可靠捕获该来源信号",
        ),
        DiagnosticHeadline::SignalNotSeen => (
            DiagnosticStageKey::Signal,
            "最近未看到信号",
            "hook 或文件事件尚未被验证触发",
        ),
        DiagnosticHeadline::TokenNotExtracted => (
            DiagnosticStageKey::Extraction,
            "未提取到 token",
            "最近信号没有产生 token observation",
        ),
        DiagnosticHeadline::NotStored => (
            DiagnosticStageKey::Storage,
            "未写入统计",
            "token usage 没有进入 Profile 统计",
        ),
        DiagnosticHeadline::RuntimeError => (
            DiagnosticStageKey::Storage,
            "运行异常",
            "运行时错误影响统计链路",
        ),
    };
    let evidence = chain
        .iter()
        .find(|item| item.key == stage)
        .and_then(|item| item.evidence.clone())
        .unwrap_or_else(|| "no stable evidence".to_string());

    Some(DiagnosticBreak {
        stage,
        title: title.to_string(),
        evidence,
        impact: impact.to_string(),
    })
}

fn build_trust_summary(
    signal: Option<&SourceSignalRecord>,
    latest_success: Option<&SourceSignalRecord>,
    blocker: Option<CurrentTrustBlocker>,
    trust: SuccessTrust,
) -> String {
    if let Some(blocker) = blocker {
        return format!("最新问题：{}", blocker.evidence());
    }
    match trust {
        SuccessTrust::Trusted => {
            if let Some(signal) =
                signal.and_then(|record| record.empty_reason.map(|reason| (record, reason)))
            {
                format!(
                    "最近成功 {} · 最新检查{}",
                    latest_success
                        .map(|record| record.seen_at.format("%H:%M").to_string())
                        .unwrap_or_else(|| "已入库".to_string()),
                    empty_reason_summary_label(signal.1)
                )
            } else {
                format!(
                    "最近成功 {} · 数据链路有完整证据",
                    latest_success
                        .map(|record| record.seen_at.format("%H:%M").to_string())
                        .unwrap_or_else(|| "已入库".to_string())
                )
            }
        }
        SuccessTrust::Expired => "最近成功已过期 · 需要新的成功写入验证".to_string(),
        SuccessTrust::Missing => "近期未验证成功写入".to_string(),
        SuccessTrust::OverriddenByCurrentBlocker => "最近成功被当前问题覆盖".to_string(),
    }
}

fn empty_reason_summary_label(reason: SourceEmptyReason) -> &'static str {
    match reason {
        SourceEmptyReason::NoNewCompleteRound
        | SourceEmptyReason::WatermarkAtEof
        | SourceEmptyReason::DuplicateOnly => "无新增",
        SourceEmptyReason::OutsideTrackingWindow => "在统计窗口外",
        SourceEmptyReason::UnsupportedHookEvent => "事件不支持",
        SourceEmptyReason::InputMissing => "输入缺失",
        SourceEmptyReason::TranscriptPathMissing => "transcript 路径缺失",
        SourceEmptyReason::TranscriptUnreadable => "transcript 不可读",
        SourceEmptyReason::NoCompleteJsonlRows => "无完整 JSONL 行",
    }
}

#[cfg(test)]
mod tests {
    use super::{build_display_summary, format_diagnostic_time, DiagnosticHeadline};
    use chrono::{Duration, Local, TimeZone, Utc};

    #[test]
    fn diagnostic_time_includes_date_across_local_days() {
        let at = Utc.with_ymd_and_hms(2026, 7, 10, 10, 0, 0).unwrap();
        let generated_at = at + Duration::hours(48);
        let local_at = at.with_timezone(&Local);
        let expected = local_at.format("%-m 月 %-d 日 %H:%M").to_string();

        assert_eq!(format_diagnostic_time(at, generated_at), expected);
    }

    // 用户面 Tauri 路径不能因 headline 与 blocker 不变量漂移而 panic：
    // 即使出现当前不应到达的 ConfigurationError / RuntimeError（无 blocker），
    // 也必须返回保守摘要而不是崩溃整份诊断快照。
    #[test]
    fn display_summary_falls_back_for_unexpected_error_headlines() {
        let generated_at = Utc.with_ymd_and_hms(2026, 7, 10, 10, 0, 0).unwrap();

        for headline in [
            DiagnosticHeadline::ConfigurationError,
            DiagnosticHeadline::RuntimeError,
        ] {
            let summary = build_display_summary(headline, None, None, None, generated_at);
            assert_eq!(summary.detail_text, "最新问题影响统计链路");
            assert_eq!(summary.note_text.as_deref(), Some("展开查看问题证据"));
        }
    }
}
