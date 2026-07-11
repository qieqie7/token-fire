use std::path::Path;

use chrono::{Datelike, Local, TimeZone, Utc};
use rusqlite::{params, Connection};
use tempfile::tempdir;
use token_fire::core::observation::{NormalizedObservation, SourceRecordIdConfidence};
use token_fire::core::pricing::{PricingStatus, DEFAULT_AVERAGE_CNY_PER_1M_TOKENS};
use token_fire::core::profile::{ProfilePeriod, ProfileSummary, RankedProfileBreakdown};
use token_fire::core::profile_rollup::{bucket_start_utc, PROFILE_ROLLUP_SCHEMA_VERSION};
use token_fire::core::usage_series::{WIDGET_USAGE_BUCKET_MINUTES, WIDGET_USAGE_WINDOW_MINUTES};
use token_fire::core::usage_store::{
    InsertOutcome, ProfileRollupStatus, RetentionPolicy, RetentionSkipReason, UsageStore,
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

/// 逐条比较两个 ranked breakdown：key/label/total_tokens/share 必须精确相等，
/// 只有浮点成本走 assert_cost_close 容差。用于证明 rollup 与 raw 排序、分组一致。
#[allow(dead_code)]
fn assert_breakdown_close(
    context: &str,
    actual: &[RankedProfileBreakdown],
    expected: &[RankedProfileBreakdown],
) {
    assert_eq!(
        actual.len(),
        expected.len(),
        "{context} breakdown length mismatch"
    );
    for (index, (actual_row, expected_row)) in actual.iter().zip(expected.iter()).enumerate() {
        assert_eq!(actual_row.key, expected_row.key, "{context}[{index}] key");
        assert_eq!(
            actual_row.label, expected_row.label,
            "{context}[{index}] label"
        );
        assert_eq!(
            actual_row.total_tokens, expected_row.total_tokens,
            "{context}[{index}] total_tokens"
        );
        assert_eq!(
            actual_row.share, expected_row.share,
            "{context}[{index}] share"
        );
        assert_cost_close(actual_row.estimated_cost, expected_row.estimated_cost);
    }
}

/// 完整 ProfileSummary parity 断言：逐项比较 365 个 day bucket、model/source
/// breakdown、cost drivers、peak day、selected period trend。token/日期/标签/排序/
/// 桶边界必须精确相等，只有浮点成本用 assert_cost_close 容差。供后续 rollup ==
/// raw 等价性测试复用，不允许只比较总量。
#[allow(dead_code)]
fn assert_profile_summary_close(actual: &ProfileSummary, expected: &ProfileSummary) {
    assert_eq!(actual.generated_at, expected.generated_at);
    assert_eq!(actual.currency, expected.currency);

    // year profile: 逐日 heatmap 与聚合指标
    assert_eq!(
        actual.year_profile.days.len(),
        expected.year_profile.days.len(),
        "year_profile.days length"
    );
    for (index, (actual_day, expected_day)) in actual
        .year_profile
        .days
        .iter()
        .zip(expected.year_profile.days.iter())
        .enumerate()
    {
        assert_eq!(
            actual_day.local_date, expected_day.local_date,
            "year day[{index}] local_date"
        );
        assert_eq!(
            actual_day.total_tokens, expected_day.total_tokens,
            "year day[{index}] total_tokens"
        );
        assert_eq!(
            actual_day.intensity, expected_day.intensity,
            "year day[{index}] intensity"
        );
        assert_cost_close(actual_day.estimated_cost, expected_day.estimated_cost);
    }
    assert_eq!(
        actual.year_profile.total_tokens, expected.year_profile.total_tokens,
        "year_profile.total_tokens"
    );
    assert_eq!(
        actual.year_profile.active_days, expected.year_profile.active_days,
        "year_profile.active_days"
    );
    assert_cost_close(
        actual.year_profile.estimated_cost,
        expected.year_profile.estimated_cost,
    );
    assert_cost_close(
        actual.year_profile.average_active_day_cost,
        expected.year_profile.average_active_day_cost,
    );
    match (
        &actual.year_profile.peak_day,
        &expected.year_profile.peak_day,
    ) {
        (Some(actual_peak), Some(expected_peak)) => {
            assert_eq!(
                actual_peak.local_date, expected_peak.local_date,
                "peak_day.local_date"
            );
            assert_eq!(
                actual_peak.total_tokens, expected_peak.total_tokens,
                "peak_day.total_tokens"
            );
            assert_cost_close(actual_peak.estimated_cost, expected_peak.estimated_cost);
        }
        (None, None) => {}
        _ => panic!("peak_day presence mismatch"),
    }

    // selected period: 精确总量/边界/趋势桶，浮点成本走容差
    let actual_period = &actual.selected_period;
    let expected_period = &expected.selected_period;
    assert_eq!(actual_period.period, expected_period.period, "period");
    assert_eq!(
        actual_period.started_at, expected_period.started_at,
        "period.started_at"
    );
    assert_eq!(
        actual_period.ended_at, expected_period.ended_at,
        "period.ended_at"
    );
    assert_eq!(
        actual_period.total_tokens, expected_period.total_tokens,
        "period.total_tokens"
    );
    assert_cost_close(actual_period.estimated_cost, expected_period.estimated_cost);
    // trend 只含 token/日期/标签/桶边界（无浮点），可整体精确比较
    assert_eq!(actual_period.trend, expected_period.trend, "period.trend");

    assert_breakdown_close(
        "model_breakdown",
        &actual_period.model_breakdown,
        &expected_period.model_breakdown,
    );
    assert_breakdown_close(
        "source_breakdown",
        &actual_period.source_breakdown,
        &expected_period.source_breakdown,
    );

    // cost drivers: 成本分量与比率走容差，token 计数精确
    let actual_drivers = &actual_period.cost_drivers;
    let expected_drivers = &expected_period.cost_drivers;
    assert_cost_close(actual_drivers.input_cost, expected_drivers.input_cost);
    assert_cost_close(actual_drivers.output_cost, expected_drivers.output_cost);
    assert_cost_close(
        actual_drivers.reasoning_output_cost,
        expected_drivers.reasoning_output_cost,
    );
    assert_cost_close(
        actual_drivers.cache_creation_input_cost,
        expected_drivers.cache_creation_input_cost,
    );
    assert_cost_close(
        actual_drivers.cached_input_cost,
        expected_drivers.cached_input_cost,
    );
    assert_cost_close(
        actual_drivers.unattributed_cost,
        expected_drivers.unattributed_cost,
    );
    assert_eq!(
        actual_drivers.cached_input_tokens, expected_drivers.cached_input_tokens,
        "cost_drivers.cached_input_tokens"
    );
    assert_cost_close(
        actual_drivers.cache_read_ratio,
        expected_drivers.cache_read_ratio,
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

/// 读取 rollup metadata（state / schema_version 等）；缺表或缺行时返回 None。
fn rollup_metadata_value(db_path: &Path, key: &str) -> Option<String> {
    Connection::open(db_path)
        .unwrap()
        .query_row(
            "select value from usage_rollup_metadata where key = ?1",
            [key],
            |row| row.get(0),
        )
        .ok()
}

fn rollup_row_count(db_path: &Path) -> i64 {
    Connection::open(db_path)
        .unwrap()
        .query_row("select count(*) from usage_rollups_15m", [], |row| {
            row.get(0)
        })
        .unwrap()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RollupRow {
    bucket_start_utc: i64,
    source: String,
    model: String,
    input_tokens: i64,
    billable_uncached_input_tokens: i64,
    output_tokens: i64,
    cached_input_tokens: i64,
    cache_creation_input_tokens: i64,
    reasoning_output_tokens: i64,
    unattributed_total_tokens: i64,
    total_tokens: i64,
    observation_count: i64,
}

fn rollup_rows(db_path: &Path) -> Vec<RollupRow> {
    let conn = Connection::open(db_path).unwrap();
    let mut stmt = conn
        .prepare(
            r#"
            select bucket_start_utc, source, model, input_tokens, billable_uncached_input_tokens,
                   output_tokens, cached_input_tokens, cache_creation_input_tokens,
                   reasoning_output_tokens, unattributed_total_tokens, total_tokens, observation_count
            from usage_rollups_15m
            order by bucket_start_utc, source, model
            "#,
        )
        .unwrap();
    let rows = stmt
        .query_map([], |row| {
            Ok(RollupRow {
                bucket_start_utc: row.get(0)?,
                source: row.get(1)?,
                model: row.get(2)?,
                input_tokens: row.get(3)?,
                billable_uncached_input_tokens: row.get(4)?,
                output_tokens: row.get(5)?,
                cached_input_tokens: row.get(6)?,
                cache_creation_input_tokens: row.get(7)?,
                reasoning_output_tokens: row.get(8)?,
                unattributed_total_tokens: row.get(9)?,
                total_tokens: row.get(10)?,
                observation_count: row.get(11)?,
            })
        })
        .unwrap();
    rows.map(Result::unwrap).collect()
}

/// 从 tracked raw 行按与 upsert/rebuild 相同的 clamp/CASE 语义聚合 8 个 token 分量 + count。
/// 顺序：input / billable / output / cached / cache_creation / reasoning / unattributed / total / count。
fn raw_conservation_totals(db_path: &Path) -> [i64; 9] {
    Connection::open(db_path)
        .unwrap()
        .query_row(
            r#"
            select
              coalesce(sum(input_tokens), 0),
              coalesce(sum(max(input_tokens - cached_input_tokens - cache_creation_input_tokens, 0)), 0),
              coalesce(sum(output_tokens), 0),
              coalesce(sum(cached_input_tokens), 0),
              coalesce(sum(cache_creation_input_tokens), 0),
              coalesce(sum(reasoning_output_tokens), 0),
              coalesce(sum(case when input_tokens = 0 and output_tokens = 0 and cached_input_tokens = 0
                        and cache_creation_input_tokens = 0 and reasoning_output_tokens = 0
                       then max(total_tokens, 0) else 0 end), 0),
              coalesce(sum(total_tokens), 0),
              count(*)
            from token_observations
            where tracking_window_id is not null
            "#,
            [],
            |row| {
                Ok([
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                ])
            },
        )
        .unwrap()
}

/// 与 raw_conservation_totals 对齐的 rollup 侧聚合（预聚合列直接求和），供全局守恒断言比较。
fn rollup_conservation_totals(db_path: &Path) -> [i64; 9] {
    Connection::open(db_path)
        .unwrap()
        .query_row(
            r#"
            select
              coalesce(sum(input_tokens), 0),
              coalesce(sum(billable_uncached_input_tokens), 0),
              coalesce(sum(output_tokens), 0),
              coalesce(sum(cached_input_tokens), 0),
              coalesce(sum(cache_creation_input_tokens), 0),
              coalesce(sum(reasoning_output_tokens), 0),
              coalesce(sum(unattributed_total_tokens), 0),
              coalesce(sum(total_tokens), 0),
              coalesce(sum(observation_count), 0)
            from usage_rollups_15m
            "#,
            [],
            |row| {
                Ok([
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                ])
            },
        )
        .unwrap()
}

/// 造一批分量丰富的 tracked 观测：既有含全部分量的行，也有 total-only（unattributed）行、
/// 缺 model（key='' ）行与多桶多来源行，覆盖全局守恒与逐 key 语义。
fn insert_rebuild_fixture(store: &UsageStore) {
    let base = Utc.with_ymd_and_hms(2026, 7, 11, 12, 7, 33).unwrap();
    insert_tracked(
        store,
        &cost_observation(
            "rb-1",
            Some("model-a"),
            1_000_000,
            500_000,
            250_000,
            100_000,
            50_000,
            1_900_000,
            base,
        ),
    )
    .unwrap();
    // 同桶同 key 追加一条 total-only，验证 unattributed 单独累积。
    insert_tracked(
        store,
        &cost_observation(
            "rb-2",
            Some("model-a"),
            0,
            0,
            0,
            0,
            0,
            1_000_000,
            base + chrono::Duration::minutes(3),
        ),
    )
    .unwrap();
    // 缺 model（key=''）+ 不同来源，落入不同 rollup 行。
    let mut codex = cost_observation(
        "rb-3",
        None,
        7,
        3,
        0,
        0,
        0,
        10,
        base + chrono::Duration::hours(1),
    );
    codex.source = "codex".to_string();
    insert_tracked(store, &codex).unwrap();
    // 远期桶，确保多桶。
    insert_tracked(
        store,
        &cost_observation(
            "rb-4",
            Some("model-b"),
            2_000,
            1_000,
            0,
            0,
            0,
            3_000,
            base + chrono::Duration::days(3),
        ),
    )
    .unwrap();
}

#[test]
fn profile_rollup_rebuild_status_requires_rebuild_when_state_missing() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("token-fire.sqlite");
    let store = UsageStore::open(&db_path).unwrap();

    // 增量双写已建 rollup 行，但 migration 从不写 state/schema_version：启动需 rebuild。
    insert_rebuild_fixture(&store);
    assert_eq!(rollup_metadata_value(&db_path, "state"), None);

    assert_eq!(
        store.profile_rollup_status().unwrap(),
        ProfileRollupStatus::RebuildRequired {
            reason: "state_missing"
        }
    );
}

#[test]
fn profile_rollup_rebuild_status_requires_rebuild_when_state_invalid() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("token-fire.sqlite");
    let store = UsageStore::open(&db_path).unwrap();
    insert_rebuild_fixture(&store);
    Connection::open(&db_path)
        .unwrap()
        .execute(
            "insert into usage_rollup_metadata (key, value) values ('state', 'invalid')",
            [],
        )
        .unwrap();

    assert_eq!(
        store.profile_rollup_status().unwrap(),
        ProfileRollupStatus::RebuildRequired {
            reason: "state_invalid"
        }
    );
}

#[test]
fn profile_rollup_rebuild_status_requires_rebuild_on_version_mismatch() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("token-fire.sqlite");
    let mut store = UsageStore::open(&db_path).unwrap();
    insert_rebuild_fixture(&store);
    store.rebuild_profile_rollups().unwrap();

    // 人为把 schema_version 改成旧值：即使 state=ready 也必须要求 rebuild。
    Connection::open(&db_path)
        .unwrap()
        .execute(
            "update usage_rollup_metadata set value = '0' where key = 'schema_version'",
            [],
        )
        .unwrap();

    assert_eq!(
        store.profile_rollup_status().unwrap(),
        ProfileRollupStatus::RebuildRequired {
            reason: "version_mismatch"
        }
    );
}

#[test]
fn profile_rollup_rebuild_status_requires_rebuild_on_checksum_mismatch() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("token-fire.sqlite");
    let mut store = UsageStore::open(&db_path).unwrap();
    insert_rebuild_fixture(&store);
    store.rebuild_profile_rollups().unwrap();
    assert!(matches!(
        store.profile_rollup_status().unwrap(),
        ProfileRollupStatus::Ready { .. }
    ));

    // 篡改一行 rollup total，使全局守恒 checksum 与 raw 不一致（state/version 仍 ready）。
    Connection::open(&db_path)
        .unwrap()
        .execute(
            "update usage_rollups_15m set total_tokens = total_tokens + 1 where rowid = (select min(rowid) from usage_rollups_15m)",
            [],
        )
        .unwrap();

    assert_eq!(
        store.profile_rollup_status().unwrap(),
        ProfileRollupStatus::RebuildRequired {
            reason: "checksum_mismatch"
        }
    );
}

