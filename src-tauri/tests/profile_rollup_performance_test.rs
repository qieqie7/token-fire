//! Profile rollup 性能基准套件。
//!
//! 本文件构建一个 deterministic 的百万行 raw-observation fixture，并提供：
//! - 一个 release-only 的 raw baseline 基准（记录当前 raw 扫描一年数据的延迟）；
//! - 后续 Task 复用的计时/门槛常量与 `measured` helper。
//!
//! 约束：fixture 只写 raw `token_observations`（唯一事实源），数据准备使用单一
//! SQLite 事务 + 单个 prepared statement，并且不计入被测查询路径。

use std::path::PathBuf;
use std::time::{Duration, Instant};

use chrono::{TimeZone, Utc};
use rusqlite::{params, Connection};
use tempfile::TempDir;
use token_fire::core::profile::ProfilePeriod;
use token_fire::core::usage_store::UsageStore;

/// 固定为一百万行，不得下调：目的就是度量一年规模 raw 扫描的真实延迟。
const OBSERVATION_COUNT: usize = 1_000_000;
/// rollup 原子重建门槛（Task 9 使用）。
const REBUILD_LIMIT: Duration = Duration::from_secs(5);
/// 冷查询门槛（Task 9 使用）。
const COLD_QUERY_LIMIT: Duration = Duration::from_secs(1);
/// 热查询门槛（Task 9 使用）。
const WARM_QUERY_LIMIT: Duration = Duration::from_millis(500);
/// tracked ingest 相对 untracked 的最大开销比（Task 9 使用）。
const MAX_INGEST_OVERHEAD_RATIO: f64 = 1.35;

/// 每条 observation 之间的固定间隔；一百万条 * 30s 约覆盖 347 天，全部落入
/// 一年 heatmap 窗口内，从而强制 baseline 查询做整年扫描。
const OBSERVATION_INTERVAL_SECONDS: i64 = 30;

/// source/model 的分布模式。
/// - `Realistic`：source/model 按 session 连续驻留（每 SESSION_LEN 条切换一次），
///   贴近真实使用，rollup 高度压缩——常规延迟门槛针对此分布。
/// - `HighCardinality`：逐 observation 轮转 source/model，制造近乎无压缩的退化 rollup，
///   仅用于给出退化曲线，不套用常规延迟门槛（spec 明确要求区分二者）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixtureDistribution {
    Realistic,
    HighCardinality,
}

/// Realistic 模式下每个 session 驻留的 observation 条数（与 session_id 分组一致）。
const SESSION_LEN: usize = 1_000;

/// 按分布模式与行号选出 (source, model)。
fn source_model_for_row(
    distribution: FixtureDistribution,
    index: usize,
) -> (&'static str, Option<&'static str>) {
    match distribution {
        // 连续驻留：整段 session 用同一 source/model（真实会话只用一个模型）。
        FixtureDistribution::Realistic => {
            let session = index / SESSION_LEN;
            (
                SOURCES[session % SOURCES.len()],
                MODELS[session % MODELS.len()],
            )
        }
        // 高基数退化：逐条轮转，几乎每条一个新 (bucket, source, model) key。
        FixtureDistribution::HighCardinality => {
            (SOURCES[index % SOURCES.len()], MODELS[index % MODELS.len()])
        }
    }
}

/// 4 个 source，覆盖已知 source_label 映射。
const SOURCES: [&str; 4] = ["traex", "codex", "claude", "cursor"];

/// 12 个 model：含 USD/CNY 规则定价、fallback 定价与一个 None（unknown）。
const MODELS: [Option<&str>; 12] = [
    Some("gpt-5.5"),
    Some("gpt-5.4"),
    Some("gpt-5.4-mini"),
    Some("gpt-5"),
    Some("claude-sonnet-5"),
    Some("claude-opus-4-8"),
    Some("claude-haiku-4-5"),
    Some("gemini-2.5-flash"),
    Some("deepseek-v4-flash"),
    Some("qwen-max"),
    Some("unknown-internal-model"),
    None,
];

