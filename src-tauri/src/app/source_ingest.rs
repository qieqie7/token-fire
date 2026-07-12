use std::fmt;
use std::path::PathBuf;

use serde_json::json;

use crate::adapters::cursor::{
    collect_pending_for_conversation, collect_pending_from_transcript_path, CursorCollectResult,
    CursorEmptyReason as CursorAdapterEmptyReason, CursorTranscriptIdentity,
};
use crate::adapters::source::{SourceContext, SourceRegistry, TokenSourceKind};
use crate::adapters::HookMetadata;
use crate::app::ingest_scheduler::{IngestReport, IngestScheduler};
use crate::app::logging::append_app_log;
use crate::app::logging::RuntimeLogger;

#[derive(Debug, Clone, Default)]
pub struct SourceIngestPaths {
    pub cursor_home: Option<PathBuf>,
    pub cursor_watermark_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceIngestOutcome {
    pub report: Option<IngestReport>,
    pub source: TokenSourceKind,
    pub event: SourceIngestEvent,
    pub resolution: SourceResolution,
    pub empty_reason: Option<SourceEmptyReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceIngestEvent {
    Hook,
    TranscriptChanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceResolution {
    TranscriptPath,
    ConversationId,
    SourceRegistry,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceEmptyReason {
    UnsupportedHookEvent,
    InputMissing,
    TranscriptPathMissing,
    TranscriptUnreadable,
    NoCompleteJsonlRows,
    NoNewCompleteRound,
    WatermarkAtEof,
    DuplicateOnly,
    OutsideTrackingWindow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceErrorKind {
    TranscriptUnreadable,
    TranscriptParseFailed,
    SqliteWriteFailed,
    WatermarkWriteFailed,
    SourceResolverFailed,
    SourceAdapterFailed,
}

impl SourceResolution {
    fn as_str(self) -> &'static str {
        match self {
            SourceResolution::TranscriptPath => "transcript_path",
            SourceResolution::ConversationId => "conversation_id",
            SourceResolution::SourceRegistry => "source_registry",
            SourceResolution::None => "none",
        }
    }
}

impl SourceEmptyReason {
    fn as_str(self) -> &'static str {
        match self {
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
}

impl SourceErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SourceErrorKind::TranscriptUnreadable => "transcript_unreadable",
            SourceErrorKind::TranscriptParseFailed => "transcript_parse_failed",
            SourceErrorKind::SqliteWriteFailed => "sqlite_write_failed",
            SourceErrorKind::WatermarkWriteFailed => "watermark_write_failed",
            SourceErrorKind::SourceResolverFailed => "source_resolver_failed",
            SourceErrorKind::SourceAdapterFailed => "source_adapter_failed",
        }
    }

    pub fn from_runtime_kind(kind: &str) -> Self {
        match kind {
            "transcript_read_failed" | "transcript_unreadable" => {
                SourceErrorKind::TranscriptUnreadable
            }
            "transcript_parse_failed" => SourceErrorKind::TranscriptParseFailed,
            "sqlite_write_failed" => SourceErrorKind::SqliteWriteFailed,
            "watermark_write_failed" => SourceErrorKind::WatermarkWriteFailed,
            "source_resolver_failed" => SourceErrorKind::SourceResolverFailed,
            "source_adapter_failed" | "transcript_ingest_failed" | "hook_ingest_failed" => {
                SourceErrorKind::SourceAdapterFailed
            }
            _ => SourceErrorKind::SourceAdapterFailed,
        }
    }
}

#[derive(Debug)]
pub struct SourceIngestError {
    kind: SourceErrorKind,
    cause: anyhow::Error,
}

impl SourceIngestError {
    fn new(kind: SourceErrorKind, cause: anyhow::Error) -> Self {
        Self { kind, cause }
    }

    pub fn kind(&self) -> SourceErrorKind {
        self.kind
    }
}

impl fmt::Display for SourceIngestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.kind.as_str(), self.cause)
    }
}

impl std::error::Error for SourceIngestError {}