#[test]
fn profile_rollup_rebuild_recomputes_all_token_components_from_raw() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("token-fire.sqlite");
    let mut store = UsageStore::open(&db_path).unwrap();
    insert_rebuild_fixture(&store);

    let outcome = store.rebuild_profile_rollups().unwrap();
    assert!(outcome.rebuilt);
    assert_eq!(outcome.schema_version, PROFILE_ROLLUP_SCHEMA_VERSION);
    assert_eq!(outcome.rollup_row_count as i64, rollup_row_count(&db_path));

    // rebuild 后所有 8 个 token 分量 + unattributed + count 的全局总和都等于 raw。
    assert_eq!(
        rollup_conservation_totals(&db_path),
        raw_conservation_totals(&db_path)
    );
    // rebuild 翻转到 ready + 当前 schema_version。
    assert_eq!(
        rollup_metadata_value(&db_path, "state"),
        Some("ready".to_string())
    );
    assert_eq!(
        rollup_metadata_value(&db_path, "schema_version"),
        Some(PROFILE_ROLLUP_SCHEMA_VERSION.to_string())
    );
    assert!(matches!(
        store.profile_rollup_status().unwrap(),
        ProfileRollupStatus::Ready { .. }
    ));
}

#[test]
fn profile_rollup_rebuild_shadow_failure_keeps_old_rollup_and_version() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("token-fire.sqlite");
    let mut store = UsageStore::open(&db_path).unwrap();
    insert_rebuild_fixture(&store);
    store.rebuild_profile_rollups().unwrap();

    // 记录旧完整状态：行内容 + 全局校验 + schema_version。
    let old_rows = rollup_rows(&db_path);
    let old_totals = rollup_conservation_totals(&db_path);
    assert!(!old_rows.is_empty());

    // 用同名 index 占用 shadow 表名：drop table if exists 不清理 index，
    // 后续 create table usage_rollups_15m_rebuild 必失败 → rebuild body 事务回滚。
    Connection::open(&db_path)
        .unwrap()
        .execute(
            "create index usage_rollups_15m_rebuild on usage_rollups_15m(source)",
            [],
        )
        .unwrap();

    let err = store.rebuild_profile_rollups();
    assert!(err.is_err(), "shadow create 冲突应使 rebuild 失败");

    // 旧表仍可查询且内容不变（原子性：只可能是完整旧状态）。
    assert_eq!(rollup_rows(&db_path), old_rows);
    assert_eq!(rollup_conservation_totals(&db_path), old_totals);
    // 旧 version 不变；state 不是 ready（失败后停留 invalid）。
    assert_eq!(
        rollup_metadata_value(&db_path, "schema_version"),
        Some(PROFILE_ROLLUP_SCHEMA_VERSION.to_string())
    );
    assert_ne!(
        rollup_metadata_value(&db_path, "state"),
        Some("ready".to_string())
    );
}