/// 度量一个 fallible 操作的耗时；只包裹被测路径，数据准备不进入这里。
fn measured<T>(operation: impl FnOnce() -> anyhow::Result<T>) -> anyhow::Result<(T, Duration)> {
    let started = Instant::now();
    let value = operation()?;
    Ok((value, started.elapsed()))
}

/// 一次生成的 raw observation 字段元组：
/// (input, output, cached, cache_creation, reasoning, total)。
type TokenFields = (i64, i64, i64, i64, i64, i64);

/// 按行号确定性地生成 token 字段分布：
/// - 每 97 条：unattributed（仅 total_tokens，其余分量为 0），验证 fallback 均价路径；
/// - 每 31 条：cached/cache-creation clamp 边界（input < cached + cache_creation）；
/// - 其余：普通带分量的 observation。
/// unattributed 优先级高于 clamp（二者语义互斥）。
fn token_fields_for_row(index: usize) -> TokenFields {
    let seed = index as i64;
    if index % 97 == 0 {
        // 无法归因：只有总量，component 全 0，走 DEFAULT_AVERAGE 均价定价。
        let total = 1_000 + (seed % 500);
        (0, 0, 0, 0, 0, total)
    } else if index % 31 == 0 {
        // clamp 边界：cached + cache_creation 超过 input，billable uncached input 应为 0。
        let output = 40 + (seed % 60);
        let reasoning = seed % 20;
        let total = 10 + output + reasoning;
        (10, output, 8, 7, reasoning, total)
    } else {
        let input = 100 + (seed % 900);
        let output = 50 + (seed % 450);
        let cached = seed % 40;
        let cache_creation = seed % 25;
        let reasoning = seed % 30;
        // billable input + output + reasoning；cached/creation 视为 input 子集，不叠加进 total。
        let total = input + output + reasoning;
        (input, output, cached, cache_creation, reasoning, total)
    }
}

/// 覆盖完整时间范围的百万行 raw fixture。持有 TempDir 以保证数据库文件在使用
/// 期间不被回收。
pub struct MillionRowFixture {
    _dir: TempDir,
    pub database: PathBuf,
    pub now_utc: chrono::DateTime<Utc>,
}

impl MillionRowFixture {
    /// 以 Realistic 分布构建 fixture（常规延迟门槛用）。
    pub fn create(observation_count: usize) -> anyhow::Result<Self> {
        Self::create_with_distribution(observation_count, FixtureDistribution::Realistic)
    }

