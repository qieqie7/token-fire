use std::path::Path;
use std::{collections::BTreeMap, fs};

use chrono::{DateTime, Utc};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::adapters::source::TokenSourceKind;
use crate::adapters::HookMetadata;
use crate::core::observation::{NormalizedObservation, SourceRecordIdConfidence};

pub mod hook_config;

pub fn collect_from_transcript(
    path: &Path,
    metadata: &HookMetadata,
) -> anyhow::Result<Option<NormalizedObservation>> {
    let content = fs::read_to_string(path)?;
    let rows = latest_conversation_rows(&content)?;
    let mut keyed = BTreeMap::<ClaudeRecordKey, TokenComponents>::new();
    let mut unkeyed = TokenComponents::default();
    let mut first_text_model = None;
    let mut first_thinking_model = None;
    let mut transcript_session_id = None;
    let mut transcript_cwd = None;
    let mut latest_record_timestamp = None;

    for row in rows {
        let Some(usage) = row.pointer("/message/usage") else {
            continue;
        };
        let components = TokenComponents::from_usage(usage);
        let message = row.get("message").unwrap_or(&Value::Null);

        if let Some(key) = ClaudeRecordKey::from_row(&row) {
            keyed.entry(key).or_default().merge_keyed_max(&components);
        } else {
            unkeyed.add_unkeyed(&components);
        }

        if transcript_session_id.is_none() {
            transcript_session_id = row
                .get("session_id")
                .and_then(Value::as_str)
                .map(str::to_string);
        }
        if transcript_cwd.is_none() {
            transcript_cwd = row.get("cwd").and_then(Value::as_str).map(str::to_string);
        }
        if let Some(timestamp) = row
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(parse_timestamp)
        {
            latest_record_timestamp = Some(timestamp);
        }
        if let Some(model) = message.get("model").and_then(Value::as_str) {
            if first_text_model.is_none() && content_has_type(message, "text") {
                first_text_model = Some(model.to_string());
            }
            if first_thinking_model.is_none() && content_has_type(message, "thinking") {
                first_thinking_model = Some(model.to_string());
            }
        }
    }

    let mut total = unkeyed;
    for usage in keyed.values() {
        total.add_unkeyed(usage);
    }
    if total.total_tokens() == 0 {
        return Ok(None);
    }

    let source = TokenSourceKind::Claude;
    let session_id = metadata.session_id.clone().or(transcript_session_id);
    let turn_id = metadata.turn_id.clone();
    let model = first_text_model
        .or(first_thinking_model)
        .or_else(|| metadata.model.clone());
    let cwd = metadata.cwd.clone().or(transcript_cwd);
    let observed_at = metadata
        .timestamp
        .as_deref()
        .and_then(parse_timestamp)
        .or(latest_record_timestamp)
        .unwrap_or_else(Utc::now);
    let hash_input = json!({
        "source": source.as_str(),
        "adapter_version": source.adapter_version(),
        "session_id": session_id.as_deref(),
        "turn_id": turn_id.as_deref(),
        "input_tokens": total.input_tokens,
        "output_tokens": total.output_tokens,
        "cached_input_tokens": total.cached_input_tokens,
        "cache_creation_input_tokens": total.cache_creation_input_tokens,
        "reasoning_output_tokens": 0,
        "total_tokens": total.total_tokens(),
        "model": model.as_deref(),
    });
    let token_payload_hash = stable_hash(&hash_input)?;
    let (source_record_id, source_record_id_confidence) =
        match (session_id.as_deref(), turn_id.as_deref()) {
            (Some(session_id), Some(turn_id)) => (
                format!("{session_id}:{turn_id}"),
                SourceRecordIdConfidence::Exact,
            ),
            _ => (
                format!("{}:{token_payload_hash}", source.as_str()),
                SourceRecordIdConfidence::Fallback,
            ),
        };

    Ok(Some(NormalizedObservation {
        source: source.as_str().to_string(),
        adapter_version: source.adapter_version().to_string(),
        source_record_id,
        source_record_id_confidence,
        session_id,
        turn_id: turn_id.clone(),
        turn_boundary_id: turn_id,
        source_path: Some(path.to_string_lossy().to_string()),
        line_no: None,
        byte_offset: None,
        input_tokens: total.input_tokens,
        output_tokens: total.output_tokens,
        cached_input_tokens: total.cached_input_tokens,
        cache_creation_input_tokens: total.cache_creation_input_tokens,
        reasoning_output_tokens: 0,
        total_tokens: total.total_tokens(),
        cumulative_total_tokens: None,
        model,
        cwd,
        observed_at,
        token_payload_hash,
    }))
}