#[test]
fn profile_rollup_ensure_ready_validates_once_and_does_not_rebuild_again() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("token-fire.sqlite");
    let mut store = UsageStore::open(&db_path).unwrap();
    insert_rebuild_fixture(&store);

    // 首次：state 缺失 → RebuildRequired → 真正 rebuild。
    let first = store.ensure_profile_rollup_ready().unwrap();
    assert!(first.rebuilt);
    assert_eq!(first.schema_version, PROFILE_ROLLUP_SCHEMA_VERSION);

    // 第二次：已 Ready → 不再 rebuild，直接返回 rebuilt:false。
    let second = store.ensure_profile_rollup_ready().unwrap();
    assert!(!second.rebuilt);
    assert_eq!(second.rollup_row_count, first.rollup_row_count);
    assert_eq!(second.schema_version, PROFILE_ROLLUP_SCHEMA_VERSION);
}

#[test]
fn profile_rollup_rebuild_bucket_matches_rust_single_write_bucket() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("token-fire.sqlite");
    let mut store = UsageStore::open(&db_path).unwrap();

    // 非 15 分钟对齐时间：验证 DB 侧 unixepoch()/900*900 与 Rust div_euclid 落入同一桶。
    let observed_at = Utc.with_ymd_and_hms(2026, 7, 11, 12, 14, 59).unwrap();
    insert_tracked(&store, &observation("bucket-parity", 100, observed_at)).unwrap();

    // 增量单条写入用 Rust bucket_start_utc。
    let rust_bucket = rollup_rows(&db_path)[0].bucket_start_utc;
    assert_eq!(rust_bucket, bucket_start_utc(observed_at));

    // 全量 rebuild 用 SQL unixepoch()/900*900 重算 bucket，必须与 Rust 相同。
    store.rebuild_profile_rollups().unwrap();
    let rebuilt = rollup_rows(&db_path);
    assert_eq!(rebuilt.len(), 1);
    assert_eq!(rebuilt[0].bucket_start_utc, rust_bucket);
}

