use std::collections::BTreeMap;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::adapters::source::SourceContext;
use crate::core::observation::{NormalizedObservation, SourceRecordIdConfidence};

#[derive(Debug, Clone)]
pub struct TranscriptParser {
    source_context: SourceContext,
}

impl TranscriptParser {
    pub fn new(source_context: SourceContext) -> Self {
        Self { source_context }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseReport {
    pub observations: Vec<NormalizedObservation>,
    pub warnings: Vec<String>,
    pub last_byte_offset: i64,
    pub safe_processed_offset: i64,
}

#[derive(Debug, Clone, Default)]
struct ParserContext {
    session_id: Option<String>,
    turn_id: Option<String>,
    model: Option<String>,
    cwd: Option<String>,
    user_boundary_offset: Option<i64>,
    previous_total: Option<TokenUsage>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct TokenUsage {
    input_tokens: i64,
    output_tokens: i64,
    cached_input_tokens: i64,
    cache_creation_input_tokens: i64,
    reasoning_output_tokens: i64,
    total_tokens: i64,
}

impl TranscriptParser {
    pub fn parse_str(&self, path: &Path, content: &str) -> anyhow::Result<ParseReport> {
        let mut observations = Vec::new();
        let mut warnings = Vec::new();
        let mut context = ParserContext::default();
        let mut byte_offset = 0_i64;
        let mut safe_processed_offset = -1_i64;

        for (index, line) in content.split_inclusive('\n').enumerate() {
            let line_start = byte_offset;
            byte_offset += line.len() as i64;
            let has_line_terminator = line.ends_with('\n');

            let trimmed = line.trim_end_matches('\n');
            if trimmed.trim().is_empty() {
                if has_line_terminator {
                    safe_processed_offset = line_start;
                } else {
                    warnings.push(format!("partial jsonl line at byte {line_start}"));
                }
                continue;
            }

            let row: Value = match serde_json::from_str(trimmed) {
                Ok(row) => row,
                Err(_) => {
                    warnings.push(format!("partial jsonl line at byte {line_start}"));
                    if has_line_terminator {
                        safe_processed_offset = line_start;
                    }
                    continue;
                }
            };
            safe_processed_offset = line_start;

            match row.get("type").and_then(Value::as_str) {
                Some("session_meta") => {
                    if let Some(id) = row.pointer("/payload/id").and_then(Value::as_str) {
                        let filename_id = session_id_from_filename(path);
                        if let Some(filename_id) = filename_id {
                            if filename_id != id {
                                warnings.push(
                                    "session identity mismatch; preferred session_meta.payload.id"
                                        .to_string(),
                                );
                            }
                        }
                        context.session_id = Some(id.to_string());
                    }
                }
                Some("turn_context") => {
                    context.turn_id = row
                        .pointer("/payload/turn_id")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    context.model = row
                        .pointer("/payload/model")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    context.cwd = row
                        .pointer("/payload/cwd")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                }
                Some("event_msg") => match row.pointer("/payload/type").and_then(Value::as_str) {
                    Some("user_message") => {
                        context.user_boundary_offset = Some(line_start);
                    }
                    Some("token_count") => {
                        if let Some(observation) = parse_token_row(
                            self.source_context,
                            path,
                            index as i64 + 1,
                            line_start,
                            &row,
                            &mut context,
                            &mut warnings,
                        )? {
                            observations.push(observation);
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
        }

        Ok(ParseReport {
            observations,
            warnings,
            last_byte_offset: byte_offset,
            safe_processed_offset,
        })
    }
}

fn parse_token_row(
    source_context: SourceContext,
    path: &Path,
    line_no: i64,
    byte_offset: i64,
    row: &Value,
    context: &mut ParserContext,
    warnings: &mut Vec<String>,
) -> anyhow::Result<Option<NormalizedObservation>> {
    let info = row.pointer("/payload/info").cloned().unwrap_or(Value::Null);
    let Some(observed_at) = row
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc))
    else {
        warnings.push(format!("missing token timestamp at byte {byte_offset}"));
        return Ok(None);
    };

    let cumulative = usage_from_value(info.get("total_token_usage"));
    let delta = match usage_from_value(info.get("last_token_usage")) {
        Some(last) if last.total_tokens > 0 => Some(last),
        Some(last) if last.total_tokens == 0 && last.has_non_zero_component() => {
            warnings.push(format!(
                "last_token_usage total is zero with non-zero components at byte {byte_offset}"
            ));
            None
        }
        _ => fallback_delta(
            context.previous_total.as_ref(),
            cumulative.as_ref(),
            warnings,
            byte_offset,
        ),
    };

    if let Some(cumulative) = cumulative.clone() {
        context.previous_total = Some(cumulative);
    }

    let Some(delta) = delta else {
        return Ok(None);
    };

    let filename_id = session_id_from_filename(path);
    let session_id = context.session_id.clone().or(filename_id);
    let (source_record_id, confidence) = match session_id.clone() {
        Some(session_id) => (
            format!("{session_id}:{byte_offset}"),
            SourceRecordIdConfidence::Exact,
        ),
        None => {
            let basename = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("unknown");
            let hash = token_payload_hash(&info)?;
            warnings.push(format!("fallback_identity_used at byte {byte_offset}"));
            (
                format!("{basename}:{byte_offset}:{hash}"),
                SourceRecordIdConfidence::Fallback,
            )
        }
    };

    let turn_boundary_id = context.turn_id.clone().or_else(|| {
        context
            .user_boundary_offset
            .map(|offset| offset.to_string())
    });
    if turn_boundary_id.is_none() {
        warnings.push(format!("missing turn boundary at byte {byte_offset}"));
    }

    Ok(Some(NormalizedObservation {
        source: source_context.source.as_str().to_string(),
        adapter_version: source_context.adapter_version.to_string(),
        source_record_id,
        source_record_id_confidence: confidence,
        session_id,
        turn_id: context.turn_id.clone(),
        turn_boundary_id,
        source_path: Some(path.to_string_lossy().to_string()),
        line_no: Some(line_no),
        byte_offset: Some(byte_offset),
        input_tokens: delta.input_tokens,
        output_tokens: delta.output_tokens,
        cached_input_tokens: delta.cached_input_tokens,
        cache_creation_input_tokens: delta.cache_creation_input_tokens,
        reasoning_output_tokens: delta.reasoning_output_tokens,
        total_tokens: delta.total_tokens,
        cumulative_total_tokens: cumulative.map(|value| value.total_tokens),
        model: context.model.clone(),
        cwd: context.cwd.clone(),
        observed_at,
        token_payload_hash: token_payload_hash(&info)?,
    }))
}

fn usage_from_value(value: Option<&Value>) -> Option<TokenUsage> {
    let value = value?;
    Some(TokenUsage {
        input_tokens: int_field(value, "input_tokens"),
        output_tokens: int_field(value, "output_tokens"),
        cached_input_tokens: int_field(value, "cached_input_tokens"),
        cache_creation_input_tokens: int_field(value, "cache_creation_input_tokens"),
        reasoning_output_tokens: int_field(value, "reasoning_output_tokens"),
        total_tokens: int_field(value, "total_tokens"),
    })
}

fn int_field(value: &Value, key: &str) -> i64 {
    value.get(key).and_then(Value::as_i64).unwrap_or(0)
}

fn fallback_delta(
    previous: Option<&TokenUsage>,
    cumulative: Option<&TokenUsage>,
    warnings: &mut Vec<String>,
    byte_offset: i64,
) -> Option<TokenUsage> {
    let previous = previous?;
    let cumulative = cumulative?;
    let delta = TokenUsage {
        input_tokens: cumulative.input_tokens - previous.input_tokens,
        output_tokens: cumulative.output_tokens - previous.output_tokens,
        cached_input_tokens: cumulative.cached_input_tokens - previous.cached_input_tokens,
        cache_creation_input_tokens: cumulative.cache_creation_input_tokens
            - previous.cache_creation_input_tokens,
        reasoning_output_tokens: cumulative.reasoning_output_tokens
            - previous.reasoning_output_tokens,
        total_tokens: cumulative.total_tokens - previous.total_tokens,
    };
    if delta.any_negative() || delta.total_tokens <= 0 {
        warnings.push(format!("negative fallback delta at byte {byte_offset}"));
        None
    } else {
        Some(delta)
    }
}

impl TokenUsage {
    fn any_negative(&self) -> bool {
        [
            self.input_tokens,
            self.output_tokens,
            self.cached_input_tokens,
            self.cache_creation_input_tokens,
            self.reasoning_output_tokens,
            self.total_tokens,
        ]
        .iter()
        .any(|value| *value < 0)
    }

    fn has_non_zero_component(&self) -> bool {
        [
            self.input_tokens,
            self.output_tokens,
            self.cached_input_tokens,
            self.cache_creation_input_tokens,
            self.reasoning_output_tokens,
        ]
        .iter()
        .any(|value| *value != 0)
    }
}

fn session_id_from_filename(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_string_lossy();
    let rollout = stem.strip_prefix("rollout-")?;
    uuid_suffix(rollout).or_else(|| Some(rollout.to_string()))
}

fn uuid_suffix(value: &str) -> Option<String> {
    let parts = value.split('-').collect::<Vec<_>>();
    if parts.len() < 5 {
        return None;
    }
    let suffix = &parts[parts.len() - 5..];
    let expected_lengths = [8, 4, 4, 4, 12];
    let is_uuid = suffix
        .iter()
        .zip(expected_lengths)
        .all(|(part, expected_len)| {
            part.len() == expected_len && part.chars().all(|ch| ch.is_ascii_hexdigit())
        });
    is_uuid.then(|| suffix.join("-"))
}

fn token_payload_hash(value: &Value) -> anyhow::Result<String> {
    let canonical = canonical_json(value);
    let bytes = serde_json::to_vec(&canonical)?;
    let digest = Sha256::digest(bytes);
    Ok(hex::encode(digest))
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