pub fn source_error_kind(error: &anyhow::Error) -> Option<SourceErrorKind> {
    error
        .downcast_ref::<SourceIngestError>()
        .map(SourceIngestError::kind)
}

pub struct SourceIngestRouter<'a> {
    pub registry: &'a SourceRegistry,
    pub scheduler: &'a IngestScheduler,
    pub logger: Option<&'a RuntimeLogger>,
    pub paths: SourceIngestPaths,
}

fn is_token_collection_event(source: TokenSourceKind, event: Option<&str>) -> bool {
    match source {
        TokenSourceKind::Traex | TokenSourceKind::Codex => event == Some("Stop"),
        TokenSourceKind::Claude => matches!(event, Some("Stop" | "StopFailure" | "SubagentStop")),
        TokenSourceKind::Cursor => event == Some("stop"),
    }
}

impl<'a> SourceIngestRouter<'a> {
    pub fn ingest_hook(
        &self,
        source: TokenSourceKind,
        metadata: HookMetadata,
    ) -> anyhow::Result<SourceIngestOutcome> {
        if !is_token_collection_event(source, metadata.hook_event_name.as_deref()) {
            return Ok(SourceIngestOutcome {
                report: None,
                source,
                event: SourceIngestEvent::Hook,
                resolution: SourceResolution::None,
                empty_reason: Some(SourceEmptyReason::UnsupportedHookEvent),
            });
        }

        match source {
            TokenSourceKind::Traex | TokenSourceKind::Codex => {
                self.ingest_traex_like_hook(source, metadata)
            }
            TokenSourceKind::Claude => self.ingest_claude_hook(source, metadata),
            TokenSourceKind::Cursor => self.ingest_cursor_hook(metadata),
        }
    }

    pub fn ingest_file_changed(
        &self,
        source: TokenSourceKind,
        path: PathBuf,
    ) -> anyhow::Result<SourceIngestOutcome> {
        let metadata = transcript_changed_metadata();
        if !matches!(source, TokenSourceKind::Traex | TokenSourceKind::Codex) {
            let outcome = SourceIngestOutcome {
                report: None,
                source,
                event: SourceIngestEvent::TranscriptChanged,
                resolution: SourceResolution::None,
                empty_reason: Some(SourceEmptyReason::UnsupportedHookEvent),
            };
            self.log_empty(&metadata, &outcome);
            return Ok(outcome);
        }
        let report = match self
            .scheduler
            .ingest_path_for_source(SourceContext::new(source), &path)
        {
            Ok(report) => report,
            Err(error) => {
                let error_kind = scheduler_error_kind(&error);
                let outcome = SourceIngestOutcome {
                    report: None,
                    source,
                    event: SourceIngestEvent::TranscriptChanged,
                    resolution: SourceResolution::SourceRegistry,
                    empty_reason: None,
                };
                self.log_failed(&metadata, &outcome, error_kind);
                return Err(source_error(error_kind, error));
            }
        };
        let outcome = self.report_outcome(
            source,
            SourceIngestEvent::TranscriptChanged,
            SourceResolution::SourceRegistry,
            report,
        );
        self.log_outcome(&metadata, &outcome);
        Ok(outcome)
    }