#[test]
fn profile_rollup_write_migration_creates_tables_without_version() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("token-fire.sqlite");

    UsageStore::open(&db_path).unwrap();

    // migration 只建 schema：两张表存在，但不预写 schema_version / state（Task 5 拥有 rebuild）。
    assert!(table_exists(&db_path, "usage_rollups_15m"));
    assert!(table_exists(&db_path, "usage_rollup_metadata"));
    assert_eq!(rollup_metadata_value(&db_path, "schema_version"), None);
    assert_eq!(rollup_metadata_value(&db_path, "state"), None);
    assert_eq!(rollup_row_count(&db_path), 0);
}

#[test]
fn profile_rollup_write_tracked_insert_writes_normalized_rollup_row() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("token-fire.sqlite");
    let store = UsageStore::open(&db_path).unwrap();
    let bucket_time = Utc.with_ymd_and_hms(2026, 7, 11, 12, 0, 0).unwrap();
    let expected_bucket = Utc
        .with_ymd_and_hms(2026, 7, 11, 12, 0, 0)
        .unwrap()
        .timestamp();

    // model=None 必须规范化为空字符串 key；billable/unattributed 走 Task 2 的 clamp/CASE 语义。
    let component = cost_observation(
        "rollup-component",
        None,
        1_000_000,
        500_000,
        250_000,
        100_000,
        50_000,
        1_900_000,
        bucket_time,
    );
    insert_tracked(&store, &component).unwrap();

    // 同 bucket/source/model 的 total-only 观测应累加进同一 rollup 行，并单独累积 unattributed。
    let total_only = cost_observation(
        "rollup-total-only",
        None,
        0,
        0,
        0,
        0,
        0,
        1_000_000,
        bucket_time + chrono::Duration::minutes(5),
    );
    insert_tracked(&store, &total_only).unwrap();

    assert_eq!(observation_count(&db_path), 2);
    let rows = rollup_rows(&db_path);
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.bucket_start_utc, expected_bucket);
    assert_eq!(row.source, "traex");
    assert_eq!(row.model, "");
    assert_eq!(row.input_tokens, 1_000_000);
    assert_eq!(row.billable_uncached_input_tokens, 650_000);
    assert_eq!(row.output_tokens, 500_000);
    assert_eq!(row.cached_input_tokens, 250_000);
    assert_eq!(row.cache_creation_input_tokens, 100_000);
    assert_eq!(row.reasoning_output_tokens, 50_000);
    assert_eq!(row.unattributed_total_tokens, 1_000_000);
    assert_eq!(row.total_tokens, 2_900_000);
    assert_eq!(row.observation_count, 2);
}

