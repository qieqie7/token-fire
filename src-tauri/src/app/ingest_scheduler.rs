use std::fs;
use std::path::Path;

use crate::adapters::source::{SourceContext, TokenSourceKind};
use crate::adapters::transcript::TranscriptParser;
use crate::app::logging::{LogFile, RuntimeLogSinks};
use crate::app::widget_events::WidgetStateChangedEvent;
use crate::core::observation::NormalizedObservation;
use crate::core::usage_store::{InsertOutcome, UsageStore};
use serde_json::json;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestReport {
    pub inserted: usize,
    pub duplicates: usize,
    pub skipped_outside_tracking: usize,
    pub last_processed_offset: i64,
}

pub struct IngestScheduler {
    store: UsageStore,
    log_sinks: Option<RuntimeLogSinks>,
}

impl IngestScheduler {
    pub fn new(store: UsageStore) -> Self {
        Self {
            store,
            log_sinks: None,
        }
    }

    pub fn new_with_logs(store: UsageStore, log_sinks: RuntimeLogSinks) -> Self {
        Self {
            store,
            log_sinks: Some(log_sinks),
        }
    }

    pub(crate) fn log(
        &self,
        file: LogFile,
        component: &str,
        level: &str,
        event: &str,
        fields: serde_json::Value,
    ) {
        if let Some(log_sinks) = &self.log_sinks {
            log_sinks.write(file, component, level, event, fields);
        }
    }

    pub fn ingest_path(&self, path: &Path) -> anyhow::Result<IngestReport> {
        self.ingest_path_for_source(SourceContext::traex(), path)
    }

    pub fn widget_state_changed_event(
        &self,
        inserted: usize,
    ) -> anyhow::Result<WidgetStateChangedEvent> {
        Ok(WidgetStateChangedEvent {
            state_revision: self.store.state_revision()?,
            last_observed_at: self.store.last_observed_at()?,
            inserted,
        })
    }

    pub fn ingest_path_for_source(
        &self,
        source_context: SourceContext,
        path: &Path,
    ) -> anyhow::Result<IngestReport> {
        let source_path = path.to_string_lossy().to_string();
        let content = fs::read_to_string(path)?;
        let baseline = self.store.file_baseline(&source_path)?.unwrap_or(-1);
        let report = TranscriptParser::new(source_context).parse_str(path, &content)?;
        for warning in &report.warnings {
            self.log(
                LogFile::Parser,
                "parser",
                "warn",
                "parser_warning",
                json!({ "source_path": source_path.as_str(), "error_kind": warning }),
            );
        }

        let insert_report = self.ingest_observations_inner(
            report.observations,
            Some(baseline),
            report.safe_processed_offset,
        )?;
        self.store
            .set_file_baseline(&source_path, report.safe_processed_offset)?;
        Ok(insert_report)
    }

    pub fn ingest_observations_for_source(
        &self,
        source: TokenSourceKind,
        observations: Vec<NormalizedObservation>,
    ) -> anyhow::Result<IngestReport> {
        for observation in &observations {
            if observation.source != source.as_str() {
                anyhow::bail!(
                    "observation source {} does not match expected source {}",
                    observation.source,
                    source.as_str()
                );
            }
            if observation.adapter_version != source.adapter_version() {
                anyhow::bail!(
                    "observation adapter version {} does not match expected adapter version {}",
                    observation.adapter_version,
                    source.adapter_version()
                );
            }
        }

        self.ingest_observations_inner(observations, None, -1)
    }

    fn ingest_observations_inner(
        &self,
        observations: Vec<NormalizedObservation>,
        baseline: Option<i64>,
        last_processed_offset: i64,
    ) -> anyhow::Result<IngestReport> {
        let windows = self.store.tracking_windows_for_ingest()?;
        let mut inserted = 0;
        let mut duplicates = 0;
        let mut skipped_outside_tracking = 0;

        for observation in observations {
            self.log(
                LogFile::Parser,
                "parser",
                "info",
                "token_count_row_seen",
                json!({
                    "source": observation.source.as_str(),
                    "session_id": observation.session_id.as_deref(),
                    "turn_id": observation.turn_id.as_deref(),
                    "source_path": observation.source_path.as_deref(),
                    "byte_offset": observation.byte_offset,
                    "observed_at": observation.observed_at.to_rfc3339(),
                    "total_tokens": observation.total_tokens
                }),
            );
            let offset_ok = match baseline {
                Some(baseline) => observation.byte_offset.unwrap_or(0) > baseline,
                None => true,
            };
            let tracking_window_id = windows.iter().find_map(|window| {
                let in_window = observation.observed_at >= window.started_at
                    && window
                        .ended_at
                        .map_or(true, |ended_at| observation.observed_at < ended_at);
                in_window.then_some(window.id)
            });

            if !offset_ok || tracking_window_id.is_none() {
                skipped_outside_tracking += 1;
                if !offset_ok {
                    self.log(
                        LogFile::Db,
                        "db",
                        "warn",
                        "observation_duplicate",
                        json!({
                            "source": observation.source.as_str(),
                            "session_id": observation.session_id.as_deref(),
                            "source_path": observation.source_path.as_deref(),
                            "byte_offset": observation.byte_offset
                        }),
                    );
                }
                continue;
            }

            match self.store.insert_observation_for_tracking_window(
                &observation,
                tracking_window_id.expect("checked tracking window"),
            )? {
                InsertOutcome::Inserted => {
                    inserted += 1;
                    self.log(
                        LogFile::Db,
                        "db",
                        "info",
                        "observation_inserted",
                        json!({
                            "source": observation.source.as_str(),
                            "session_id": observation.session_id.as_deref(),
                            "turn_id": observation.turn_id.as_deref(),
                            "source_path": observation.source_path.as_deref(),
                            "byte_offset": observation.byte_offset,
                            "observed_at": observation.observed_at.to_rfc3339(),
                            "total_tokens": observation.total_tokens
                        }),
                    );
                }
                InsertOutcome::Duplicate => {
                    duplicates += 1;
                    self.log(
                        LogFile::Db,
                        "db",
                        "warn",
                        "observation_duplicate",
                        json!({
                            "source": observation.source.as_str(),
                            "session_id": observation.session_id.as_deref(),
                            "source_path": observation.source_path.as_deref(),
                            "byte_offset": observation.byte_offset
                        }),
                    );
                }
            }
        }

        Ok(IngestReport {
            inserted,
            duplicates,
            skipped_outside_tracking,
            last_processed_offset,
        })
    }
}
