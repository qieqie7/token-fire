use std::fs;

use chrono::{TimeZone, Utc};
use tempfile::tempdir;
use token_fire::adapters::source::{SourceContext, TokenSourceKind};
use token_fire::app::ingest_scheduler::IngestScheduler;
use token_fire::core::observation::{NormalizedObservation, SourceRecordIdConfidence};
use token_fire::core::usage_store::UsageStore;

fn normalized_source_observation(
    source: TokenSourceKind,
    record_id: &str,
    observed_at: chrono::DateTime<Utc>,
    total_tokens: i64,
) -> NormalizedObservation {
    NormalizedObservation {
        source: source.as_str().to_string(),
        adapter_version: source.adapter_version().to_string(),
        source_record_id: record_id.to_string(),
        source_record_id_confidence: SourceRecordIdConfidence::Exact,
        session_id: Some(format!("{}-session", source.as_str())),
        turn_id: Some(format!("{}-turn", source.as_str())),
        turn_boundary_id: Some(format!("{}-turn", source.as_str())),
        source_path: Some(format!("{}:fixture", source.as_str())),
        line_no: None,
        byte_offset: None,
        input_tokens: total_tokens,
        output_tokens: 0,
        cached_input_tokens: 0,
        cache_creation_input_tokens: 0,
        reasoning_output_tokens: 0,
        total_tokens,
        cumulative_total_tokens: Some(total_tokens),
        model: Some("fixture-model".to_string()),
        cwd: Some("/tmp/project".to_string()),
        observed_at,
        token_payload_hash: format!("hash-{}-{record_id}", source.as_str()),
    }
}

#[test]
fn scheduler_ingests_normalized_observations_with_tracking_window() {
    let dir = tempdir().unwrap();
    let store = UsageStore::open(&dir.path().join("token-fire.sqlite")).unwrap();
    let started_at = Utc.with_ymd_and_hms(2026, 7, 5, 9, 0, 0).unwrap();
    store.open_tracking_window(started_at).unwrap();
    let scheduler = IngestScheduler::new(store);

    let report = scheduler
        .ingest_observations_for_source(
            TokenSourceKind::Claude,
            vec![normalized_source_observation(
                TokenSourceKind::Claude,
                "claude-record-1",
                started_at + chrono::Duration::minutes(1),
                42,
            )],
        )
        .unwrap();

    assert_eq!(report.inserted, 1);
    assert_eq!(report.duplicates, 0);
    assert_eq!(report.skipped_outside_tracking, 0);
    assert_eq!(report.last_processed_offset, -1);
}

#[test]
fn scheduler_counts_duplicate_normalized_observations() {
    let dir = tempdir().unwrap();
    let store = UsageStore::open(&dir.path().join("token-fire.sqlite")).unwrap();
    let started_at = Utc.with_ymd_and_hms(2026, 7, 5, 9, 0, 0).unwrap();
    store.open_tracking_window(started_at).unwrap();
    let scheduler = IngestScheduler::new(store);
    let observation = normalized_source_observation(
        TokenSourceKind::Cursor,
        "cursor-record-1",
        started_at + chrono::Duration::minutes(1),
        24,
    );

    let first = scheduler
        .ingest_observations_for_source(TokenSourceKind::Cursor, vec![observation.clone()])
        .unwrap();
    let second = scheduler
        .ingest_observations_for_source(TokenSourceKind::Cursor, vec![observation])
        .unwrap();

    assert_eq!(first.inserted, 1);
    assert_eq!(second.inserted, 0);
    assert_eq!(second.duplicates, 1);
}

#[test]
fn scheduler_rejects_source_or_adapter_version_mismatch() {
    let dir = tempdir().unwrap();
    let store = UsageStore::open(&dir.path().join("token-fire.sqlite")).unwrap();
    store
        .open_tracking_window(Utc.with_ymd_and_hms(2026, 7, 5, 9, 0, 0).unwrap())
        .unwrap();
    let scheduler = IngestScheduler::new(store);
    let mut observation = normalized_source_observation(
        TokenSourceKind::Claude,
        "claude-record-1",
        Utc.with_ymd_and_hms(2026, 7, 5, 9, 1, 0).unwrap(),
        42,
    );

    observation.adapter_version = "cursor-storage-estimate-v1".to_string();

    let err = scheduler
        .ingest_observations_for_source(TokenSourceKind::Claude, vec![observation])
        .unwrap_err();

    assert!(err.to_string().contains("adapter version"));
}

