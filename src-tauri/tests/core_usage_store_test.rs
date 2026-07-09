use std::path::Path;

use chrono::{Datelike, Local, TimeZone, Utc};
use rusqlite::{params, Connection};
use tempfile::tempdir;
use token_fire::core::observation::{NormalizedObservation, SourceRecordIdConfidence};
use token_fire::core::pricing::{PricingStatus, DEFAULT_AVERAGE_CNY_PER_1M_TOKENS};
use token_fire::core::profile::ProfilePeriod;
use token_fire::core::usage_series::{WIDGET_USAGE_BUCKET_MINUTES, WIDGET_USAGE_WINDOW_MINUTES};
use token_fire::core::usage_store::{
    InsertOutcome, RetentionPolicy, RetentionSkipReason, UsageStore,
};

fn observation(
    source_record_id: &str,
    total_tokens: i64,
    observed_at: chrono::DateTime<Utc>,
) -> NormalizedObservation {
    NormalizedObservation {
        source: "traex".to_string(),
        adapter_version: "traex-jsonl-v1".to_string(),
        source_record_id: source_record_id.to_string(),
        source_record_id_confidence: SourceRecordIdConfidence::Exact,
        session_id: Some("session-a".to_string()),
        turn_id: Some("turn-a".to_string()),
        turn_boundary_id: Some("turn-a".to_string()),
        source_path: Some("/tmp/rollout-session-a.jsonl".to_string()),
        line_no: Some(1),
        byte_offset: Some(10),
        input_tokens: total_tokens,
        output_tokens: 0,
        cached_input_tokens: 0,
        cache_creation_input_tokens: 0,
        reasoning_output_tokens: 0,
        total_tokens,
        cumulative_total_tokens: Some(total_tokens),
        model: Some("model-a".to_string()),
        cwd: Some("~/project".to_string()),
        observed_at,
        token_payload_hash: format!("hash-{source_record_id}"),
    }
}

fn insert_tracked(
    store: &UsageStore,
    row: &NormalizedObservation,
) -> anyhow::Result<InsertOutcome> {
    let window_id = store.open_tracking_window(row.observed_at - chrono::Duration::minutes(1))?;
    store.insert_observation_for_tracking_window(row, window_id)
}

fn insert_tracked_without_validation(
    db_path: &Path,
    row: &NormalizedObservation,
) -> anyhow::Result<()> {
    let conn = Connection::open(db_path)?;
    conn.execute(
        "insert into tracking_windows (started_at) values (?1)",
        params![(row.observed_at - chrono::Duration::minutes(1)).to_rfc3339()],
    )?;
    let window_id = conn.last_insert_rowid();
    let confidence = match row.source_record_id_confidence {
        SourceRecordIdConfidence::Exact => "exact",
        SourceRecordIdConfidence::Fallback => "fallback",
    };
    conn.execute(
        r#"
        insert into token_observations (
          tracking_window_id, source, adapter_version, source_record_id, source_record_id_confidence,
          session_id, turn_id, turn_boundary_id, source_path, line_no, byte_offset,
          input_tokens, output_tokens, cached_input_tokens, cache_creation_input_tokens,
          reasoning_output_tokens, total_tokens, cumulative_total_tokens, model, cwd,
          observed_at, token_payload_hash, dedupe_key
        ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23)
        "#,
        params![
            window_id,
            row.source,
            row.adapter_version,
            row.source_record_id,
            confidence,
            row.session_id,
            row.turn_id,
            row.turn_boundary_id,
            row.source_path,
            row.line_no,
            row.byte_offset,
            row.input_tokens,
            row.output_tokens,
            row.cached_input_tokens,
            row.cache_creation_input_tokens,
            row.reasoning_output_tokens,
            row.total_tokens,
            row.cumulative_total_tokens,
            row.model,
            row.cwd,
            row.observed_at.to_rfc3339(),
            row.token_payload_hash,
            format!("raw-profile-test:{}", row.source_record_id),
        ],
    )?;
    Ok(())
}

fn cost_observation(
    source_record_id: &str,
    model: Option<&str>,
    input_tokens: i64,
    output_tokens: i64,
    cached_input_tokens: i64,
    cache_creation_input_tokens: i64,
    reasoning_output_tokens: i64,
    total_tokens: i64,
    observed_at: chrono::DateTime<Utc>,
) -> NormalizedObservation {
    let mut row = observation(source_record_id, total_tokens, observed_at);
    row.model = model.map(str::to_string);
    row.input_tokens = input_tokens;
    row.output_tokens = output_tokens;
    row.cached_input_tokens = cached_input_tokens;
    row.cache_creation_input_tokens = cache_creation_input_tokens;
    row.reasoning_output_tokens = reasoning_output_tokens;
    row.total_tokens = total_tokens;
    row.cumulative_total_tokens = Some(total_tokens);
    row
}

fn assert_cost_close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 0.000_001,
        "expected {expected}, got {actual}"
    );
}

fn observation_count(db_path: &Path) -> i64 {
    Connection::open(db_path)
        .unwrap()
        .query_row("select count(*) from token_observations", [], |row| {
            row.get(0)
        })
        .unwrap()
}

fn table_exists(db_path: &Path, table: &str) -> bool {
    Connection::open(db_path)
        .unwrap()
        .query_row(
            "select exists(select 1 from sqlite_master where type = 'table' and name = ?1)",
            [table],
            |row| row.get::<_, i64>(0),
        )
        .unwrap()
        == 1
}

fn state_value(db_path: &Path, key: &str) -> Option<String> {
    Connection::open(db_path)
        .unwrap()
        .query_row(
            "select value from retention_state where key = ?1",
            [key],
            |row| row.get(0),
        )
        .ok()
}