fn latest_conversation_rows(content: &str) -> anyhow::Result<Vec<Value>> {
    let mut rows = Vec::new();
    let mut last_user_index = None;

    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let row: Value = serde_json::from_str(line)?;
        if row
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|value| value == "user")
        {
            last_user_index = Some(rows.len());
        }
        rows.push(row);
    }

    let Some(last_user_index) = last_user_index else {
        return Ok(rows);
    };
    Ok(rows.into_iter().skip(last_user_index + 1).collect())
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ClaudeRecordKey {
    message_id: String,
    request_id: String,
}

impl ClaudeRecordKey {
    fn from_row(row: &Value) -> Option<Self> {
        let message_id = row.pointer("/message/id").and_then(Value::as_str)?;
        let request_id = row.get("requestId").and_then(Value::as_str)?;
        if message_id.is_empty() || request_id.is_empty() {
            return None;
        }
        Some(Self {
            message_id: message_id.to_string(),
            request_id: request_id.to_string(),
        })
    }
}

#[derive(Debug, Clone, Default)]
struct TokenComponents {
    input_tokens: i64,
    output_tokens: i64,
    cached_input_tokens: i64,
    cache_creation_input_tokens: i64,
}

impl TokenComponents {
    fn from_usage(usage: &Value) -> Self {
        Self {
            input_tokens: int_field(usage, "input_tokens"),
            output_tokens: int_field(usage, "output_tokens"),
            cached_input_tokens: int_field(usage, "cache_read_input_tokens"),
            cache_creation_input_tokens: int_field(usage, "cache_creation_input_tokens"),
        }
    }

    fn merge_keyed_max(&mut self, other: &Self) {
        self.input_tokens = self.input_tokens.max(other.input_tokens);
        self.output_tokens = self.output_tokens.max(other.output_tokens);
        self.cached_input_tokens = self.cached_input_tokens.max(other.cached_input_tokens);
        self.cache_creation_input_tokens = self
            .cache_creation_input_tokens
            .max(other.cache_creation_input_tokens);
    }

    fn add_unkeyed(&mut self, other: &Self) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cached_input_tokens += other.cached_input_tokens;
        self.cache_creation_input_tokens += other.cache_creation_input_tokens;
    }

    fn total_tokens(&self) -> i64 {
        self.input_tokens + self.output_tokens
    }
}

fn int_field(value: &Value, key: &str) -> i64 {
    value
        .get(key)
        .and_then(|field| {
            field
                .as_i64()
                .or_else(|| field.as_u64().and_then(|value| i64::try_from(value).ok()))
        })
        .unwrap_or(0)
        .max(0)
}

fn content_has_type(message: &Value, block_type: &str) -> bool {
    message
        .get("content")
        .and_then(Value::as_array)
        .map(|items| {
            items.iter().any(|item| {
                item.get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|value| value == block_type)
            })
        })
        .unwrap_or(false)
}

