use std::path::Path;

use token_fire::adapters::source::SourceContext;
use token_fire::adapters::traex::parser::TraexParser;
use token_fire::adapters::transcript::TranscriptParser;
use token_fire::core::observation::SourceRecordIdConfidence;

#[test]
fn parses_last_token_usage_and_turn_boundary_without_prompt_content() {
    let content = include_str!("fixtures/traex-session.jsonl");
    let report = TraexParser::default()
        .parse_str(Path::new("/tmp/rollout-019-session-a.jsonl"), content)
        .unwrap();

    assert_eq!(report.observations.len(), 2);
    let first = &report.observations[0];
    assert_eq!(first.source, "traex");
    assert_eq!(first.adapter_version, "traex-jsonl-v1");
    assert_eq!(first.session_id.as_deref(), Some("019-session-a"));
    assert_eq!(first.turn_id.as_deref(), Some("turn-a"));
    assert_eq!(first.turn_boundary_id.as_deref(), Some("turn-a"));
    assert_eq!(first.input_tokens, 100);
    assert_eq!(first.output_tokens, 20);
    assert_eq!(first.cached_input_tokens, 5);
    assert_eq!(first.cache_creation_input_tokens, 3);
    assert_eq!(first.reasoning_output_tokens, 2);
    assert_eq!(first.total_tokens, 130);
    assert_eq!(first.cumulative_total_tokens, Some(130));
    assert_eq!(
        first.source_record_id_confidence,
        SourceRecordIdConfidence::Exact
    );
    assert!(!serde_json::to_string(first)
        .unwrap()
        .contains("redacted by fixture"));
}

#[test]
fn shared_parser_uses_explicit_codex_source_context() {
    let content = include_str!("fixtures/codex-session.jsonl");
    let report = TranscriptParser::new(SourceContext::codex())
        .parse_str(Path::new("/tmp/rollout-019-codex-session.jsonl"), content)
        .unwrap();

    assert_eq!(report.observations.len(), 2);
    let first = &report.observations[0];
    assert_eq!(first.source, "codex");
    assert_eq!(first.adapter_version, "codex-jsonl-v1");
    assert_eq!(first.session_id.as_deref(), Some("019-codex-session"));
    assert_eq!(first.turn_id.as_deref(), Some("turn-codex"));
    assert_eq!(first.total_tokens, 64);
    assert_ne!(first.source, "Codex Desktop");
    assert_ne!(first.source, "openai");
}

#[test]
fn shared_parser_does_not_use_raw_originator_as_source() {
    let content = r#"{"type":"session_meta","timestamp":"2026-06-26T03:00:00.000Z","payload":{"id":"session-source-test","originator":"Codex Desktop","model_provider":"openai"}}
{"type":"turn_context","timestamp":"2026-06-26T03:00:01.000Z","payload":{"turn_id":"turn-source-test","model":"gpt-5","cwd":"/tmp/project"}}
{"type":"event_msg","timestamp":"2026-06-26T03:00:02.000Z","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":7,"output_tokens":3,"cached_input_tokens":0,"cache_creation_input_tokens":0,"reasoning_output_tokens":0,"total_tokens":10},"total_token_usage":{"input_tokens":7,"output_tokens":3,"cached_input_tokens":0,"cache_creation_input_tokens":0,"reasoning_output_tokens":0,"total_tokens":10}}}}
"#;

    let report = TranscriptParser::new(SourceContext::codex())
        .parse_str(Path::new("/tmp/rollout-session-source-test.jsonl"), content)
        .unwrap();

    assert_eq!(report.observations[0].source, "codex");
    assert_eq!(report.observations[0].adapter_version, "codex-jsonl-v1");
}

#[test]
fn falls_back_to_cumulative_deltas_when_last_usage_is_missing() {
    let content = r#"{"type":"session_meta","timestamp":"2026-06-20T03:00:00.000Z","payload":{"id":"session-c"}}
{"type":"event_msg","timestamp":"2026-06-20T03:01:02.000Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"output_tokens":5,"cached_input_tokens":0,"cache_creation_input_tokens":0,"reasoning_output_tokens":0,"total_tokens":15}}}}
{"type":"event_msg","timestamp":"2026-06-20T03:01:03.000Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":15,"output_tokens":8,"cached_input_tokens":1,"cache_creation_input_tokens":0,"reasoning_output_tokens":0,"total_tokens":24}}}}
"#;

    let report = TraexParser::default()
        .parse_str(Path::new("/tmp/rollout-session-c.jsonl"), content)
        .unwrap();

    assert_eq!(report.observations.len(), 1);
    assert_eq!(report.observations[0].input_tokens, 5);
    assert_eq!(report.observations[0].output_tokens, 3);
    assert_eq!(report.observations[0].cached_input_tokens, 1);
    assert_eq!(report.observations[0].total_tokens, 9);
}