    fn ingest_cursor_hook(&self, metadata: HookMetadata) -> anyhow::Result<SourceIngestOutcome> {
        let watermark_dir = self.paths.cursor_watermark_dir.as_deref();
        if let Some(transcript_path) = metadata.transcript_path.as_ref().map(PathBuf::from) {
            let identity = CursorTranscriptIdentity::from_path(&transcript_path, &metadata);
            let result = collect_pending_from_transcript_path(
                &transcript_path,
                &metadata,
                identity,
                watermark_dir,
            );
            let should_fallback = matches!(
                result.as_ref(),
                Ok(CursorCollectResult::Empty(
                    CursorAdapterEmptyReason::TranscriptPathMissing
                        | CursorAdapterEmptyReason::TranscriptUnreadable
                ))
            ) && metadata.conversation_id.is_some();
            if !should_fallback {
                return self.finish_cursor_collect(
                    metadata,
                    SourceResolution::TranscriptPath,
                    result,
                );
            }
        }

        let Some(conversation_id) = metadata.conversation_id.as_deref() else {
            let outcome = SourceIngestOutcome {
                report: None,
                source: TokenSourceKind::Cursor,
                event: SourceIngestEvent::Hook,
                resolution: SourceResolution::None,
                empty_reason: Some(SourceEmptyReason::InputMissing),
            };
            self.log_empty(&metadata, &outcome);
            return Ok(outcome);
        };

        let result = match collect_pending_for_conversation(
            conversation_id,
            &metadata,
            self.paths.cursor_home.as_deref(),
            watermark_dir,
        ) {
            Ok(result) => result,
            Err(error) => {
                let error_kind = cursor_collect_error_kind(&error);
                let outcome = SourceIngestOutcome {
                    report: None,
                    source: TokenSourceKind::Cursor,
                    event: SourceIngestEvent::Hook,
                    resolution: SourceResolution::ConversationId,
                    empty_reason: None,
                };
                self.log_failed(&metadata, &outcome, error_kind);
                return Err(source_error(error_kind, error));
            }
        };
        self.finish_cursor_collect(metadata, SourceResolution::ConversationId, Ok(result))
    }