    /// 构建 fixture：写入 `observation_count` 条 tracked raw observation，全部绑定到
    /// 同一个覆盖整段时间范围的 tracking window。`distribution` 决定 source/model 的分布。
    pub fn create_with_distribution(
        observation_count: usize,
        distribution: FixtureDistribution,
    ) -> anyhow::Result<Self> {
        let dir = TempDir::new()?;
        let database = dir.path().join("token-fire-perf.sqlite");

        // 先用生产 migrate 建立 schema（migrate 幂等），保证列定义与索引一致。
        UsageStore::open(&database)?;

        // 固定 now，使 fixture 与 wall clock 解耦；最新一条落在 now 之前 30s。
        let now_utc = Utc
            .with_ymd_and_hms(2026, 7, 11, 12, 0, 0)
            .single()
            .ok_or_else(|| anyhow::anyhow!("invalid fixture now_utc"))?;
        let span_seconds = observation_count as i64 * OBSERVATION_INTERVAL_SECONDS;
        let first_observation = now_utc - chrono::Duration::seconds(span_seconds);
        // tracking window 起点早于最早 observation，ended_at 为 NULL 覆盖到未来。
        let window_start = first_observation - chrono::Duration::minutes(1);

        let mut conn = Connection::open(&database)?;
        // 数据准备使用非持久 PRAGMA 加速批量写入；这段不计入被测路径。
        conn.pragma_update(None, "journal_mode", "MEMORY")?;
        conn.pragma_update(None, "synchronous", "OFF")?;

        conn.execute(
            "insert into tracking_windows (started_at) values (?1)",
            params![window_start.to_rfc3339()],
        )?;
        let window_id = conn.last_insert_rowid();

        // 单事务 + 单 prepared statement：避免逐行事务开销污染 fixture 构建时间。
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                r#"
                insert into token_observations (
                  tracking_window_id, source, adapter_version, source_record_id, source_record_id_confidence,
                  session_id, turn_id, turn_boundary_id, source_path, line_no, byte_offset,
                  input_tokens, output_tokens, cached_input_tokens, cache_creation_input_tokens,
                  reasoning_output_tokens, total_tokens, cumulative_total_tokens, model, cwd,
                  observed_at, token_payload_hash, dedupe_key
                ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23)
                "#,
            )?;
            for index in 0..observation_count {
                let (input, output, cached, cache_creation, reasoning, total) =
                    token_fields_for_row(index);
                let observed_at = first_observation
                    + chrono::Duration::seconds(index as i64 * OBSERVATION_INTERVAL_SECONDS);
                let (source, model) = source_model_for_row(distribution, index);
                // 稳定且唯一的行标识，从行号派生，保证 dedupe_key 不冲突。
                let source_record_id = format!("perf-{index:07}");
                let token_payload_hash = format!("payload-{index:07}");
                let dedupe_key = format!("perf-dedupe-{index:07}");
                let session_id = format!("session-{}", index / 1_000);
                let turn_id = format!("turn-{index:07}");
                let source_path = format!("/tmp/perf/{}.jsonl", index % SOURCES.len());

                stmt.execute(params![
                    window_id,
                    source,
                    "perf-fixture-v1",
                    source_record_id,
                    "exact",
                    session_id,
                    turn_id,
                    turn_id,
                    source_path,
                    index as i64,
                    index as i64 * 10,
                    input,
                    output,
                    cached,
                    cache_creation,
                    reasoning,
                    total,
                    Option::<i64>::None,
                    model,
                    "~/perf",
                    observed_at.to_rfc3339(),
                    token_payload_hash,
                    dedupe_key,
                ])?;
            }
        }
        tx.commit()?;

        Ok(Self {
            _dir: dir,
            database,
            now_utc,
        })
    }
}

#[test]
#[ignore = "release-only million-row benchmark"]
fn profile_raw_baseline_reports_one_million_row_latency() -> anyhow::Result<()> {
    let fixture = MillionRowFixture::create(OBSERVATION_COUNT)?;
    let store = UsageStore::open(&fixture.database)?;
    let now_utc = fixture.now_utc;
    let now_local = now_utc.with_timezone(&chrono::Local);
    // 仅度量 raw profile 查询本身，fixture 构建与 open 都在计时之外。
    let (_, elapsed) =
        measured(|| store.profile_summary_at(ProfilePeriod::ThisYear, now_utc, now_local))?;
    eprintln!(
        "profile_raw_baseline_rows={OBSERVATION_COUNT} elapsed_ms={}",
        elapsed.as_millis()
    );
    Ok(())
}