fn parse_timestamp(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

fn stable_hash(value: &Value) -> anyhow::Result<String> {
    let canonical = canonical_json(value);
    let bytes = serde_json::to_vec(&canonical)?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn canonical_json(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut sorted = BTreeMap::new();
            for (key, value) in map {
                sorted.insert(key.clone(), canonical_json(value));
            }
            Value::Object(sorted.into_iter().collect::<Map<String, Value>>())
        }
        Value::Array(values) => Value::Array(values.iter().map(canonical_json).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::source::TokenSourceKind;
    use crate::core::observation::SourceRecordIdConfidence;

    #[test]
    fn claude_fixture_applies_flux_keyed_max_without_content_storage() {
        let dir = tempfile::tempdir().unwrap();
        let transcript = dir.path().join("claude-transcript.jsonl");
        std::fs::write(
            &transcript,
            include_str!("../../../tests/fixtures/claude-transcript.jsonl"),
        )
        .unwrap();
        let metadata = HookMetadata {
            source: Some("claude".to_string()),
            hook_event_name: Some("Stop".to_string()),
            transcript_path: Some(transcript.to_string_lossy().to_string()),
            session_id: Some("claude-session-1".to_string()),
            turn_id: Some("claude-turn-1".to_string()),
            cwd: Some("/Users/example/claude-project".to_string()),
            timestamp: Some("2026-07-05T10:00:03Z".to_string()),
            ..HookMetadata::default()
        };

        let observation = collect_from_transcript(&transcript, &metadata)
            .unwrap()
            .expect("observation");

        assert_eq!(observation.source, TokenSourceKind::Claude.as_str());
        assert_eq!(
            observation.adapter_version,
            TokenSourceKind::Claude.adapter_version()
        );
        assert_eq!(
            observation.source_record_id_confidence,
            SourceRecordIdConfidence::Exact
        );
        assert_eq!(observation.session_id.as_deref(), Some("claude-session-1"));
        assert_eq!(observation.turn_id.as_deref(), Some("claude-turn-1"));
        assert_eq!(observation.input_tokens, 140);
        assert_eq!(observation.output_tokens, 54);
        assert_eq!(observation.cached_input_tokens, 20);
        assert_eq!(observation.cache_creation_input_tokens, 5);
        assert_eq!(observation.total_tokens, 194);
        assert_eq!(observation.model.as_deref(), Some("claude-sonnet-4"));
        let debug = format!("{observation:?}");
        assert!(!debug.contains("SENTINEL_ASSISTANT_TEXT"));
        assert!(!debug.contains("SENTINEL_DUPLICATE_SNAPSHOT"));
        assert!(!debug.contains("SENTINEL_THINKING_TEXT"));
    }

    #[test]
    fn claude_collector_uses_latest_conversation_segment_only() {
        let dir = tempfile::tempdir().unwrap();
        let transcript = dir.path().join("claude-latest-turn.jsonl");
        std::fs::write(
            &transcript,
            r#"{"type":"user","timestamp":"2026-07-05T10:00:00Z","cwd":"/Users/example/claude-project","session_id":"claude-session-1","message":{"content":[{"type":"text","text":"SENTINEL_OLD_USER"}]}}
{"type":"assistant","timestamp":"2026-07-05T10:00:01Z","cwd":"/Users/example/claude-project","session_id":"claude-session-1","message":{"id":"msg-old","model":"claude-sonnet-old","usage":{"input_tokens":1000,"output_tokens":500,"cache_read_input_tokens":200,"cache_creation_input_tokens":50},"content":[{"type":"text","text":"SENTINEL_OLD_ASSISTANT"}]},"requestId":"req-old"}
{"type":"user","timestamp":"2026-07-05T10:00:02Z","cwd":"/Users/example/claude-project","session_id":"claude-session-1","message":{"content":[{"type":"text","text":"SENTINEL_LATEST_USER"}]}}
{"type":"assistant","timestamp":"2026-07-05T10:00:03Z","cwd":"/Users/example/claude-project","session_id":"claude-session-1","message":{"id":"msg-latest","model":"claude-sonnet-latest","usage":{"input_tokens":20,"output_tokens":7,"cache_read_input_tokens":3,"cache_creation_input_tokens":2},"content":[{"type":"text","text":"SENTINEL_LATEST_ASSISTANT"}]},"requestId":"req-latest"}
"#,
        )
        .unwrap();
        let metadata = HookMetadata {
            source: Some("claude".to_string()),
            hook_event_name: Some("Stop".to_string()),
            transcript_path: Some(transcript.to_string_lossy().to_string()),
            ..HookMetadata::default()
        };

        let observation = collect_from_transcript(&transcript, &metadata)
            .unwrap()
            .expect("observation");

        assert_eq!(observation.input_tokens, 20);
        assert_eq!(observation.output_tokens, 7);
        assert_eq!(observation.cached_input_tokens, 3);
        assert_eq!(observation.cache_creation_input_tokens, 2);
        assert_eq!(observation.total_tokens, 27);
        assert_eq!(observation.model.as_deref(), Some("claude-sonnet-latest"));
        let debug = format!("{observation:?}");
        assert!(!debug.contains("SENTINEL_OLD_USER"));
        assert!(!debug.contains("SENTINEL_OLD_ASSISTANT"));
        assert!(!debug.contains("SENTINEL_LATEST_USER"));
        assert!(!debug.contains("SENTINEL_LATEST_ASSISTANT"));
    }

    #[test]
    fn claude_collector_returns_none_for_empty_usage() {
        let dir = tempfile::tempdir().unwrap();
        let transcript = dir.path().join("empty-claude.jsonl");
        std::fs::write(
            &transcript,
            r#"{"type":"assistant","timestamp":"2026-07-05T10:00:00Z","message":{"id":"msg","usage":{"input_tokens":0,"output_tokens":0}},"requestId":"req"}"#,
        )
        .unwrap();

        let metadata = HookMetadata {
            source: Some("claude".to_string()),
            transcript_path: Some(transcript.to_string_lossy().to_string()),
            ..HookMetadata::default()
        };

        assert!(collect_from_transcript(&transcript, &metadata)
            .unwrap()
            .is_none());
    }
}