#[test]
fn opening_legacy_store_adds_tracking_window_id_before_indexing_it() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("token-fire.sqlite");
    let conn = Connection::open(&db_path).unwrap();
    conn.execute_batch(
        r#"
        create table token_observations (
          id integer primary key autoincrement,
          source text not null,
          adapter_version text not null,
          source_record_id text not null,
          source_record_id_confidence text not null,
          session_id text,
          turn_id text,
          turn_boundary_id text,
          source_path text,
          line_no integer,
          byte_offset integer,
          input_tokens integer not null,
          output_tokens integer not null,
          cached_input_tokens integer not null,
          cache_creation_input_tokens integer not null,
          reasoning_output_tokens integer not null,
          total_tokens integer not null,
          cumulative_total_tokens integer,
          model text,
          cwd text,
          observed_at text not null,
          created_at text not null default (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
          token_payload_hash text not null,
          dedupe_key text not null
        );
        insert into token_observations (
          source,
          adapter_version,
          source_record_id,
          source_record_id_confidence,
          input_tokens,
          output_tokens,
          cached_input_tokens,
          cache_creation_input_tokens,
          reasoning_output_tokens,
          total_tokens,
          observed_at,
          token_payload_hash,
          dedupe_key
        ) values (
          'traex',
          'traex-jsonl-v1',
          'legacy-row',
          'exact',
          10,
          20,
          0,
          0,
          0,
          30,
          '2026-06-20T02:00:00Z',
          'legacy-hash',
          'legacy-dedupe'
        );
        "#,
    )
    .unwrap();
    drop(conn);

    UsageStore::open(&db_path).unwrap();

    let migrated = Connection::open(&db_path).unwrap();
    let has_tracking_window_id: bool = migrated
        .prepare("pragma table_info(token_observations)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .any(|column| column.unwrap() == "tracking_window_id");
    let index_count: i64 = migrated
        .query_row(
            "select count(*) from sqlite_master where type = 'index' and name = 'idx_token_observations_tracking_window_id'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let row_count: i64 = migrated
        .query_row("select count(*) from token_observations", [], |row| {
            row.get(0)
        })
        .unwrap();

    assert!(has_tracking_window_id);
    assert_eq!(index_count, 1);
    assert_eq!(row_count, 1);
}

#[test]
fn duplicate_dedupe_key_does_not_change_totals() {
    let dir = tempdir().unwrap();
    let store = UsageStore::open(&dir.path().join("token-fire.sqlite")).unwrap();
    let observed_at = Utc.with_ymd_and_hms(2026, 6, 20, 2, 0, 0).unwrap();
    let row = observation("session-a:10", 100, observed_at);

    assert_eq!(
        insert_tracked(&store, &row).unwrap(),
        InsertOutcome::Inserted
    );
    assert_eq!(
        insert_tracked(&store, &row).unwrap(),
        InsertOutcome::Duplicate
    );

    let today = Local.with_ymd_and_hms(2026, 6, 20, 12, 0, 0).unwrap();
    assert_eq!(store.today_total(today).unwrap(), 100);
}

#[test]
fn canonical_transcript_duplicate_across_sources_does_not_change_totals() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("token-fire.sqlite");
    let store = UsageStore::open(&db_path).unwrap();
    let observed_at = Utc.with_ymd_and_hms(2026, 6, 26, 3, 0, 0).unwrap();
    let window_id = store
        .open_tracking_window(observed_at - chrono::Duration::minutes(1))
        .unwrap();
    let mut traex = observation("session-cross-source:42", 25, observed_at);
    traex.source = "traex".to_string();
    traex.adapter_version = "traex-jsonl-v1".to_string();
    traex.session_id = Some("session-cross-source".to_string());
    traex.byte_offset = Some(42);
    traex.token_payload_hash = "same-token-payload".to_string();
    let mut codex = traex.clone();
    codex.source = "codex".to_string();
    codex.adapter_version = "codex-jsonl-v1".to_string();

    assert_eq!(
        store
            .insert_observation_for_tracking_window(&traex, window_id)
            .unwrap(),
        InsertOutcome::Inserted
    );
    assert_eq!(
        store
            .insert_observation_for_tracking_window(&codex, window_id)
            .unwrap(),
        InsertOutcome::Duplicate
    );
    assert_eq!(
        store
            .today_total(observed_at.with_timezone(&Local))
            .unwrap(),
        25
    );
}

#[test]
fn today_total_uses_observed_at_not_created_at() {
    let dir = tempdir().unwrap();
    let store = UsageStore::open(&dir.path().join("token-fire.sqlite")).unwrap();
    let yesterday = Utc.with_ymd_and_hms(2026, 6, 19, 2, 0, 0).unwrap();

    store
        .insert_observation_for_tracking_window(
            &observation("session-a:20", 200, yesterday),
            store
                .open_tracking_window(yesterday - chrono::Duration::minutes(1))
                .unwrap(),
        )
        .unwrap();

    let today = Local.with_ymd_and_hms(2026, 6, 20, 12, 0, 0).unwrap();
    assert_eq!(store.today_total(today).unwrap(), 0);
}

#[test]
fn latest_turn_delta_sums_latest_boundary_group() {
    let dir = tempdir().unwrap();
    let store = UsageStore::open(&dir.path().join("token-fire.sqlite")).unwrap();
    let first = Utc.with_ymd_and_hms(2026, 6, 20, 2, 0, 0).unwrap();
    let second = Utc.with_ymd_and_hms(2026, 6, 20, 2, 1, 0).unwrap();

    store
        .insert_observation_for_tracking_window(
            &observation("session-a:10", 100, first),
            store
                .open_tracking_window(first - chrono::Duration::minutes(1))
                .unwrap(),
        )
        .unwrap();
    store
        .insert_observation_for_tracking_window(
            &observation("session-a:20", 50, second),
            store
                .open_tracking_window(second - chrono::Duration::minutes(1))
                .unwrap(),
        )
        .unwrap();

    assert_eq!(store.latest_turn_delta().unwrap(), 150);
}

#[test]
fn untracked_observations_do_not_contribute_to_ui_totals() {
    let dir = tempdir().unwrap();
    let store = UsageStore::open(&dir.path().join("token-fire.sqlite")).unwrap();
    let observed_at = Utc.with_ymd_and_hms(2026, 6, 20, 2, 0, 0).unwrap();

    store
        .insert_untracked_observation_for_test(&observation("session-a:10", 100, observed_at))
        .unwrap();

    let today = Local.with_ymd_and_hms(2026, 6, 20, 12, 0, 0).unwrap();
    assert_eq!(store.today_total(today).unwrap(), 0);
    assert_eq!(store.latest_turn_delta().unwrap(), 0);
}

#[test]
fn tracked_insert_rejects_missing_tracking_window() {
    let dir = tempdir().unwrap();
    let store = UsageStore::open(&dir.path().join("token-fire.sqlite")).unwrap();
    let observed_at = Utc.with_ymd_and_hms(2026, 6, 20, 2, 0, 0).unwrap();

    let error = store
        .insert_observation_for_tracking_window(
            &observation("session-a:missing-window", 100, observed_at),
            42,
        )
        .unwrap_err();

    assert!(error.to_string().contains("tracking window not found"));
}

#[test]
fn tracked_insert_rejects_observation_outside_tracking_window() {
    let dir = tempdir().unwrap();
    let store = UsageStore::open(&dir.path().join("token-fire.sqlite")).unwrap();
    let started = Utc.with_ymd_and_hms(2026, 6, 20, 2, 0, 0).unwrap();
    let ended = Utc.with_ymd_and_hms(2026, 6, 20, 3, 0, 0).unwrap();
    let window_id = store.open_tracking_window(started).unwrap();
    store.close_tracking_window(ended).unwrap();

    let error = store
        .insert_observation_for_tracking_window(
            &observation(
                "session-a:outside-window",
                100,
                Utc.with_ymd_and_hms(2026, 6, 20, 3, 0, 0).unwrap(),
            ),
            window_id,
        )
        .unwrap_err();

    assert!(error
        .to_string()
        .contains("observation outside tracking window"));
}

#[test]
fn tracking_windows_and_baselines_round_trip() {
    let dir = tempdir().unwrap();
    let store = UsageStore::open(&dir.path().join("token-fire.sqlite")).unwrap();
    let started = Utc.with_ymd_and_hms(2026, 6, 20, 2, 0, 0).unwrap();
    let ended = Utc.with_ymd_and_hms(2026, 6, 20, 3, 0, 0).unwrap();

    let window_id = store.open_tracking_window(started).unwrap();
    assert_eq!(store.active_tracking_windows().unwrap().len(), 1);
    store.close_tracking_window(ended).unwrap();
    assert_eq!(store.active_tracking_windows().unwrap().len(), 0);

    store.set_file_baseline("/tmp/rollout.jsonl", 123).unwrap();
    assert_eq!(
        store.file_baseline("/tmp/rollout.jsonl").unwrap(),
        Some(123)
    );
    assert!(window_id > 0);
}

#[test]
fn retention_migration_creates_state_table() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("token-fire.sqlite");

    UsageStore::open(&db_path).unwrap();

    assert!(table_exists(&db_path, "retention_state"));
}