#[test]
fn ingests_only_active_window_rows_and_respects_baseline_offsets() {
    let dir = tempdir().unwrap();
    let db = UsageStore::open(&dir.path().join("token-fire.sqlite")).unwrap();
    let transcript = dir.path().join("rollout-019-session-a.jsonl");
    let content = include_str!("fixtures/traex-session.jsonl");
    fs::write(&transcript, content).unwrap();

    db.open_tracking_window(Utc.with_ymd_and_hms(2026, 6, 20, 3, 1, 2).unwrap())
        .unwrap();
    db.set_file_baseline(&transcript.to_string_lossy(), 0)
        .unwrap();

    let scheduler = IngestScheduler::new(db);
    let report = scheduler.ingest_path(&transcript).unwrap();

    assert_eq!(report.inserted, 2);
    assert_eq!(report.duplicates, 0);
    assert!(report.last_processed_offset > 0);
}

#[test]
fn scheduler_binds_inserted_observations_to_tracking_window() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("token-fire.sqlite");
    let db = UsageStore::open(&db_path).unwrap();
    let transcript = dir.path().join("rollout-019-session-a.jsonl");
    fs::write(&transcript, include_str!("fixtures/traex-session.jsonl")).unwrap();

    let window_id = db
        .open_tracking_window(Utc.with_ymd_and_hms(2026, 6, 20, 3, 0, 0).unwrap())
        .unwrap();
    let scheduler = IngestScheduler::new(db);

    let report = scheduler.ingest_path(&transcript).unwrap();

    assert_eq!(report.inserted, 2);
    let metadata = UsageStore::open(&db_path)
        .unwrap()
        .recent_observation_metadata(10)
        .unwrap();
    assert_eq!(metadata.len(), 2);
    assert!(metadata
        .iter()
        .all(|row| row["tracking_window_id"] == window_id));
}

#[test]
fn scheduler_can_ingest_codex_source_context() {
    let dir = tempdir().unwrap();
    let transcript = dir.path().join("rollout-019-codex-session.jsonl");
    fs::write(&transcript, include_str!("fixtures/codex-session.jsonl")).unwrap();
    let store = UsageStore::open(&dir.path().join("token-fire.sqlite")).unwrap();
    store
        .open_tracking_window(Utc.with_ymd_and_hms(2026, 6, 26, 2, 59, 0).unwrap())
        .unwrap();
    let scheduler = IngestScheduler::new(store);

    let report = scheduler
        .ingest_path_for_source(SourceContext::codex(), &transcript)
        .unwrap();

    assert_eq!(report.inserted, 2);
}

#[test]
fn pre_existing_codex_file_can_be_baselined_without_backfill() {
    let dir = tempdir().unwrap();
    let transcript = dir.path().join("rollout-019-codex-session.jsonl");
    fs::write(&transcript, include_str!("fixtures/codex-session.jsonl")).unwrap();
    let store = UsageStore::open(&dir.path().join("token-fire.sqlite")).unwrap();
    store
        .set_file_baseline(
            &transcript.to_string_lossy(),
            fs::read_to_string(&transcript).unwrap().len() as i64,
        )
        .unwrap();
    store
        .open_tracking_window(Utc.with_ymd_and_hms(2026, 6, 26, 2, 59, 0).unwrap())
        .unwrap();
    let scheduler = IngestScheduler::new(store);

    let report = scheduler
        .ingest_path_for_source(SourceContext::codex(), &transcript)
        .unwrap();

    assert_eq!(report.inserted, 0);
    assert_eq!(report.skipped_outside_tracking, 2);
}

#[test]
fn repeated_ingest_after_baseline_is_skip_safe() {
    let dir = tempdir().unwrap();
    let db = UsageStore::open(&dir.path().join("token-fire.sqlite")).unwrap();
    let transcript = dir.path().join("rollout-019-session-a.jsonl");
    fs::write(&transcript, include_str!("fixtures/traex-session.jsonl")).unwrap();

    db.open_tracking_window(Utc.with_ymd_and_hms(2026, 6, 20, 3, 0, 0).unwrap())
        .unwrap();
    let scheduler = IngestScheduler::new(db);

    assert_eq!(scheduler.ingest_path(&transcript).unwrap().inserted, 2);
    let second = scheduler.ingest_path(&transcript).unwrap();
    assert_eq!(second.inserted, 0);
    assert_eq!(second.duplicates, 0);
    assert_eq!(second.skipped_outside_tracking, 2);
}

#[test]
fn default_baseline_ingests_token_row_at_byte_offset_zero() {
    let dir = tempdir().unwrap();
    let db = UsageStore::open(&dir.path().join("token-fire.sqlite")).unwrap();
    let transcript = dir.path().join("rollout-offset-zero.jsonl");
    fs::write(
        &transcript,
        "{\"type\":\"event_msg\",\"timestamp\":\"2026-06-20T03:01:02.000Z\",\"payload\":{\"type\":\"token_count\",\"info\":{\"last_token_usage\":{\"input_tokens\":1,\"output_tokens\":2,\"cached_input_tokens\":0,\"cache_creation_input_tokens\":0,\"reasoning_output_tokens\":0,\"total_tokens\":3},\"total_token_usage\":{\"input_tokens\":1,\"output_tokens\":2,\"cached_input_tokens\":0,\"cache_creation_input_tokens\":0,\"reasoning_output_tokens\":0,\"total_tokens\":3}}}}\n",
    )
    .unwrap();

    db.open_tracking_window(Utc.with_ymd_and_hms(2026, 6, 20, 3, 0, 0).unwrap())
        .unwrap();
    let scheduler = IngestScheduler::new(db);

    let report = scheduler.ingest_path(&transcript).unwrap();

    assert_eq!(report.inserted, 1);
    assert_eq!(report.skipped_outside_tracking, 0);
}