    fn finish_cursor_collect(
        &self,
        metadata: HookMetadata,
        resolution: SourceResolution,
        result: anyhow::Result<CursorCollectResult>,
    ) -> anyhow::Result<SourceIngestOutcome> {
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                let error_kind = cursor_collect_error_kind(&error);
                let outcome = SourceIngestOutcome {
                    report: None,
                    source: TokenSourceKind::Cursor,
                    event: SourceIngestEvent::Hook,
                    resolution,
                    empty_reason: None,
                };
                self.log_failed(&metadata, &outcome, error_kind);
                return Err(source_error(error_kind, error));
            }
        };
        match result {
            CursorCollectResult::Pending(pending) => {
                let report = match self.scheduler.ingest_observations_for_source(
                    TokenSourceKind::Cursor,
                    vec![pending.observation().clone()],
                ) {
                    Ok(report) => report,
                    Err(error) => {
                        let error_kind = scheduler_error_kind(&error);
                        let outcome = SourceIngestOutcome {
                            report: None,
                            source: TokenSourceKind::Cursor,
                            event: SourceIngestEvent::Hook,
                            resolution,
                            empty_reason: None,
                        };
                        self.log_failed(&metadata, &outcome, error_kind);
                        return Err(source_error(error_kind, error));
                    }
                };
                let outcome = self.report_outcome(
                    TokenSourceKind::Cursor,
                    SourceIngestEvent::Hook,
                    resolution,
                    report,
                );
                if should_commit_cursor_watermark(&outcome) {
                    if let Err(error) = pending.commit() {
                        let error_kind = SourceErrorKind::WatermarkWriteFailed;
                        self.log_failed(&metadata, &outcome, error_kind);
                        return Err(source_error(error_kind, error));
                    }
                }
                self.log_outcome(&metadata, &outcome);
                Ok(outcome)
            }
            CursorCollectResult::Empty(reason) => {
                let outcome = SourceIngestOutcome {
                    report: None,
                    source: TokenSourceKind::Cursor,
                    event: SourceIngestEvent::Hook,
                    resolution,
                    empty_reason: Some(map_cursor_empty_reason(reason)),
                };
                if matches!(reason, CursorAdapterEmptyReason::TranscriptUnreadable) {
                    self.log_failed(&metadata, &outcome, SourceErrorKind::TranscriptUnreadable);
                } else {
                    self.log_empty(&metadata, &outcome);
                }
                Ok(outcome)
            }
        }
    }

    fn ingest_claude_hook(
        &self,
        source: TokenSourceKind,
        metadata: HookMetadata,
    ) -> anyhow::Result<SourceIngestOutcome> {
        let Some(path) = metadata.transcript_path.as_ref().map(PathBuf::from) else {
            let outcome = SourceIngestOutcome {
                report: None,
                source,
                event: SourceIngestEvent::Hook,
                resolution: SourceResolution::None,
                empty_reason: Some(SourceEmptyReason::InputMissing),
            };
            self.log_empty(&metadata, &outcome);
            return Ok(outcome);
        };
        let observation = match crate::adapters::claude::collect_from_transcript(&path, &metadata) {
            Ok(Some(observation)) => observation,
            Ok(None) => {
                let outcome = SourceIngestOutcome {
                    report: None,
                    source,
                    event: SourceIngestEvent::Hook,
                    resolution: SourceResolution::TranscriptPath,
                    empty_reason: Some(SourceEmptyReason::NoNewCompleteRound),
                };
                self.log_empty(&metadata, &outcome);
                return Ok(outcome);
            }
            Err(_) => {
                let outcome = SourceIngestOutcome {
                    report: None,
                    source,
                    event: SourceIngestEvent::Hook,
                    resolution: SourceResolution::TranscriptPath,
                    empty_reason: Some(SourceEmptyReason::TranscriptUnreadable),
                };
                self.log_failed(&metadata, &outcome, SourceErrorKind::TranscriptUnreadable);
                return Ok(outcome);
            }
        };
        let report = match self
            .scheduler
            .ingest_observations_for_source(source, vec![observation])
        {
            Ok(report) => report,
            Err(error) => {
                let error_kind = scheduler_error_kind(&error);
                let outcome = SourceIngestOutcome {
                    report: None,
                    source,
                    event: SourceIngestEvent::Hook,
                    resolution: SourceResolution::TranscriptPath,
                    empty_reason: None,
                };
                self.log_failed(&metadata, &outcome, error_kind);
                return Err(source_error(error_kind, error));
            }
        };
        let outcome = self.report_outcome(
            source,
            SourceIngestEvent::Hook,
            SourceResolution::TranscriptPath,
            report,
        );
        self.log_outcome(&metadata, &outcome);
        Ok(outcome)
    }

    fn ingest_traex_like_hook(
        &self,
        source: TokenSourceKind,
        metadata: HookMetadata,
    ) -> anyhow::Result<SourceIngestOutcome> {
        let Some(source_paths) = self.registry.source_paths(source) else {
            let outcome = SourceIngestOutcome {
                report: None,
                source,
                event: SourceIngestEvent::Hook,
                resolution: SourceResolution::None,
                empty_reason: Some(SourceEmptyReason::InputMissing),
            };
            self.log_empty(&metadata, &outcome);
            return Ok(outcome);
        };
        let resolved_path = match crate::adapters::traex::resolver::resolve_transcript_for_source(
            source_paths,
            &metadata,
        ) {
            Ok(path) => path,
            Err(error) => {
                let error_kind = SourceErrorKind::SourceResolverFailed;
                let outcome = SourceIngestOutcome {
                    report: None,
                    source,
                    event: SourceIngestEvent::Hook,
                    resolution: SourceResolution::SourceRegistry,
                    empty_reason: None,
                };
                self.log_failed(&metadata, &outcome, error_kind);
                return Err(source_error(error_kind, error));
            }
        };
        if let Some(path) = resolved_path {
            let report = match self
                .scheduler
                .ingest_path_for_source(SourceContext::new(source), &path)
            {
                Ok(report) => report,
                Err(error) => {
                    let error_kind = scheduler_error_kind(&error);
                    let outcome = SourceIngestOutcome {
                        report: None,
                        source,
                        event: SourceIngestEvent::Hook,
                        resolution: SourceResolution::SourceRegistry,
                        empty_reason: None,
                    };
                    self.log_failed(&metadata, &outcome, error_kind);
                    return Err(source_error(error_kind, error));
                }
            };
            let outcome = self.report_outcome(
                source,
                SourceIngestEvent::Hook,
                SourceResolution::SourceRegistry,
                report,
            );
            self.log_outcome(&metadata, &outcome);
            return Ok(outcome);
        }
        if metadata
            .hook_event_name
            .as_deref()
            .is_some_and(|event| event.eq_ignore_ascii_case("stop"))
        {
            self.log_unresolved_transcript(&metadata, source);
        }
        let outcome = SourceIngestOutcome {
            report: None,
            source,
            event: SourceIngestEvent::Hook,
            resolution: SourceResolution::SourceRegistry,
            empty_reason: Some(SourceEmptyReason::TranscriptPathMissing),
        };
        self.log_empty(&metadata, &outcome);
        Ok(outcome)
    }

    fn report_outcome(
        &self,
        source: TokenSourceKind,
        event: SourceIngestEvent,
        resolution: SourceResolution,
        report: IngestReport,
    ) -> SourceIngestOutcome {
        let empty_reason = if report.inserted == 0 && report.skipped_outside_tracking > 0 {
            Some(SourceEmptyReason::OutsideTrackingWindow)
        } else if report.inserted == 0 && report.duplicates > 0 {
            Some(SourceEmptyReason::DuplicateOnly)
        } else {
            None
        };
        SourceIngestOutcome {
            report: Some(report),
            source,
            event,
            resolution,
            empty_reason,
        }
    }

    fn log_outcome(&self, metadata: &HookMetadata, outcome: &SourceIngestOutcome) {
        if outcome.empty_reason.is_some() {
            self.log_empty(metadata, outcome);
            return;
        }
        let Some(logger) = self.logger else {
            return;
        };
        let inserted = outcome
            .report
            .as_ref()
            .map(|report| report.inserted)
            .unwrap_or(0);
        let duplicates = outcome
            .report
            .as_ref()
            .map(|report| report.duplicates)
            .unwrap_or(0);
        let _ = append_app_log(
            logger,
            "info",
            "source_ingested",
            json!({
                "source": outcome.source.as_str(),
                "hook_event_name": metadata.hook_event_name.as_deref(),
                "session_id_present": metadata.session_id.is_some(),
                "conversation_id_present": metadata.conversation_id.is_some(),
                "transcript_path_present": metadata.transcript_path.is_some(),
                "resolved_by": outcome.resolution.as_str(),
                "inserted": inserted,
                "duplicates": duplicates,
                "skipped_outside_tracking": outcome
                    .report
                    .as_ref()
                    .map(|report| report.skipped_outside_tracking)
                    .unwrap_or(0)
            }),
        );
    }

    fn log_empty(&self, metadata: &HookMetadata, outcome: &SourceIngestOutcome) {
        let Some(logger) = self.logger else {
            return;
        };
        let reason = outcome
            .empty_reason
            .unwrap_or(SourceEmptyReason::NoNewCompleteRound);
        let _ = append_app_log(
            logger,
            "warn",
            "source_collect_empty",
            json!({
                "source": outcome.source.as_str(),
                "hook_event_name": metadata.hook_event_name.as_deref(),
                "session_id_present": metadata.session_id.is_some(),
                "conversation_id_present": metadata.conversation_id.is_some(),
                "transcript_path_present": metadata.transcript_path.is_some(),
                "resolved_by": outcome.resolution.as_str(),
                "empty_reason": reason.as_str(),
                "inserted": outcome.report.as_ref().map(|report| report.inserted),
                "duplicates": outcome.report.as_ref().map(|report| report.duplicates),
                "skipped_outside_tracking": outcome
                    .report
                    .as_ref()
                    .map(|report| report.skipped_outside_tracking)
            }),
        );
    }

    fn log_failed(
        &self,
        metadata: &HookMetadata,
        outcome: &SourceIngestOutcome,
        error_kind: SourceErrorKind,
    ) {
        let Some(logger) = self.logger else {
            return;
        };
        let _ = append_app_log(
            logger,
            "warn",
            "source_collect_failed",
            json!({
                "source": outcome.source.as_str(),
                "hook_event_name": metadata.hook_event_name.as_deref(),
                "session_id_present": metadata.session_id.is_some(),
                "conversation_id_present": metadata.conversation_id.is_some(),
                "transcript_path_present": metadata.transcript_path.is_some(),
                "resolved_by": outcome.resolution.as_str(),
                "error_kind": error_kind.as_str()
            }),
        );
    }

    fn log_unresolved_transcript(&self, metadata: &HookMetadata, source: TokenSourceKind) {
        let Some(logger) = self.logger else {
            return;
        };
        let _ = append_app_log(
            logger,
            "warn",
            "hook_transcript_unresolved",
            json!({
                "source": source.as_str(),
                "session_id": metadata.session_id.as_deref(),
                "hook_event_name": metadata.hook_event_name.as_deref(),
                "transcript_path_present": metadata.transcript_path.is_some(),
            }),
        );
    }
}