#[test]
fn retention_deletes_only_observations_older_than_cutoff() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("token-fire.sqlite");
    let mut store = UsageStore::open(&db_path).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 6, 26, 12, 0, 0).unwrap();
    let cutoff = now - chrono::Duration::days(365);
    let older = cutoff - chrono::Duration::seconds(1);
    let boundary = cutoff;
    let newer = cutoff + chrono::Duration::seconds(1);

    insert_tracked(&store, &observation("old", 10, older)).unwrap();
    insert_tracked(&store, &observation("boundary", 20, boundary)).unwrap();
    insert_tracked(&store, &observation("new", 30, newer)).unwrap();
    let closed_window_id = store
        .open_tracking_window(older - chrono::Duration::minutes(1))
        .unwrap();
    store
        .close_tracking_window(older + chrono::Duration::minutes(1))
        .unwrap();
    store.set_file_baseline("/tmp/rollout.jsonl", 512).unwrap();

    let outcome = store
        .run_retention_if_due(now, RetentionPolicy::default())
        .unwrap();

    assert!(outcome.ran);
    assert_eq!(outcome.deleted_observations, 1);
    assert_eq!(outcome.skipped_reason, None);
    assert_eq!(observation_count(&db_path), 2);
    assert_eq!(
        store.file_baseline("/tmp/rollout.jsonl").unwrap(),
        Some(512)
    );
    assert!(store
        .tracking_windows_for_ingest()
        .unwrap()
        .iter()
        .any(|window| window.id == closed_window_id));
}

#[test]
fn retention_skips_after_recent_success() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("token-fire.sqlite");
    let mut store = UsageStore::open(&db_path).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 6, 26, 12, 0, 0).unwrap();

    let first = store
        .run_retention_if_due(now, RetentionPolicy::default())
        .unwrap();
    let second = store
        .run_retention_if_due(now + chrono::Duration::hours(1), RetentionPolicy::default())
        .unwrap();

    assert!(first.ran);
    assert!(!second.ran);
    assert_eq!(
        second.skipped_reason,
        Some(RetentionSkipReason::RecentlySucceeded)
    );
    assert_eq!(
        state_value(&db_path, "last_success_at"),
        Some(now.to_rfc3339())
    );
}

#[test]
fn retention_policy_is_internally_configurable() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("token-fire.sqlite");
    let mut store = UsageStore::open(&db_path).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 6, 26, 12, 0, 0).unwrap();
    let old_for_seven_days = now - chrono::Duration::days(8);
    let kept_for_seven_days = now - chrono::Duration::days(6);
    let policy = RetentionPolicy {
        observation_retention_days: 7,
        min_interval_hours: 24,
    };

    insert_tracked(
        &store,
        &observation("old-for-seven-days", 10, old_for_seven_days),
    )
    .unwrap();
    insert_tracked(
        &store,
        &observation("kept-for-seven-days", 20, kept_for_seven_days),
    )
    .unwrap();

    let outcome = store.run_retention_if_due(now, policy).unwrap();

    assert_eq!(outcome.deleted_observations, 1);
    assert_eq!(observation_count(&db_path), 1);
}

#[test]
fn failed_retention_transaction_does_not_advance_success_state() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("token-fire.sqlite");
    let store = UsageStore::open(&db_path).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 6, 26, 12, 0, 0).unwrap();
    let old = now - chrono::Duration::days(366);

    insert_tracked(&store, &observation("old-before-trigger", 10, old)).unwrap();
    drop(store);

    Connection::open(&db_path)
        .unwrap()
        .execute_batch(
            r#"
            create trigger fail_retention_delete
            before delete on token_observations
            begin
              select raise(abort, 'retention delete blocked');
            end;
            "#,
        )
        .unwrap();

    let mut store = UsageStore::open(&db_path).unwrap();
    let error = store
        .run_retention_if_due(now, RetentionPolicy::default())
        .unwrap_err();

    assert!(error.to_string().contains("retention delete blocked"));
    assert_eq!(state_value(&db_path, "last_success_at"), None);
    assert_eq!(observation_count(&db_path), 1);
}

