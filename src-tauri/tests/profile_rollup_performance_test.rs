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
#[allow(dead_code)]
const REBUILD_LIMIT: Duration = Duration::from_secs(5);
/// 冷查询门槛（Task 9 使用）。
#[allow(dead_code)]
const COLD_QUERY_LIMIT: Duration = Duration::from_secs(1);
/// 热查询门槛（Task 9 使用）。
#[allow(dead_code)]
const WARM_QUERY_LIMIT: Duration = Duration::from_millis(500);
/// tracked ingest 相对 untracked 的最大开销比（Task 9 使用）。
#[allow(dead_code)]
const MAX_INGEST_OVERHEAD_RATIO: f64 = 1.35;

/// 每条 observation 之间的固定间隔；一百万条 * 30s 约覆盖 347 天，全部落入
/// 一年 heatmap 窗口内，从而强制 baseline 查询做整年扫描。
const OBSERVATION_INTERVAL_SECONDS: i64 = 30;

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
#[allow(dead_code)]
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
    /// 构建 fixture：写入 `observation_count` 条 tracked raw observation，全部绑定到
    /// 同一个覆盖整段时间范围的 tracking window。
    pub fn create(observation_count: usize) -> anyhow::Result<Self> {
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
                let source = SOURCES[index % SOURCES.len()];
                let model = MODELS[index % MODELS.len()];
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