#[test]
fn skips_partial_jsonl_lines_and_warns_on_negative_delta() {
    let content = r#"{"type":"session_meta","timestamp":"2026-06-20T03:00:00.000Z","payload":{"id":"session-d"}}
{"type":"event_msg","timestamp":"2026-06-20T03:01:02.000Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"output_tokens":5,"total_tokens":15}}}}
{"type":"event_msg","timestamp":"2026-06-20T03:01:03.000Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":5,"output_tokens":2,"total_tokens":7}}}}
{"type":"event_msg","timestamp":"2026-06-20T03:01:04.000Z","payload":{"type":"token_count"
"#;

    let report = TraexParser::default()
        .parse_str(Path::new("/tmp/rollout-session-d.jsonl"), content)
        .unwrap();

    assert!(report.observations.is_empty());
    assert!(report
        .warnings
        .iter()
        .any(|warning| warning.contains("negative fallback delta")));
    assert!(report
        .warnings
        .iter()
        .any(|warning| warning.contains("partial jsonl line")));
}

#[test]
fn skips_token_rows_without_source_timestamp_instead_of_using_ingest_time() {
    let content = r#"{"type":"session_meta","timestamp":"2026-06-20T03:00:00.000Z","payload":{"id":"session-e"}}
{"type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":1,"output_tokens":2,"cached_input_tokens":0,"cache_creation_input_tokens":0,"reasoning_output_tokens":0,"total_tokens":3},"total_token_usage":{"input_tokens":1,"output_tokens":2,"cached_input_tokens":0,"cache_creation_input_tokens":0,"reasoning_output_tokens":0,"total_tokens":3}}}}
"#;

    let report = TraexParser::default()
        .parse_str(Path::new("/tmp/rollout-session-e.jsonl"), content)
        .unwrap();

    assert!(report.observations.is_empty());
    assert!(report
        .warnings
        .iter()
        .any(|warning| warning.contains("missing token timestamp")));
}

#[test]
fn derives_session_identity_from_filename_or_fallback() {
    let parser = TraexParser::default();

    let filename_report = parser
        .parse_str(
            Path::new("/tmp/rollout-019-filename-session.jsonl"),
            include_str!("fixtures/traex-session-filename-id.jsonl"),
        )
        .unwrap();
    assert_eq!(
        filename_report.observations[0].session_id.as_deref(),
        Some("019-filename-session")
    );
    assert_eq!(
        filename_report.observations[0].source_record_id_confidence,
        SourceRecordIdConfidence::Exact
    );

    let fallback_report = parser
        .parse_str(
            Path::new("/tmp/no-session-name.jsonl"),
            include_str!("fixtures/traex-session-fallback-id.jsonl"),
        )
        .unwrap();
    assert_eq!(fallback_report.observations[0].session_id, None);
    assert_eq!(
        fallback_report.observations[0].source_record_id_confidence,
        SourceRecordIdConfidence::Fallback
    );
    assert!(fallback_report
        .warnings
        .iter()
        .any(|warning| warning.contains("fallback_identity_used")));
}

#[test]
fn derives_session_identity_from_real_rollout_filename_id_segment() {
    let session_id = "019ee5e5-1425-7481-83a4-4c03f2f1a987";
    let content = format!(
        r#"{{"type":"session_meta","timestamp":"2026-06-20T03:00:00.000Z","payload":{{"id":"{session_id}"}}}}
{{"type":"event_msg","timestamp":"2026-06-20T03:01:00.000Z","payload":{{"type":"user_message"}}}}
{{"type":"event_msg","timestamp":"2026-06-20T03:01:02.000Z","payload":{{"type":"token_count","info":{{"last_token_usage":{{"input_tokens":1,"output_tokens":2,"cached_input_tokens":0,"cache_creation_input_tokens":0,"reasoning_output_tokens":0,"total_tokens":3}},"total_token_usage":{{"input_tokens":1,"output_tokens":2,"cached_input_tokens":0,"cache_creation_input_tokens":0,"reasoning_output_tokens":0,"total_tokens":3}}}}}}}}
"#
    );

    let report = TraexParser::default()
        .parse_str(
            Path::new(
                "/tmp/rollout-2026-06-21T00-37-35-019ee5e5-1425-7481-83a4-4c03f2f1a987.jsonl",
            ),
            &content,
        )
        .unwrap();

    assert_eq!(report.observations.len(), 1);
    assert_eq!(
        report.observations[0].session_id.as_deref(),
        Some(session_id)
    );
    assert!(!report
        .warnings
        .iter()
        .any(|warning| warning.contains("session identity mismatch")));
}