#[test]
fn retention_failure_diagnostics_do_not_change_success_state() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("token-fire.sqlite");
    let store = UsageStore::open(&db_path).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 6, 26, 12, 0, 0).unwrap();

    store
        .record_retention_failure(now, "sqlite_retention_failed")
        .unwrap();
    let diagnostics = store
        .retention_diagnostics(RetentionPolicy::default())
        .unwrap();

    assert_eq!(diagnostics.last_success_at, None);
    assert_eq!(diagnostics.last_failure_at, Some(now.to_rfc3339()));
    assert_eq!(
        diagnostics.last_error_kind,
        Some("sqlite_retention_failed".to_string())
    );
}

#[test]
fn migration_creates_tracked_observed_at_partial_index() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("token-fire.sqlite");

    UsageStore::open(&db_path).unwrap();

    let conn = Connection::open(&db_path).unwrap();
    let (sql, partial): (String, i64) = conn
        .query_row(
            "select sql, partial from sqlite_master join pragma_index_list('token_observations') on sqlite_master.name = pragma_index_list.name where sqlite_master.type = 'index' and sqlite_master.name = 'idx_token_observations_tracked_observed_at'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();

    assert_eq!(partial, 1);
    assert!(sql.contains("on token_observations(observed_at)"));
    assert!(sql.contains("where tracking_window_id is not null"));
}

#[test]
fn usage_series_query_uses_observed_at_range_index() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("token-fire.sqlite");
    UsageStore::open(&db_path).unwrap();
    let conn = Connection::open(&db_path).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 6, 28, 12, 0, 0).unwrap();
    let start = now - chrono::Duration::minutes(WIDGET_USAGE_WINDOW_MINUTES);

    let plan: String = conn
        .prepare(
            r#"
            explain query plan
            select observed_at, total_tokens
            from token_observations
            where tracking_window_id is not null
              and observed_at >= ?1
              and observed_at < ?2
            order by observed_at asc
            "#,
        )
        .unwrap()
        .query_map(params![start.to_rfc3339(), now.to_rfc3339()], |row| {
            row.get::<_, String>(3)
        })
        .unwrap()
        .map(|row| row.unwrap())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(plan.contains("idx_token_observations_tracked_observed_at"));
    assert!(plan.contains("observed_at>?"));
}

#[test]
fn state_revision_and_last_observed_at_use_only_tracked_rows() {
    let dir = tempdir().unwrap();
    let store = UsageStore::open(&dir.path().join("token-fire.sqlite")).unwrap();
    let first = Utc.with_ymd_and_hms(2026, 6, 28, 2, 0, 0).unwrap();
    let second = Utc.with_ymd_and_hms(2026, 6, 28, 2, 30, 0).unwrap();
    let untracked = Utc.with_ymd_and_hms(2026, 6, 28, 2, 45, 0).unwrap();

    let first_row = observation("revision-first", 10, first);
    let second_row = observation("revision-second", 20, second);
    let untracked_row = observation("revision-untracked", 999, untracked);

    insert_tracked(&store, &first_row).unwrap();
    insert_tracked(&store, &second_row).unwrap();
    store
        .insert_untracked_observation_for_test(&untracked_row)
        .unwrap();

    assert_eq!(store.state_revision().unwrap(), 2);
    assert_eq!(store.last_observed_at().unwrap(), Some(second));
}

#[test]
fn usage_series_returns_12_tracked_30_minute_buckets_with_average() {
    let dir = tempdir().unwrap();
    let store = UsageStore::open(&dir.path().join("token-fire.sqlite")).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 6, 28, 12, 0, 0).unwrap();
    let start = now - chrono::Duration::minutes(WIDGET_USAGE_WINDOW_MINUTES);
    let previous_day_end = now - chrono::Duration::hours(24);
    let previous_day_start =
        previous_day_end - chrono::Duration::minutes(WIDGET_USAGE_WINDOW_MINUTES);

    insert_tracked(&store, &observation("bucket-0", 120, start)).unwrap();
    insert_tracked(
        &store,
        &observation("bucket-1-a", 30, start + chrono::Duration::minutes(30)),
    )
    .unwrap();
    insert_tracked(
        &store,
        &observation("bucket-1-b", 40, start + chrono::Duration::minutes(45)),
    )
    .unwrap();
    insert_tracked(
        &store,
        &observation("latest", 200, now - chrono::Duration::minutes(2)),
    )
    .unwrap();
    insert_tracked(
        &store,
        &observation(
            "previous-day-0",
            900,
            previous_day_start + chrono::Duration::minutes(5),
        ),
    )
    .unwrap();
    insert_tracked(
        &store,
        &observation(
            "previous-day-3",
            300,
            previous_day_start + chrono::Duration::minutes(95),
        ),
    )
    .unwrap();
    store
        .insert_untracked_observation_for_test(&observation(
            "previous-day-untracked",
            999,
            previous_day_start + chrono::Duration::minutes(125),
        ))
        .unwrap();
    store
        .insert_untracked_observation_for_test(&observation(
            "untracked",
            999,
            now - chrono::Duration::minutes(1),
        ))
        .unwrap();
    insert_tracked(
        &store,
        &observation("too-old", 777, start - chrono::Duration::seconds(1)),
    )
    .unwrap();

    let series = store.usage_series_at(now).unwrap();

    assert_eq!(series.window_minutes, 360);
    assert_eq!(series.bucket_minutes, WIDGET_USAGE_BUCKET_MINUTES);
    assert_eq!(series.buckets.len(), 12);
    assert_eq!(series.previous_day_buckets.len(), 12);
    assert_eq!(series.buckets[0].start_at, start);
    assert_eq!(series.previous_day_buckets[0].start_at, previous_day_start);
    assert_eq!(series.previous_day_buckets[11].end_at, previous_day_end);
    assert_eq!(series.buckets[0].total_tokens, 120);
    assert_eq!(series.buckets[1].total_tokens, 70);
    assert_eq!(series.buckets[10].total_tokens, 0);
    assert_eq!(series.buckets[11].total_tokens, 200);
    assert_eq!(series.previous_day_buckets[0].total_tokens, 900);
    assert_eq!(series.previous_day_buckets[1].total_tokens, 0);
    assert_eq!(series.previous_day_buckets[3].total_tokens, 300);
    assert_eq!(series.previous_day_buckets[4].total_tokens, 0);
    assert_eq!(series.latest_bucket_tokens, 200);
    assert!(series.latest_bucket_active);
    assert_eq!(series.average_tokens_per_bucket, 390.0 / 12.0);
}