/// release-only 性能验收：在同一百万行 fixture 上校验
/// rebuild < 5s、fresh-connection 冷查询 < 1s、warm 查询 < 500ms，且计时前断言走 Rollup 路径。
#[test]
#[ignore = "release-only million-row benchmark"]
fn profile_rollup_meets_rebuild_and_query_limits() -> anyhow::Result<()> {
    use token_fire::core::usage_store::ProfileQuerySource;

    let fixture = MillionRowFixture::create(OBSERVATION_COUNT)?;
    let now_utc = fixture.now_utc;
    let now_local = now_utc.with_timezone(&chrono::Local);

    // rebuild：从百万行 raw 原子重建整张 rollup。
    let mut store = UsageStore::open(&fixture.database)?;
    let (rebuild_outcome, rebuild_elapsed) = measured(|| store.rebuild_profile_rollups())?;
    assert!(
        rebuild_outcome.rebuilt && rebuild_outcome.rollup_row_count > 0,
        "rebuild 应产生 rollup 行"
    );
    assert!(
        rebuild_elapsed < REBUILD_LIMIT,
        "rebuild took {rebuild_elapsed:?} (limit {REBUILD_LIMIT:?})"
    );
    drop(store);

    // 冷查询：fresh connection（drop/reopen；OS page cache 仍可能 warm，故只称 fresh-connection）。
    let cold_store = UsageStore::open(&fixture.database)?;
    // 计时前先断言就绪且实际走 Rollup（避免误把 raw fallback 当成 rollup 性能）。
    let ready_probe = cold_store.profile_summary_with_diagnostics_at(
        ProfilePeriod::ThisYear,
        now_utc,
        now_local,
    )?;
    assert_eq!(
        ready_probe.source,
        ProfileQuerySource::Rollup,
        "性能计时前必须确认走 Rollup 路径"
    );
    assert!(ready_probe.rollup_row_count > 0, "rollup 行数应 > 0");

    drop(cold_store);
    let cold_store = UsageStore::open(&fixture.database)?;
    let (_, cold_elapsed) =
        measured(|| cold_store.profile_summary_at(ProfilePeriod::ThisYear, now_utc, now_local))?;
    assert!(
        cold_elapsed < COLD_QUERY_LIMIT,
        "cold query took {cold_elapsed:?} (limit {COLD_QUERY_LIMIT:?})"
    );

    // warm 查询：同一 connection 再查一次。
    let (_, warm_elapsed) =
        measured(|| cold_store.profile_summary_at(ProfilePeriod::ThisYear, now_utc, now_local))?;
    assert!(
        warm_elapsed < WARM_QUERY_LIMIT,
        "warm query took {warm_elapsed:?} (limit {WARM_QUERY_LIMIT:?})"
    );

    eprintln!(
        "profile_rollup_perf rollup_rows={} rebuild_ms={} cold_query_ms={} warm_query_ms={}",
        rebuild_outcome.rollup_row_count,
        rebuild_elapsed.as_millis(),
        cold_elapsed.as_millis(),
        warm_elapsed.as_millis()
    );
    Ok(())
}

