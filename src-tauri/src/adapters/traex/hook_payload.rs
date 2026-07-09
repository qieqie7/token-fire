use serde_json::Value;

use crate::adapters::HookMetadata;

pub fn filter_hook_payload(value: Value) -> HookMetadata {
    filter_hook_payload_with_source(value, None)
}

pub fn filter_hook_payload_with_source(
    value: Value,
    source_override: Option<&str>,
) -> HookMetadata {
    let source_override = source_override.filter(|source| is_allowed_source(source));
    let payload_source = value
        .get("source")
        .and_then(Value::as_str)
        .filter(|source| is_allowed_source(source));
    let source = source_override.or(payload_source);

    HookMetadata {
        source: source.map(str::to_string),
        hook_event_name: value
            .get("hook_event_name")
            .and_then(Value::as_str)
            .map(str::to_string),
        session_id: value
            .get("session_id")
            .and_then(Value::as_str)
            .map(str::to_string),
        transcript_path: value
            .get("transcript_path")
            .and_then(Value::as_str)
            .map(str::to_string),
        conversation_id: value
            .get("conversation_id")
            .and_then(Value::as_str)
            .map(str::to_string),
        turn_id: value
            .get("turn_id")
            .and_then(Value::as_str)
            .map(str::to_string),
        model: value
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_string),
        cwd: value.get("cwd").and_then(Value::as_str).map(str::to_string),
        timestamp: value
            .get("timestamp")
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

fn is_allowed_source(source: &str) -> bool {
    matches!(source, "traex" | "codex" | "claude" | "cursor")
}