#[test]
fn usage_series_latest_bucket_is_inactive_when_last_observation_is_stale() {
    let dir = tempdir().unwrap();
    let store = UsageStore::open(&dir.path().join("token-fire.sqlite")).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 6, 28, 12, 0, 0).unwrap();

    insert_tracked(
        &store,
        &observation("stale-latest", 50, now - chrono::Duration::minutes(10)),
    )
    .unwrap();

    let series = store.usage_series_at(now).unwrap();

    assert_eq!(series.latest_bucket_tokens, 50);
    assert!(!series.latest_bucket_active);
}

#[test]
fn usage_series_ignores_future_observations_for_latest_activity() {
    let dir = tempdir().unwrap();
    let store = UsageStore::open(&dir.path().join("token-fire.sqlite")).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 6, 28, 12, 0, 0).unwrap();

    insert_tracked(
        &store,
        &observation("future", 50, now + chrono::Duration::minutes(1)),
    )
    .unwrap();

    let series = store.usage_series_at(now).unwrap();

    assert_eq!(series.latest_bucket_tokens, 0);
    assert!(!series.latest_bucket_active);
}

#[test]
fn usage_series_latest_bucket_is_inactive_when_newest_observation_is_future() {
    let dir = tempdir().unwrap();
    let store = UsageStore::open(&dir.path().join("token-fire.sqlite")).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 6, 28, 12, 0, 0).unwrap();

    insert_tracked(
        &store,
        &observation("latest-valid", 20, now - chrono::Duration::minutes(2)),
    )
    .unwrap();
    insert_tracked(
        &store,
        &observation("future-newest", 50, now + chrono::Duration::minutes(1)),
    )
    .unwrap();

    let series = store.usage_series_at(now).unwrap();

    assert_eq!(series.latest_bucket_tokens, 20);
    assert!(!series.latest_bucket_active);
}

#[test]
fn previous_day_buckets_do_not_affect_current_latest_activity() {
    let dir = tempdir().unwrap();
    let store = UsageStore::open(&dir.path().join("token-fire.sqlite")).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 6, 28, 12, 0, 0).unwrap();
    let previous_day_latest = now - chrono::Duration::hours(24) - chrono::Duration::minutes(2);

    insert_tracked(
        &store,
        &observation("previous-day-latest", 500, previous_day_latest),
    )
    .unwrap();

    let series = store.usage_series_at(now).unwrap();

    assert_eq!(series.latest_bucket_tokens, 0);
    assert!(!series.latest_bucket_active);
    assert_eq!(series.buckets[11].total_tokens, 0);
    assert_eq!(series.previous_day_buckets[11].total_tokens, 500);
}

#[test]
fn cost_summary_groups_by_model_and_sums_rule_and_fallback_costs() {
    let dir = tempdir().unwrap();
    let store = UsageStore::open(&dir.path().join("token-fire.sqlite")).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 7, 2, 12, 0, 0).unwrap();
    let start = now - chrono::Duration::hours(1);
    insert_tracked(
        &store,
        &cost_observation(
            "gpt-a",
            Some("gpt-5.5"),
            1_000_000,
            500_000,
            250_000,
            100_000,
            50_000,
            1_900_000,
            now - chrono::Duration::minutes(30),
        ),
    )
    .unwrap();
    insert_tracked(
        &store,
        &cost_observation(
            "gpt-b",
            Some("gpt-5.5-20260701"),
            1_000_000,
            500_000,
            250_000,
            100_000,
            50_000,
            1_900_000,
            now - chrono::Duration::minutes(20),
        ),
    )
    .unwrap();
    insert_tracked(
        &store,
        &cost_observation(
            "unknown",
            Some("unknown-internal-model"),
            2_000_000,
            0,
            0,
            0,
            0,
            2_000_000,
            now - chrono::Duration::minutes(10),
        ),
    )
    .unwrap();

    let summary = store.cost_summary_between(start, now).unwrap();

    assert_eq!(summary.total_tokens, 5_800_000);
    assert_eq!(summary.pricing_status, PricingStatus::Mixed);
    assert_cost_close(summary.estimated_cost, 147.71875 * 2.0 + 6.0);
}

#[test]
fn cost_summary_excludes_untracked_and_out_of_window_observations() {
    let dir = tempdir().unwrap();
    let store = UsageStore::open(&dir.path().join("token-fire.sqlite")).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 7, 2, 12, 0, 0).unwrap();
    let start = now - chrono::Duration::hours(1);
    insert_tracked(
        &store,
        &cost_observation(
            "tracked",
            Some("unknown-model"),
            0,
            0,
            0,
            0,
            0,
            1_000_000,
            now - chrono::Duration::minutes(10),
        ),
    )
    .unwrap();
    store
        .insert_untracked_observation_for_test(&cost_observation(
            "untracked",
            Some("unknown-model"),
            0,
            0,
            0,
            0,
            0,
            1_000_000,
            now - chrono::Duration::minutes(9),
        ))
        .unwrap();
    insert_tracked(
        &store,
        &cost_observation(
            "too-old",
            Some("unknown-model"),
            0,
            0,
            0,
            0,
            0,
            1_000_000,
            start - chrono::Duration::seconds(1),
        ),
    )
    .unwrap();

    let summary = store.cost_summary_between(start, now).unwrap();

    assert_eq!(summary.total_tokens, 1_000_000);
    assert_eq!(summary.pricing_status, PricingStatus::Fallback);
    assert_cost_close(summary.estimated_cost, DEFAULT_AVERAGE_CNY_PER_1M_TOKENS);
}