#[test]
fn profile_rollup_write_duplicate_insert_does_not_change_rollup() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("token-fire.sqlite");
    let store = UsageStore::open(&db_path).unwrap();
    let observed_at = Utc.with_ymd_and_hms(2026, 7, 11, 12, 0, 0).unwrap();
    let row = observation("rollup-dup", 100, observed_at);

    assert_eq!(
        insert_tracked(&store, &row).unwrap(),
        InsertOutcome::Inserted
    );
    assert_eq!(
        insert_tracked(&store, &row).unwrap(),
        InsertOutcome::Duplicate
    );

    let rows = rollup_rows(&db_path);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].total_tokens, 100);
    assert_eq!(rows[0].observation_count, 1);
}

#[test]
fn profile_rollup_write_untracked_insert_writes_no_rollup_row() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("token-fire.sqlite");
    let store = UsageStore::open(&db_path).unwrap();
    let observed_at = Utc.with_ymd_and_hms(2026, 7, 11, 12, 0, 0).unwrap();

    store
        .insert_untracked_observation_for_test(&observation("rollup-untracked", 100, observed_at))
        .unwrap();

    assert_eq!(observation_count(&db_path), 1);
    assert_eq!(rollup_row_count(&db_path), 0);
}

#[test]
fn profile_rollup_write_failure_keeps_raw_and_marks_invalid() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("token-fire.sqlite");
    UsageStore::open(&db_path).unwrap();

    // 在 rollup 表安装 BEFORE INSERT 失败触发器，模拟派生模型写入故障。
    Connection::open(&db_path)
        .unwrap()
        .execute_batch(
            r#"
            create trigger fail_rollup_insert
            before insert on usage_rollups_15m
            begin
              select raise(fail, 'rollup write failed');
            end;
            "#,
        )
        .unwrap();

    let store = UsageStore::open(&db_path).unwrap();
    let first = observation(
        "rollup-fail-1",
        100,
        Utc.with_ymd_and_hms(2026, 7, 11, 12, 0, 0).unwrap(),
    );

    // OVERRIDE A：raw 是唯一事实源，rollup 写失败不能丢 raw；返回 Inserted 而非错误。
    assert_eq!(
        insert_tracked(&store, &first).unwrap(),
        InsertOutcome::Inserted
    );
    assert_eq!(observation_count(&db_path), 1);
    assert_eq!(rollup_row_count(&db_path), 0);
    assert_eq!(
        rollup_metadata_value(&db_path, "state"),
        Some("invalid".to_string())
    );

    // 进入 raw-only 降级：后续 tracked insert 只写 raw，rollup 不增长，也不再报错。
    let second = observation(
        "rollup-fail-2",
        200,
        Utc.with_ymd_and_hms(2026, 7, 11, 12, 1, 0).unwrap(),
    );
    assert_eq!(
        insert_tracked(&store, &second).unwrap(),
        InsertOutcome::Inserted
    );
    assert_eq!(observation_count(&db_path), 2);
    assert_eq!(rollup_row_count(&db_path), 0);
}