#[test]
fn final_token_jsonl_row_without_trailing_newline_is_ingested() {
    let dir = tempdir().unwrap();
    let db = UsageStore::open(&dir.path().join("token-fire.sqlite")).unwrap();
    let transcript = dir.path().join("rollout-final-no-newline.jsonl");
    let final_line = "{\"type\":\"event_msg\",\"timestamp\":\"2026-06-20T03:01:02.000Z\",\"payload\":{\"type\":\"token_count\",\"info\":{\"last_token_usage\":{\"input_tokens\":1,\"output_tokens\":2,\"cached_input_tokens\":0,\"cache_creation_input_tokens\":0,\"reasoning_output_tokens\":0,\"total_tokens\":3},\"total_token_usage\":{\"input_tokens\":1,\"output_tokens\":2,\"cached_input_tokens\":0,\"cache_creation_input_tokens\":0,\"reasoning_output_tokens\":0,\"total_tokens\":3}}}}";
    fs::write(&transcript, final_line).unwrap();

    db.open_tracking_window(Utc.with_ymd_and_hms(2026, 6, 20, 3, 0, 0).unwrap())
        .unwrap();
    let scheduler = IngestScheduler::new(db);

    let report = scheduler.ingest_path(&transcript).unwrap();

    assert_eq!(report.inserted, 1);
    assert_eq!(report.skipped_outside_tracking, 0);
    assert_eq!(report.last_processed_offset, 0);
}

#[test]
fn trailing_partial_jsonl_does_not_advance_baseline_past_complete_line() {
    let dir = tempdir().unwrap();
    let db = UsageStore::open(&dir.path().join("token-fire.sqlite")).unwrap();
    let transcript = dir.path().join("rollout-partial.jsonl");
    let complete_line = "{\"type\":\"event_msg\",\"timestamp\":\"2026-06-20T03:01:02.000Z\",\"payload\":{\"type\":\"token_count\",\"info\":{\"last_token_usage\":{\"input_tokens\":1,\"output_tokens\":2,\"cached_input_tokens\":0,\"cache_creation_input_tokens\":0,\"reasoning_output_tokens\":0,\"total_tokens\":3},\"total_token_usage\":{\"input_tokens\":1,\"output_tokens\":2,\"cached_input_tokens\":0,\"cache_creation_input_tokens\":0,\"reasoning_output_tokens\":0,\"total_tokens\":3}}}}\n";
    let partial_line = "{\"type\":\"event_msg\",\"timestamp\":\"2026-06-20T03:01:03.000Z\",\"payload\":{\"type\":\"token_count\",\"info\":{\"last_token_usage\":{\"input_tokens\":4,\"output_tokens\":5";
    let completed_line = "{\"type\":\"event_msg\",\"timestamp\":\"2026-06-20T03:01:03.000Z\",\"payload\":{\"type\":\"token_count\",\"info\":{\"last_token_usage\":{\"input_tokens\":4,\"output_tokens\":5,\"cached_input_tokens\":0,\"cache_creation_input_tokens\":0,\"reasoning_output_tokens\":0,\"total_tokens\":9},\"total_token_usage\":{\"input_tokens\":5,\"output_tokens\":7,\"cached_input_tokens\":0,\"cache_creation_input_tokens\":0,\"reasoning_output_tokens\":0,\"total_tokens\":12}}}}\n";
    fs::write(&transcript, format!("{complete_line}{partial_line}")).unwrap();

    db.open_tracking_window(Utc.with_ymd_and_hms(2026, 6, 20, 3, 0, 0).unwrap())
        .unwrap();
    let scheduler = IngestScheduler::new(db);

    let first = scheduler.ingest_path(&transcript).unwrap();
    assert_eq!(first.inserted, 1);
    assert_eq!(first.last_processed_offset, 0);

    fs::write(&transcript, format!("{complete_line}{completed_line}")).unwrap();
    let second = scheduler.ingest_path(&transcript).unwrap();

    assert_eq!(second.inserted, 1);
    assert_eq!(second.skipped_outside_tracking, 1);
    assert_eq!(second.last_processed_offset, complete_line.len() as i64);
}