#[test]
fn profile_summary_returns_fixed_365_day_heatmap_independent_from_period() {
    let dir = tempdir().unwrap();
    let store = UsageStore::open(&dir.path().join("token-fire.sqlite")).unwrap();
    let now_utc = Utc.with_ymd_and_hms(2026, 7, 4, 12, 0, 0).unwrap();
    let now_local = now_utc.with_timezone(&Local);
    let active_day = Utc.with_ymd_and_hms(2026, 7, 3, 8, 0, 0).unwrap();
    let row = cost_observation(
        "profile-fixed-year",
        Some("gpt-5.5"),
        1_000_000,
        500_000,
        0,
        0,
        0,
        1_500_000,
        active_day,
    );
    insert_tracked(&store, &row).unwrap();

    let one_day = store
        .profile_summary_at(
            token_fire::core::profile::ProfilePeriod::Today,
            now_utc,
            now_local,
        )
        .unwrap();
    let one_week = store
        .profile_summary_at(
            token_fire::core::profile::ProfilePeriod::ThisWeek,
            now_utc,
            now_local,
        )
        .unwrap();

    assert_eq!(one_day.year_profile.days.len(), 365);
    assert_eq!(one_week.year_profile.days.len(), 365);
    assert_eq!(one_day.year_profile.days, one_week.year_profile.days);
    assert_eq!(one_day.year_profile.active_days, 1);
    assert!(one_day.year_profile.peak_day.is_some());
}

#[test]
fn profile_summary_ranks_models_and_sources_by_total_tokens() {
    let dir = tempdir().unwrap();
    let store = UsageStore::open(&dir.path().join("token-fire.sqlite")).unwrap();
    let now_utc = Utc.with_ymd_and_hms(2026, 7, 4, 12, 0, 0).unwrap();
    let now_local = now_utc.with_timezone(&Local);

    let mut costlier_smaller_token_row = cost_observation(
        "profile-gpt54",
        Some("gpt-5.4"),
        1_000_000,
        1_000_000,
        0,
        0,
        0,
        2_000_000,
        now_utc - chrono::Duration::hours(2),
    );
    costlier_smaller_token_row.source = "codex".to_string();
    insert_tracked(&store, &costlier_smaller_token_row).unwrap();

    let mut larger_token_row = cost_observation(
        "profile-gpt55",
        Some("gpt-5.5"),
        1_000_000,
        2_500_000,
        0,
        0,
        0,
        3_500_000,
        now_utc - chrono::Duration::hours(1),
    );
    larger_token_row.source = "traex".to_string();
    insert_tracked(&store, &larger_token_row).unwrap();

    let summary = store
        .profile_summary_at(
            token_fire::core::profile::ProfilePeriod::ThisWeek,
            now_utc,
            now_local,
        )
        .unwrap();

    assert_eq!(summary.selected_period.model_breakdown[0].label, "gpt-5.5");
    assert_eq!(summary.selected_period.model_breakdown[1].label, "gpt-5.4");
    assert_eq!(summary.selected_period.source_breakdown[0].label, "TraeX");
    assert_eq!(summary.selected_period.source_breakdown[1].label, "Codex");
    assert!(
        summary.selected_period.model_breakdown[0].share
            > summary.selected_period.model_breakdown[1].share
    );
    assert_cost_close(
        summary.selected_period.model_breakdown[0].share,
        3_500_000.0 / 5_500_000.0,
    );
}

#[test]
fn profile_summary_groups_missing_model_and_source_under_unknown() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("token-fire.sqlite");
    let store = UsageStore::open(&db_path).unwrap();
    let now_utc = Utc.with_ymd_and_hms(2026, 7, 4, 12, 0, 0).unwrap();
    let now_local = now_utc.with_timezone(&Local);
    let mut row = cost_observation(
        "profile-unknown",
        None,
        0,
        0,
        0,
        0,
        0,
        1_000_000,
        now_utc - chrono::Duration::hours(3),
    );
    row.source = "".to_string();
    insert_tracked_without_validation(&db_path, &row).unwrap();

    let summary = store
        .profile_summary_at(
            token_fire::core::profile::ProfilePeriod::Today,
            now_utc,
            now_local,
        )
        .unwrap();

    assert_eq!(summary.selected_period.model_breakdown[0].label, "Unknown");
    assert_eq!(summary.selected_period.source_breakdown[0].label, "Unknown");
}

#[test]
fn profile_summary_reports_cost_drivers_and_cache_read_ratio() {
    let dir = tempdir().unwrap();
    let store = UsageStore::open(&dir.path().join("token-fire.sqlite")).unwrap();
    let now_utc = Utc.with_ymd_and_hms(2026, 7, 4, 12, 0, 0).unwrap();
    let now_local = now_utc.with_timezone(&Local);
    let row = cost_observation(
        "profile-drivers",
        Some("gpt-5.5"),
        1_000_000,
        500_000,
        200_000,
        100_000,
        50_000,
        1_850_000,
        now_utc - chrono::Duration::minutes(30),
    );
    insert_tracked(&store, &row).unwrap();

    let summary = store
        .profile_summary_at(
            token_fire::core::profile::ProfilePeriod::Today,
            now_utc,
            now_local,
        )
        .unwrap();
    let drivers = summary.selected_period.cost_drivers;

    assert!(drivers.input_cost > 0.0);
    assert!(drivers.output_cost > 0.0);
    assert!(drivers.reasoning_output_cost > 0.0);
    assert!(drivers.cache_creation_input_cost > 0.0);
    assert!(drivers.cached_input_cost > 0.0);
    assert_eq!(drivers.unattributed_cost, 0.0);
    assert_eq!(drivers.cached_input_tokens, 200_000);
    assert!(drivers.cache_read_ratio > 0.0);
}