#[test]
fn profile_rollup_write_connection_uses_wal_full_busy_timeout() {
    let dir = tempdir().unwrap();
    let store = UsageStore::open(&dir.path().join("token-fire.sqlite")).unwrap();

    // OVERRIDE C：Profile 查询将与 ingest writer 真正并发，WAL + busy_timeout 是正确性而非润色。
    let pragmas = store.connection_pragmas().unwrap();
    assert_eq!(pragmas.journal_mode.to_lowercase(), "wal");
    assert_eq!(pragmas.busy_timeout, 5000);
    assert_eq!(pragmas.synchronous, 2); // FULL
    assert_eq!(pragmas.wal_autocheckpoint, 1000);
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

// 主用例：非 15 分钟对齐的 cutoff，边界桶必须只保留 cutoff 后贡献。
// 观测布置：边界桶前（完全过期）/ cutoff 前但在边界桶内（应从边界剔除）/ cutoff 后在边界桶内（应保留）/ 远期（不受影响）。
#[test]
fn retention_rebuilds_profile_rollup_boundary_keeps_only_post_cutoff() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("token-fire.sqlite");
    let mut store = UsageStore::open(&db_path).unwrap();

    // now 故意带 7 分 33 秒，使 cutoff 不落在 15 分钟边界。
    let now = Utc.with_ymd_and_hms(2026, 6, 26, 12, 7, 33).unwrap();
    let cutoff = now - chrono::Duration::days(365);
    assert_ne!(
        bucket_start_utc(cutoff),
        cutoff.timestamp(),
        "cutoff 应非对齐"
    );
    let cutoff_bucket = bucket_start_utc(cutoff);

    let expired = Utc.with_ymd_and_hms(2025, 6, 26, 11, 50, 0).unwrap();
    let pruned = Utc.with_ymd_and_hms(2025, 6, 26, 12, 3, 0).unwrap();
    let retained = Utc.with_ymd_and_hms(2025, 6, 26, 12, 10, 0).unwrap();
    let recent = Utc.with_ymd_and_hms(2026, 6, 25, 12, 7, 33).unwrap();
    let expired_bucket = bucket_start_utc(expired);
    let recent_bucket = bucket_start_utc(recent);

    insert_tracked(&store, &observation("expired", 1000, expired)).unwrap();
    insert_tracked(&store, &observation("pruned", 200, pruned)).unwrap();
    insert_tracked(&store, &observation("retained", 77, retained)).unwrap();
    insert_tracked(&store, &observation("recent", 5000, recent)).unwrap();

    let outcome = store
        .run_retention_if_due(now, RetentionPolicy::default())
        .unwrap();

    // cutoff 前 raw 被删除（expired + pruned）。
    assert!(outcome.ran);
    assert_eq!(outcome.deleted_observations, 2);
    assert_eq!(observation_count(&db_path), 2);

    let rows = rollup_rows(&db_path);
    // 完全过期 rollup 桶被删除。
    assert!(
        rows.iter()
            .all(|row| row.bucket_start_utc != expired_bucket),
        "完全过期桶应被删除"
    );
    // 远期桶不受影响。
    let recent_row = rows
        .iter()
        .find(|row| row.bucket_start_utc == recent_bucket)
        .expect("远期桶应保留");
    assert_eq!(recent_row.total_tokens, 5000);

    // 边界桶只保留 cutoff 后贡献（仅 retained=77）。
    let boundary = rows
        .iter()
        .find(|row| row.bucket_start_utc == cutoff_bucket)
        .expect("边界桶应存在且已重建");
    assert_eq!(boundary.source, "traex");
    assert_eq!(boundary.model, "model-a");
    assert_eq!(boundary.total_tokens, 77);
    assert_eq!(boundary.input_tokens, 77);
    assert_eq!(boundary.billable_uncached_input_tokens, 77);
    assert_eq!(boundary.output_tokens, 0);
    assert_eq!(boundary.unattributed_total_tokens, 0);
    assert_eq!(boundary.observation_count, 1);

    // retention metadata 与 raw/rollup 修改同事务提交。
    assert_eq!(
        state_value(&db_path, "last_success_at"),
        Some(now.to_rfc3339())
    );
    // Ready 模式不翻转 state。
    assert_eq!(rollup_metadata_value(&db_path, "state"), None);
}

