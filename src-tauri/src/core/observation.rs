use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceRecordIdConfidence {
    Exact,
    Fallback,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NormalizedObservation {
    pub source: String,
    pub adapter_version: String,
    pub source_record_id: String,
    pub source_record_id_confidence: SourceRecordIdConfidence,
    pub session_id: Option<String>,
    pub turn_id: Option<String>,
    pub turn_boundary_id: Option<String>,
    pub source_path: Option<String>,
    pub line_no: Option<i64>,
    pub byte_offset: Option<i64>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cached_input_tokens: i64,
    pub cache_creation_input_tokens: i64,
    pub reasoning_output_tokens: i64,
    pub total_tokens: i64,
    pub cumulative_total_tokens: Option<i64>,
    pub model: Option<String>,
    pub cwd: Option<String>,
    pub observed_at: DateTime<Utc>,
    pub token_payload_hash: String,
}

pub fn validate_observation(observation: &NormalizedObservation) -> anyhow::Result<()> {
    if observation.source.trim().is_empty() {
        anyhow::bail!("source is required");
    }
    if observation.adapter_version.trim().is_empty() {
        anyhow::bail!("adapter_version is required");
    }
    if observation.source_record_id.trim().is_empty() {
        anyhow::bail!("source_record_id is required");
    }
    if observation.token_payload_hash.trim().is_empty() {
        anyhow::bail!("token_payload_hash is required");
    }
    let token_fields = [
        observation.input_tokens,
        observation.output_tokens,
        observation.cached_input_tokens,
        observation.cache_creation_input_tokens,
        observation.reasoning_output_tokens,
        observation.total_tokens,
    ];
    if token_fields.iter().any(|value| *value < 0) {
        anyhow::bail!("token fields must be non-negative");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn valid_observation() -> NormalizedObservation {
        NormalizedObservation {
            source: "source-a".to_string(),
            adapter_version: "adapter-v1".to_string(),
            source_record_id: "session-1:42".to_string(),
            source_record_id_confidence: SourceRecordIdConfidence::Exact,
            session_id: Some("session-1".to_string()),
            turn_id: Some("turn-1".to_string()),
            turn_boundary_id: Some("turn-1".to_string()),
            source_path: Some("/tmp/source-record.jsonl".to_string()),
            line_no: Some(3),
            byte_offset: Some(42),
            input_tokens: 10,
            output_tokens: 20,
            cached_input_tokens: 1,
            cache_creation_input_tokens: 2,
            reasoning_output_tokens: 3,
            total_tokens: 36,
            cumulative_total_tokens: Some(100),
            model: Some("model-a".to_string()),
            cwd: Some("~/project".to_string()),
            observed_at: Utc.with_ymd_and_hms(2026, 6, 20, 3, 0, 0).unwrap(),
            token_payload_hash: "hash-1".to_string(),
        }
    }

    #[test]
    fn accepts_valid_observation() {
        validate_observation(&valid_observation()).expect("valid observation");
    }

    #[test]
    fn rejects_missing_required_source() {
        let mut observation = valid_observation();
        observation.source.clear();

        let error = validate_observation(&observation).unwrap_err().to_string();

        assert!(error.contains("source is required"));
    }

    #[test]
    fn rejects_negative_token_delta() {
        let mut observation = valid_observation();
        observation.total_tokens = -1;

        let error = validate_observation(&observation).unwrap_err().to_string();

        assert!(error.contains("token fields must be non-negative"));
    }
}