/// release-only：rollup 双写相对“同 tracking window 的 raw-only 旧基线”的写入开销比。
///
/// 按 spec“新实现必须使用相同 tracking window、transaction 粒度、PRAGMA、fixture 和预热状态…
/// raw+rollup writer 不得超过对应旧 baseline 的 1.35x”：两侧都走 `insert_observation_for_tracking_window`
/// （相同 window、相同 validate_tracking_window + 事务粒度），基线侧预置 `state=invalid` 走
/// raw-only 降级（只跳过 rollup upsert）。这样比值只隔离 rollup 写入本身的开销，而不把
/// tracked 路径固有的 window 校验成本计入分母（untracked 路径会跳过该校验，属不公平基线）。
/// release-only：rollup 双写相对“同 tracking window 的 raw-only 旧基线”的**写入 p95** 开销比。
///
/// 按 spec“新实现必须使用相同 tracking window、transaction 粒度、PRAGMA、fixture 和预热状态…
/// raw+rollup writer p95 不得超过对应旧 baseline 的 1.35x”：两侧都走
/// `insert_observation_for_tracking_window`（相同 window、相同 validate_tracking_window + 事务粒度），
/// 基线侧预置 `state=invalid` 走 raw-only 降级（只跳过 rollup upsert）。这样比值只隔离 rollup
/// 写入本身的开销。断言的是**逐条写入延迟的 p95 之比**（spec 绑定统计量），而非批量吞吐比或
/// median——批量比在 n 很小时无法表达尾延迟。每路预热后采 WRITE_COUNT 条独立延迟样本。
#[test]
#[ignore = "release-only million-row benchmark"]
fn profile_rollup_tracked_ingest_overhead_within_ratio() -> anyhow::Result<()> {
    use rusqlite::Connection as RawConnection;
    use token_fire::core::observation::{NormalizedObservation, SourceRecordIdConfidence};

    // 采样量放大到 20,000，使 p95 稳定（避免个别冷样本主导）。
    const WRITE_COUNT: usize = 20_000;
    const WARMUP: usize = 2_000;

    fn make_obs(prefix: &str, index: usize, base: chrono::DateTime<Utc>) -> NormalizedObservation {
        let seed = index as i64;
        NormalizedObservation {
            source: SOURCES[index % SOURCES.len()].to_string(),
            adapter_version: "perf-ingest-v1".to_string(),
            source_record_id: format!("{prefix}-{index:07}"),
            source_record_id_confidence: SourceRecordIdConfidence::Exact,
            session_id: Some(format!("{prefix}-session-{}", index / 500)),
            turn_id: Some(format!("{prefix}-turn-{index:07}")),
            turn_boundary_id: Some(format!("{prefix}-turn-{index:07}")),
            source_path: Some(format!("/tmp/perf-ingest/{prefix}.jsonl")),
            line_no: Some(index as i64),
            byte_offset: Some(index as i64 * 10),
            input_tokens: 100 + (seed % 900),
            output_tokens: 50 + (seed % 450),
            cached_input_tokens: seed % 40,
            cache_creation_input_tokens: seed % 25,
            reasoning_output_tokens: seed % 30,
            total_tokens: 150 + (seed % 1_300),
            cumulative_total_tokens: None,
            model: MODELS[index % MODELS.len()].map(str::to_string),
            cwd: Some("~/perf-ingest".to_string()),
            // 每条间隔 1s，落在 window 内。
            observed_at: base + chrono::Duration::seconds(index as i64),
            token_payload_hash: format!("{prefix}-hash-{index:07}"),
        }
    }

    /// 逐条写入并收集每次写入的纳秒延迟；返回排序后的样本。预热样本不计入。
    fn per_write_latencies_ns(
        store: &UsageStore,
        window: i64,
        prefix: &str,
        base: chrono::DateTime<Utc>,
    ) -> anyhow::Result<Vec<u128>> {
        for index in 0..WARMUP {
            store.insert_observation_for_tracking_window(
                &make_obs(&format!("warm-{prefix}"), index, base),
                window,
            )?;
        }
        let mut samples = Vec::with_capacity(WRITE_COUNT);
        for index in 0..WRITE_COUNT {
            let obs = make_obs(prefix, index, base);
            let started = Instant::now();
            store.insert_observation_for_tracking_window(&obs, window)?;
            samples.push(started.elapsed().as_nanos());
        }
        samples.sort_unstable();
        Ok(samples)
    }

    /// 排序样本的分位数（nearest-rank）。
    fn percentile(sorted: &[u128], q: f64) -> u128 {
        if sorted.is_empty() {
            return 0;
        }
        let rank = ((q * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len());
        sorted[rank - 1]
    }

    let base = Utc.with_ymd_and_hms(2026, 7, 11, 0, 0, 0).unwrap();

    // 旧基线：tracked 但 rollup state=invalid → raw-only 降级（同 window/校验/事务，仅跳过 upsert）。
    let baseline_dir = TempDir::new()?;
    let baseline_db = baseline_dir.path().join("baseline.sqlite");
    {
        UsageStore::open(&baseline_db)?; // 建表
        RawConnection::open(&baseline_db)?.execute(
            "insert into usage_rollup_metadata (key, value) values ('state', 'invalid')",
            [],
        )?;
    }
    let baseline_store = UsageStore::open(&baseline_db)?;
    let baseline_window =
        baseline_store.open_tracking_window(base - chrono::Duration::minutes(1))?;
    let baseline_samples = per_write_latencies_ns(&baseline_store, baseline_window, "b", base)?;

    // 新实现：tracked 且 rollup 活跃（absent-state → 双写）。
    let rollup_dir = TempDir::new()?;
    let rollup_db = rollup_dir.path().join("rollup.sqlite");
    let rollup_store = UsageStore::open(&rollup_db)?;
    let rollup_window = rollup_store.open_tracking_window(base - chrono::Duration::minutes(1))?;
    let rollup_samples = per_write_latencies_ns(&rollup_store, rollup_window, "r", base)?;

    let baseline_p95 = percentile(&baseline_samples, 0.95).max(1);
    let rollup_p95 = percentile(&rollup_samples, 0.95);
    let baseline_median = percentile(&baseline_samples, 0.50).max(1);
    let rollup_median = percentile(&rollup_samples, 0.50);
    let baseline_max = *baseline_samples.last().unwrap_or(&0);
    let rollup_max = *rollup_samples.last().unwrap_or(&0);

    // spec 绑定统计量：writer p95 之比。同时打印 median/max 供复核（spec“报告 median/p95/max”）。
    let p95_ratio = rollup_p95 as f64 / baseline_p95 as f64;
    let median_ratio = rollup_median as f64 / baseline_median as f64;
    eprintln!(
        "profile_rollup_ingest n={WRITE_COUNT} \
         baseline_p95_us={} rollup_p95_us={} p95_ratio={p95_ratio:.3} \
         baseline_median_us={} rollup_median_us={} median_ratio={median_ratio:.3} \
         baseline_max_us={} rollup_max_us={}",
        baseline_p95 / 1000,
        rollup_p95 / 1000,
        baseline_median / 1000,
        rollup_median / 1000,
        baseline_max / 1000,
        rollup_max / 1000,
    );
    assert!(
        p95_ratio <= MAX_INGEST_OVERHEAD_RATIO,
        "tracked raw+rollup writer p95 ratio {p95_ratio:.3} exceeds {MAX_INGEST_OVERHEAD_RATIO} \
         (baseline = same tracking window, rollup state=invalid raw-only degraded; \
         baseline_p95_us={} rollup_p95_us={})",
        baseline_p95 / 1000,
        rollup_p95 / 1000,
    );
    Ok(())
}

/// release-only（无硬延迟门槛）：高基数退化 fixture 的重建与查询画像。
/// source/model 逐条轮转，rollup 几乎不压缩；仅打印画像供适用边界评审，不套用 5s/1s/500ms。
#[test]
#[ignore = "release-only million-row benchmark"]
fn profile_rollup_high_cardinality_degradation_profile() -> anyhow::Result<()> {
    use token_fire::core::usage_store::ProfileQuerySource;

    let fixture = MillionRowFixture::create_with_distribution(
        OBSERVATION_COUNT,
        FixtureDistribution::HighCardinality,
    )?;
    let now_utc = fixture.now_utc;
    let now_local = now_utc.with_timezone(&chrono::Local);

    let mut store = UsageStore::open(&fixture.database)?;
    let (rebuild_outcome, rebuild_elapsed) = measured(|| store.rebuild_profile_rollups())?;
    let outcome =
        store.profile_summary_with_diagnostics_at(ProfilePeriod::ThisYear, now_utc, now_local)?;
    assert_eq!(outcome.source, ProfileQuerySource::Rollup);
    let (_, query_elapsed) =
        measured(|| store.profile_summary_at(ProfilePeriod::ThisYear, now_utc, now_local))?;
    eprintln!(
        "profile_rollup_high_cardinality rollup_rows={} rebuild_ms={} query_ms={}",
        rebuild_outcome.rollup_row_count,
        rebuild_elapsed.as_millis(),
        query_elapsed.as_millis()
    );
    Ok(())
}