fn map_cursor_empty_reason(reason: CursorAdapterEmptyReason) -> SourceEmptyReason {
    match reason {
        CursorAdapterEmptyReason::TranscriptPathMissing => SourceEmptyReason::TranscriptPathMissing,
        CursorAdapterEmptyReason::TranscriptUnreadable => SourceEmptyReason::TranscriptUnreadable,
        CursorAdapterEmptyReason::NoCompleteJsonlRows => SourceEmptyReason::NoCompleteJsonlRows,
        CursorAdapterEmptyReason::NoNewCompleteRound => SourceEmptyReason::NoNewCompleteRound,
        CursorAdapterEmptyReason::WatermarkAtEof => SourceEmptyReason::WatermarkAtEof,
    }
}

fn transcript_changed_metadata() -> HookMetadata {
    HookMetadata {
        transcript_path: Some(String::new()),
        ..HookMetadata::default()
    }
}

fn scheduler_error_kind(error: &anyhow::Error) -> SourceErrorKind {
    let message = error.to_string();
    if error.downcast_ref::<std::io::Error>().is_some() {
        SourceErrorKind::TranscriptUnreadable
    } else if error.downcast_ref::<rusqlite::Error>().is_some() {
        SourceErrorKind::SqliteWriteFailed
    } else if message.contains("sqlite") || message.contains("database") {
        SourceErrorKind::SqliteWriteFailed
    } else {
        SourceErrorKind::SourceAdapterFailed
    }
}