#[test]
fn profile_period_serializes_as_calendar_codes_and_accepts_legacy_aliases() {
    use serde_json::json;

    assert_eq!(
        serde_json::to_value(ProfilePeriod::Today).unwrap(),
        json!("today")
    );
    assert_eq!(
        serde_json::to_value(ProfilePeriod::ThisWeek).unwrap(),
        json!("this_week")
    );
    assert_eq!(
        serde_json::to_value(ProfilePeriod::ThisMonth).unwrap(),
        json!("this_month")
    );
    assert_eq!(
        serde_json::to_value(ProfilePeriod::ThisYear).unwrap(),
        json!("this_year")
    );
    assert_eq!(
        serde_json::from_value::<ProfilePeriod>(json!("1d")).unwrap(),
        ProfilePeriod::Today
    );
    assert_eq!(
        serde_json::from_value::<ProfilePeriod>(json!("one_day")).unwrap(),
        ProfilePeriod::Today
    );
    assert_eq!(
        serde_json::from_value::<ProfilePeriod>(json!("1w")).unwrap(),
        ProfilePeriod::ThisWeek
    );
    assert_eq!(
        serde_json::from_value::<ProfilePeriod>(json!("1m")).unwrap(),
        ProfilePeriod::ThisMonth
    );
    assert_eq!(
        serde_json::from_value::<ProfilePeriod>(json!("one_month")).unwrap(),
        ProfilePeriod::ThisMonth
    );
    assert_eq!(
        serde_json::from_value::<ProfilePeriod>(json!("1y")).unwrap(),
        ProfilePeriod::ThisYear
    );
    assert_eq!(
        serde_json::from_value::<ProfilePeriod>(json!("one_year")).unwrap(),
        ProfilePeriod::ThisYear
    );
    assert_eq!(
        serde_json::from_value::<ProfilePeriod>(json!("one_week")).unwrap(),
        ProfilePeriod::ThisWeek
    );
    assert!(serde_json::from_value::<ProfilePeriod>(json!("last_30_days")).is_err());
}

#[test]
fn profile_period_bounds_use_local_calendar_periods() {
    let now_utc = Utc.with_ymd_and_hms(2026, 7, 8, 12, 34, 56).unwrap();
    let now_local = now_utc.with_timezone(&Local);

    let (today_start, today_end) =
        token_fire::core::profile::period_bounds(ProfilePeriod::Today, now_utc, now_local).unwrap();
    let (week_start, week_end) =
        token_fire::core::profile::period_bounds(ProfilePeriod::ThisWeek, now_utc, now_local)
            .unwrap();
    let (month_start, month_end) =
        token_fire::core::profile::period_bounds(ProfilePeriod::ThisMonth, now_utc, now_local)
            .unwrap();
    let (year_start, year_end) =
        token_fire::core::profile::period_bounds(ProfilePeriod::ThisYear, now_utc, now_local)
            .unwrap();

    assert_eq!(today_end, now_utc);
    assert_eq!(week_end, now_utc);
    assert_eq!(month_end, now_utc);
    assert_eq!(year_end, now_utc);

    let today_local = today_start.with_timezone(&Local);
    assert_eq!(today_local.date_naive(), now_local.date_naive());
    assert_eq!(today_local.format("%H:%M:%S").to_string(), "00:00:00");

    let week_local = week_start.with_timezone(&Local);
    assert_eq!(
        week_local.weekday(),
        chrono::Weekday::Mon,
        "this_week must start on local Monday"
    );
    assert_eq!(week_local.format("%H:%M:%S").to_string(), "00:00:00");
    assert!(week_start <= today_start);

    let month_local = month_start.with_timezone(&Local);
    assert_eq!(month_local.year(), now_local.year());
    assert_eq!(month_local.month(), now_local.month());
    assert_eq!(month_local.day(), 1);
    assert_eq!(month_local.format("%H:%M:%S").to_string(), "00:00:00");

    let year_local = year_start.with_timezone(&Local);
    assert_eq!(year_local.year(), now_local.year());
    assert_eq!(year_local.month(), 1);
    assert_eq!(year_local.day(), 1);
    assert_eq!(year_local.format("%H:%M:%S").to_string(), "00:00:00");
}

#[test]
fn profile_calendar_periods_include_and_exclude_boundary_rows() {
    let dir = tempdir().unwrap();
    let store = UsageStore::open(&dir.path().join("token-fire.sqlite")).unwrap();
    let now_utc = Utc.with_ymd_and_hms(2026, 7, 8, 12, 0, 0).unwrap();
    let now_local = now_utc.with_timezone(&Local);
    let (week_start, _) =
        token_fire::core::profile::period_bounds(ProfilePeriod::ThisWeek, now_utc, now_local)
            .unwrap();
    let (month_start, _) =
        token_fire::core::profile::period_bounds(ProfilePeriod::ThisMonth, now_utc, now_local)
            .unwrap();
    let (year_start, _) =
        token_fire::core::profile::period_bounds(ProfilePeriod::ThisYear, now_utc, now_local)
            .unwrap();

    for (id, observed_at, total_tokens) in [
        (
            "before-week",
            week_start - chrono::Duration::seconds(1),
            1_000_000,
        ),
        ("at-week", week_start, 2_000_000),
        (
            "before-month",
            month_start - chrono::Duration::seconds(1),
            4_000_000,
        ),
        ("at-month", month_start, 8_000_000),
        (
            "before-year",
            year_start - chrono::Duration::seconds(1),
            16_000_000,
        ),
        ("at-year", year_start, 32_000_000),
        ("at-period-end", now_utc, 64_000_000),
    ] {
        insert_tracked(
            &store,
            &cost_observation(
                id,
                Some("gpt-5.5"),
                0,
                0,
                0,
                0,
                0,
                total_tokens,
                observed_at,
            ),
        )
        .unwrap();
    }

    let week = store
        .profile_summary_at(ProfilePeriod::ThisWeek, now_utc, now_local)
        .unwrap();
    let month = store
        .profile_summary_at(ProfilePeriod::ThisMonth, now_utc, now_local)
        .unwrap();
    let year = store
        .profile_summary_at(ProfilePeriod::ThisYear, now_utc, now_local)
        .unwrap();

    assert_eq!(week.selected_period.total_tokens, 2_000_000);
    assert_eq!(
        month.selected_period.total_tokens,
        8_000_000 + 1_000_000 + 2_000_000
    );
    assert_eq!(
        year.selected_period.total_tokens,
        32_000_000 + 1_000_000 + 2_000_000 + 4_000_000 + 8_000_000
    );
}