// OVERRIDE D：rollup 边界维护失败时，raw 删除与 last_success_at 仍需持久化，rollup 降级为 invalid。
#[test]
fn retention_rebuilds_profile_rollup_boundary_degrades_on_rollup_failure() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("token-fire.sqlite");

    let now = Utc.with_ymd_and_hms(2026, 6, 26, 12, 7, 33).unwrap();
    let expired = Utc.with_ymd_and_hms(2025, 6, 26, 11, 50, 0).unwrap();
    let retained = Utc.with_ymd_and_hms(2025, 6, 26, 12, 10, 0).unwrap();

    // 先插入观测让增量 rollup 正常写入，再安装失败触发器只影响 retention 的重建 INSERT。
    let mut store = UsageStore::open(&db_path).unwrap();
    insert_tracked(&store, &observation("expired", 1000, expired)).unwrap();
    insert_tracked(&store, &observation("retained", 77, retained)).unwrap();
    drop(store);

    Connection::open(&db_path)
        .unwrap()
        .execute_batch(
            r#"
            create trigger fail_rollup_boundary_rebuild
            before insert on usage_rollups_15m
            begin
              select raise(fail, 'rollup boundary rebuild failed');
            end;
            "#,
        )
        .unwrap();

    let mut store = UsageStore::open(&db_path).unwrap();
    let outcome = store
        .run_retention_if_due(now, RetentionPolicy::default())
        .unwrap();

    // raw 删除仍生效（expired 被删，retained 保留），last_success_at 前进，rollup 降级 invalid。
    assert!(outcome.ran);
    assert_eq!(outcome.deleted_observations, 1);
    assert_eq!(observation_count(&db_path), 1);
    assert_eq!(
        state_value(&db_path, "last_success_at"),
        Some(now.to_rfc3339())
    );
    assert_eq!(
        rollup_metadata_value(&db_path, "state"),
        Some("invalid".to_string())
    );
}

// Invalid 模式：Retention 只维护 raw，保持 rollup invalid，不做边界重建。
#[test]
fn retention_rebuilds_profile_rollup_boundary_invalid_mode_is_raw_only() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("token-fire.sqlite");
    UsageStore::open(&db_path).unwrap();

    // 预置 state=invalid：后续 tracked insert 走 raw-only 降级，rollup 不写。
    Connection::open(&db_path)
        .unwrap()
        .execute(
            "insert into usage_rollup_metadata (key, value) values ('state', 'invalid')",
            [],
        )
        .unwrap();

    let now = Utc.with_ymd_and_hms(2026, 6, 26, 12, 7, 33).unwrap();
    let expired = Utc.with_ymd_and_hms(2025, 6, 26, 11, 50, 0).unwrap();
    let retained = Utc.with_ymd_and_hms(2025, 6, 26, 12, 10, 0).unwrap();

    let mut store = UsageStore::open(&db_path).unwrap();
    insert_tracked(&store, &observation("expired", 1000, expired)).unwrap();
    insert_tracked(&store, &observation("retained", 77, retained)).unwrap();
    assert_eq!(rollup_row_count(&db_path), 0, "invalid 模式不写 rollup");

    let outcome = store
        .run_retention_if_due(now, RetentionPolicy::default())
        .unwrap();

    // raw 被裁剪，state 保持 invalid，不重建任何 rollup 桶。
    assert!(outcome.ran);
    assert_eq!(outcome.deleted_observations, 1);
    assert_eq!(observation_count(&db_path), 1);
    assert_eq!(
        state_value(&db_path, "last_success_at"),
        Some(now.to_rfc3339())
    );
    assert_eq!(
        rollup_metadata_value(&db_path, "state"),
        Some("invalid".to_string())
    );
    assert_eq!(rollup_row_count(&db_path), 0);
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
fn profile_trend_today_uses_24_hour_buckets_and_future_nulls() {
    let dir = tempdir().unwrap();
    let store = UsageStore::open(&dir.path().join("token-fire.sqlite")).unwrap();
    let now_local = Local
        .with_ymd_and_hms(2026, 7, 4, 12, 30, 0)
        .single()
        .unwrap();
    let now_utc = now_local.with_timezone(&Utc);

    insert_tracked(
        &store,
        &cost_observation(
            "trend-today-early",
            Some("gpt-5.5"),
            0,
            0,
            0,
            0,
            0,
            2_000_000,
            now_local
                .date_naive()
                .and_hms_opt(2, 10, 0)
                .unwrap()
                .and_local_timezone(Local)
                .single()
                .unwrap()
                .with_timezone(&Utc),
        ),
    )
    .unwrap();
    insert_tracked(
        &store,
        &cost_observation(
            "trend-today-current",
            Some("gpt-5.5"),
            0,
            0,
            0,
            0,
            0,
            3_000_000,
            now_local
                .date_naive()
                .and_hms_opt(12, 5, 0)
                .unwrap()
                .and_local_timezone(Local)
                .single()
                .unwrap()
                .with_timezone(&Utc),
        ),
    )
    .unwrap();

    let summary = store
        .profile_summary_at(ProfilePeriod::Today, now_utc, now_local)
        .unwrap();
    let trend = &summary.selected_period.trend;

    assert_eq!(trend.unit, token_fire::core::profile::PeriodTrendUnit::Hour);
    assert_eq!(trend.buckets.len(), 24);
    assert_eq!(
        trend
            .x_ticks
            .iter()
            .map(|tick| tick.label.as_str())
            .collect::<Vec<_>>(),
        vec!["0", "6", "12", "18", "23"]
    );
    assert_eq!(trend.buckets[2].total_tokens, Some(2_000_000));
    assert_eq!(trend.buckets[3].total_tokens, Some(0));
    assert_eq!(trend.buckets[12].total_tokens, Some(3_000_000));
    assert_eq!(trend.buckets[13].total_tokens, None);
    assert!(trend.buckets[13].is_future);
    assert_eq!(summary.selected_period.ended_at, now_utc);
    assert_eq!(
        trend
            .buckets
            .last()
            .unwrap()
            .ended_at
            .with_timezone(&Local)
            .date_naive(),
        now_local.date_naive() + chrono::Days::new(1)
    );
    assert_eq!(
        trend
            .buckets
            .iter()
            .filter_map(|bucket| bucket.total_tokens)
            .sum::<i64>(),
        summary.selected_period.total_tokens
    );
}