fn cursor_collect_error_kind(error: &anyhow::Error) -> SourceErrorKind {
    if error.downcast_ref::<serde_json::Error>().is_some() {
        SourceErrorKind::TranscriptParseFailed
    } else {
        SourceErrorKind::SourceAdapterFailed
    }
}

fn source_error(kind: SourceErrorKind, error: anyhow::Error) -> anyhow::Error {
    SourceIngestError::new(kind, error).into()
}

fn should_commit_cursor_watermark(outcome: &SourceIngestOutcome) -> bool {
    match outcome.report.as_ref() {
        Some(report) if report.inserted > 0 => true,
        Some(report)
            if report.inserted == 0
                && report.duplicates > 0
                && report.skipped_outside_tracking == 0 =>
        {
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::core::usage_store::UsageStore;

    #[test]
    fn report_outcome_classifies_zero_insert_mixed_duplicates_and_outside_tracking() {
        let dir = tempdir().unwrap();
        let store = UsageStore::open(&dir.path().join("token-fire.sqlite")).unwrap();
        let scheduler = IngestScheduler::new(store);
        let registry = SourceRegistry::new(vec![]);
        let router = SourceIngestRouter {
            registry: &registry,
            scheduler: &scheduler,
            logger: None,
            paths: SourceIngestPaths::default(),
        };

        let outcome = router.report_outcome(
            TokenSourceKind::Cursor,
            SourceIngestEvent::Hook,
            SourceResolution::TranscriptPath,
            IngestReport {
                inserted: 0,
                duplicates: 1,
                skipped_outside_tracking: 1,
                last_processed_offset: -1,
            },
        );

        assert_eq!(
            outcome.empty_reason,
            Some(SourceEmptyReason::OutsideTrackingWindow)
        );
        assert_eq!(outcome.report.as_ref().unwrap().duplicates, 1);
        assert_eq!(outcome.report.as_ref().unwrap().skipped_outside_tracking, 1);
    }
}