#[test]
fn profile_period_totals_match_breakdown_totals() {
    let dir = tempdir().unwrap();
    let store = UsageStore::open(&dir.path().join("token-fire.sqlite")).unwrap();
    let now_utc = Utc.with_ymd_and_hms(2026, 7, 4, 12, 0, 0).unwrap();
    let now_local = now_utc.with_timezone(&Local);

    let mut row_a = cost_observation(
        "profile-consistency-a",
        Some("gpt-5.5"),
        1_000_000,
        500_000,
        0,
        0,
        0,
        1_500_000,
        now_utc - chrono::Duration::hours(2),
    );
    row_a.source = "traex".to_string();
    insert_tracked(&store, &row_a).unwrap();

    let mut row_b = cost_observation(
        "profile-consistency-b",
        Some("gpt-5.5"),
        0,
        0,
        0,
        0,
        0,
        2_000_000,
        now_utc - chrono::Duration::hours(1),
    );
    row_b.source = "codex".to_string();
    insert_tracked(&store, &row_b).unwrap();

    let summary = store
        .profile_summary_at(ProfilePeriod::ThisWeek, now_utc, now_local)
        .unwrap();
    let model_total: f64 = summary
        .selected_period
        .model_breakdown
        .iter()
        .map(|row| row.estimated_cost)
        .sum();
    let source_total: f64 = summary
        .selected_period
        .source_breakdown
        .iter()
        .map(|row| row.estimated_cost)
        .sum();

    assert_cost_close(summary.selected_period.estimated_cost, model_total);
    assert_cost_close(summary.selected_period.estimated_cost, source_total);
    assert!(summary.selected_period.cost_drivers.unattributed_cost > 0.0);
}

#[test]
fn profile_period_breakdowns_include_other_bucket_when_more_than_ten_groups() {
    let dir = tempdir().unwrap();
    let store = UsageStore::open(&dir.path().join("token-fire.sqlite")).unwrap();
    let now_utc = Utc.with_ymd_and_hms(2026, 7, 4, 12, 0, 0).unwrap();
    let now_local = now_utc.with_timezone(&Local);

    for index in 0..12 {
        let mut row = cost_observation(
            &format!("profile-cardinality-{index}"),
            Some(&format!("model-{index:02}")),
            0,
            0,
            0,
            0,
            0,
            (12 - index) * 1_000_000,
            now_utc - chrono::Duration::minutes(index as i64 + 1),
        );
        row.source = format!("source-{index:02}");
        insert_tracked(&store, &row).unwrap();
    }

    let summary = store
        .profile_summary_at(ProfilePeriod::ThisWeek, now_utc, now_local)
        .unwrap();
    let model_breakdown = &summary.selected_period.model_breakdown;
    let source_breakdown = &summary.selected_period.source_breakdown;
    let model_total: f64 = model_breakdown.iter().map(|row| row.estimated_cost).sum();
    let source_total: f64 = source_breakdown.iter().map(|row| row.estimated_cost).sum();

    assert_eq!(model_breakdown.len(), 10);
    assert_eq!(source_breakdown.len(), 10);
    assert_eq!(model_breakdown.last().unwrap().key, "other");
    assert_eq!(model_breakdown.last().unwrap().label, "Other");
    assert_eq!(source_breakdown.last().unwrap().key, "other");
    assert_eq!(source_breakdown.last().unwrap().label, "Other");
    assert_cost_close(summary.selected_period.estimated_cost, model_total);
    assert_cost_close(summary.selected_period.estimated_cost, source_total);
    assert_eq!(
        model_breakdown.last().unwrap().total_tokens,
        (1 + 2 + 3) * 1_000_000
    );
    assert_eq!(
        source_breakdown.last().unwrap().total_tokens,
        (1 + 2 + 3) * 1_000_000
    );
}

#[test]
fn profile_year_heatmap_uses_exact_local_date_window() {
    let dir = tempdir().unwrap();
    let store = UsageStore::open(&dir.path().join("token-fire.sqlite")).unwrap();
    let now_utc = Utc.with_ymd_and_hms(2026, 7, 4, 1, 0, 0).unwrap();
    let now_local = now_utc.with_timezone(&Local);

    let summary = store
        .profile_summary_at(ProfilePeriod::ThisYear, now_utc, now_local)
        .unwrap();

    assert_eq!(summary.year_profile.days.len(), 365);
    assert_eq!(
        summary.year_profile.days.last().unwrap().local_date,
        now_local.date_naive()
    );
    assert_eq!(
        summary.year_profile.days.first().unwrap().local_date,
        now_local.date_naive() - chrono::Days::new(364)
    );
}

#[test]
fn widget_cost_summary_today_ends_at_now_and_seven_days_is_rolling() {
    let dir = tempdir().unwrap();
    let store = UsageStore::open(&dir.path().join("token-fire.sqlite")).unwrap();
    let now_utc = Utc.with_ymd_and_hms(2026, 7, 2, 4, 0, 0).unwrap();
    let now_local = now_utc.with_timezone(&Local);

    insert_tracked(
        &store,
        &cost_observation(
            "today",
            Some("unknown-model"),
            0,
            0,
            0,
            0,
            0,
            1_000_000,
            now_utc - chrono::Duration::minutes(10),
        ),
    )
    .unwrap();
    insert_tracked(
        &store,
        &cost_observation(
            "future-today",
            Some("unknown-model"),
            0,
            0,
            0,
            0,
            0,
            1_000_000,
            now_utc + chrono::Duration::minutes(10),
        ),
    )
    .unwrap();
    insert_tracked(
        &store,
        &cost_observation(
            "seven-day",
            Some("unknown-model"),
            0,
            0,
            0,
            0,
            0,
            2_000_000,
            now_utc - chrono::Duration::days(6),
        ),
    )
    .unwrap();
    insert_tracked(
        &store,
        &cost_observation(
            "too-old-seven-day",
            Some("unknown-model"),
            0,
            0,
            0,
            0,
            0,
            4_000_000,
            now_utc - chrono::Duration::days(7) - chrono::Duration::seconds(1),
        ),
    )
    .unwrap();

    let summary = store.widget_cost_summary_at(now_utc, now_local).unwrap();

    assert_eq!(summary.currency, "CNY");
    assert_eq!(summary.generated_at, now_utc);
    assert_eq!(summary.today.total_tokens, 1_000_000);
    assert_eq!(summary.seven_days.total_tokens, 3_000_000);
}
