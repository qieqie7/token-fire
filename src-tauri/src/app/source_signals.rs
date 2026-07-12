use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};

use crate::adapters::source::TokenSourceKind;
use crate::app::ingest_scheduler::IngestReport;
use crate::app::source_ingest::{
    SourceEmptyReason, SourceErrorKind, SourceIngestEvent, SourceIngestOutcome, SourceResolution,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSignalRecord {
    pub source: TokenSourceKind,
    pub event: SourceIngestEvent,
    pub resolution: SourceResolution,
    pub seen_at: DateTime<Utc>,
    pub inserted: Option<usize>,
    pub duplicates: Option<usize>,
    pub skipped_outside_tracking: Option<usize>,
    pub empty_reason: Option<SourceEmptyReason>,
    pub error_kind: Option<String>,
}

impl SourceSignalRecord {
    pub fn from_outcome(outcome: &SourceIngestOutcome, seen_at: DateTime<Utc>) -> Self {
        let report: Option<&IngestReport> = outcome.report.as_ref();
        Self {
            source: outcome.source,
            event: outcome.event,
            resolution: outcome.resolution,
            seen_at,
            inserted: report.map(|report| report.inserted),
            duplicates: report.map(|report| report.duplicates),
            skipped_outside_tracking: report.map(|report| report.skipped_outside_tracking),
            empty_reason: outcome.empty_reason,
            error_kind: None,
        }
    }

    pub fn from_error(
        source: TokenSourceKind,
        event: SourceIngestEvent,
        seen_at: DateTime<Utc>,
        error_kind: SourceErrorKind,
    ) -> Self {
        Self {
            source,
            event,
            resolution: SourceResolution::None,
            seen_at,
            inserted: None,
            duplicates: None,
            skipped_outside_tracking: None,
            empty_reason: None,
            error_kind: Some(error_kind.as_str().to_string()),
        }
    }

    pub fn inserted_tokens(&self) -> bool {
        self.inserted.is_some_and(|inserted| inserted > 0)
    }

    pub fn benign_noop(&self) -> bool {
        self.inserted == Some(0)
            && matches!(
                self.empty_reason,
                Some(
                    SourceEmptyReason::NoNewCompleteRound
                        | SourceEmptyReason::WatermarkAtEof
                        | SourceEmptyReason::DuplicateOnly
                )
            )
            && self.error_kind.is_none()
    }

    pub fn hard_failure(&self) -> bool {
        self.error_kind.is_some()
            || matches!(
                self.empty_reason,
                Some(
                    SourceEmptyReason::UnsupportedHookEvent
                        | SourceEmptyReason::InputMissing
                        | SourceEmptyReason::TranscriptPathMissing
                        | SourceEmptyReason::TranscriptUnreadable
                        | SourceEmptyReason::NoCompleteJsonlRows
                )
            )
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceSignalState {
    pub latest_signal: Option<SourceSignalRecord>,
    pub latest_success: Option<SourceSignalRecord>,
    pub latest_hard_failure: Option<SourceSignalRecord>,
}

#[derive(Debug, Clone, Default)]
pub struct RecentSourceSignals {
    inner: Arc<Mutex<HashMap<TokenSourceKind, SourceSignalState>>>,
}

impl RecentSourceSignals {
    pub fn record(&self, record: SourceSignalRecord) {
        let mut inner = self.inner.lock().unwrap();
        let state = inner.entry(record.source).or_default();
        update_if_newer_or_same(&mut state.latest_signal, record.clone());
        if record.inserted_tokens() {
            update_if_newer_or_same(&mut state.latest_success, record.clone());
        }
        if record.hard_failure() {
            update_if_newer_or_same(&mut state.latest_hard_failure, record);
        }
    }

    pub fn latest(&self, source: TokenSourceKind) -> Option<SourceSignalRecord> {
        self.inner
            .lock()
            .unwrap()
            .get(&source)
            .and_then(|state| state.latest_signal.clone())
    }

    pub fn latest_state(&self, source: TokenSourceKind) -> Option<SourceSignalState> {
        self.inner.lock().unwrap().get(&source).cloned()
    }

    pub fn snapshot(&self) -> HashMap<TokenSourceKind, SourceSignalRecord> {
        self.inner
            .lock()
            .unwrap()
            .iter()
            .filter_map(|(source, state)| {
                state.latest_signal.clone().map(|record| (*source, record))
            })
            .collect()
    }

    pub fn state_snapshot(&self) -> HashMap<TokenSourceKind, SourceSignalState> {
        self.inner.lock().unwrap().clone()
    }
}

fn update_if_newer_or_same(slot: &mut Option<SourceSignalRecord>, record: SourceSignalRecord) {
    if slot
        .as_ref()
        .is_none_or(|current| record.seen_at >= current.seen_at)
    {
        *slot = Some(record);
    }
}