#[test]
fn profile_trend_week_month_and_year_use_calendar_ticks() {
    let dir = tempdir().unwrap();
    let store = UsageStore::open(&dir.path().join("token-fire.sqlite")).unwrap();
    let now_local = Local
        .with_ymd_and_hms(2026, 7, 8, 12, 0, 0)
        .single()
        .unwrap();
    let now_utc = now_local.with_timezone(&Utc);

    let week = store
        .profile_summary_at(ProfilePeriod::ThisWeek, now_utc, now_local)
        .unwrap();
    let month = store
        .profile_summary_at(ProfilePeriod::ThisMonth, now_utc, now_local)
        .unwrap();
    let year = store
        .profile_summary_at(ProfilePeriod::ThisYear, now_utc, now_local)
        .unwrap();

    assert_eq!(week.selected_period.trend.buckets.len(), 7);
    assert_eq!(week.selected_period.ended_at, now_utc);
    assert_eq!(
        week.selected_period
            .trend
            .x_ticks
            .iter()
            .map(|tick| tick.label.as_str())
            .collect::<Vec<_>>(),
        vec!["一", "二", "三", "四", "五", "六", "日"]
    );
    assert_eq!(week.selected_period.trend.buckets[0].total_tokens, Some(0));
    assert_eq!(week.selected_period.trend.buckets[3].total_tokens, None);
    assert_eq!(
        week.selected_period
            .trend
            .buckets
            .last()
            .unwrap()
            .ended_at
            .with_timezone(&Local)
            .weekday(),
        chrono::Weekday::Mon
    );

    assert_eq!(month.selected_period.trend.buckets.len(), 31);
    assert_eq!(
        month
            .selected_period
            .trend
            .x_ticks
            .iter()
            .map(|tick| tick.label.as_str())
            .collect::<Vec<_>>(),
        vec!["1", "10", "20", "月末"]
    );
    assert_eq!(month.selected_period.trend.buckets[7].total_tokens, Some(0));
    assert_eq!(month.selected_period.trend.buckets[8].total_tokens, None);
    assert_eq!(
        month
            .selected_period
            .trend
            .buckets
            .last()
            .unwrap()
            .ended_at
            .with_timezone(&Local)
            .day(),
        1
    );

    assert_eq!(year.selected_period.trend.buckets.len(), 12);
    assert_eq!(
        year.selected_period
            .trend
            .x_ticks
            .iter()
            .map(|tick| tick.label.as_str())
            .collect::<Vec<_>>(),
        vec!["1", "4", "7", "10", "12月"]
    );
    assert_eq!(year.selected_period.trend.buckets[6].total_tokens, Some(0));
    assert_eq!(year.selected_period.trend.buckets[7].total_tokens, None);
    assert_eq!(
        year.selected_period
            .trend
            .buckets
            .last()
            .unwrap()
            .ended_at
            .with_timezone(&Local)
            .month(),
        1
    );
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
