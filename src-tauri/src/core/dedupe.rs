use crate::core::observation::NormalizedObservation;

pub fn compute_dedupe_key(observation: &NormalizedObservation) -> String {
    if let (Some(session_id), Some(byte_offset)) =
        (observation.session_id.as_deref(), observation.byte_offset)
    {
        return format!(
            "transcript:{}:{}:{}",
            session_id, byte_offset, observation.token_payload_hash
        );
    }

    format!(
        "fallback:{}:{}:{}:{}",
        observation.source,
        observation.adapter_version,
        observation.source_record_id,
        observation.token_payload_hash
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::observation::{NormalizedObservation, SourceRecordIdConfidence};
    use chrono::{TimeZone, Utc};

    #[test]
    fn computes_dedupe_key_from_stable_normalized_fields() {
        let observation = NormalizedObservation {
            source: "source-a".to_string(),
            adapter_version: "adapter-v1".to_string(),
            source_record_id: "session-1:42".to_string(),
            source_record_id_confidence: SourceRecordIdConfidence::Exact,
            session_id: Some("session-1".to_string()),
            turn_id: None,
            turn_boundary_id: None,
            source_path: Some("/tmp/active/source-record.jsonl".to_string()),
            line_no: Some(7),
            byte_offset: Some(42),
            input_tokens: 1,
            output_tokens: 2,
            cached_input_tokens: 0,
            cache_creation_input_tokens: 0,
            reasoning_output_tokens: 0,
            total_tokens: 3,
            cumulative_total_tokens: None,
            model: None,
            cwd: None,
            observed_at: Utc.with_ymd_and_hms(2026, 6, 20, 3, 0, 0).unwrap(),
            token_payload_hash: "payload-hash".to_string(),
        };

        assert_eq!(
            compute_dedupe_key(&observation),
            "transcript:session-1:42:payload-hash"
        );
    }

    #[test]
    fn fallback_dedupe_key_keeps_source_when_session_or_offset_is_missing() {
        let mut observation = NormalizedObservation {
            source: "source-a".to_string(),
            adapter_version: "adapter-v1".to_string(),
            source_record_id: "fallback-record".to_string(),
            source_record_id_confidence: SourceRecordIdConfidence::Fallback,
            session_id: None,
            turn_id: None,
            turn_boundary_id: None,
            source_path: Some("/tmp/active/source-record.jsonl".to_string()),
            line_no: Some(7),
            byte_offset: None,
            input_tokens: 1,
            output_tokens: 2,
            cached_input_tokens: 0,
            cache_creation_input_tokens: 0,
            reasoning_output_tokens: 0,
            total_tokens: 3,
            cumulative_total_tokens: None,
            model: None,
            cwd: None,
            observed_at: Utc.with_ymd_and_hms(2026, 6, 20, 3, 0, 0).unwrap(),
            token_payload_hash: "payload-hash".to_string(),
        };

        assert_eq!(
            compute_dedupe_key(&observation),
            "fallback:source-a:adapter-v1:fallback-record:payload-hash"
        );

        observation.session_id = Some("session-1".to_string());
        assert!(compute_dedupe_key(&observation).starts_with("fallback:"));
    }
}
