use std::{collections::BTreeMap, path::Path};

use chrono::{DateTime, Duration, Local, Utc};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

use crate::core::day_boundary::local_day_bounds;
use crate::core::dedupe::compute_dedupe_key;
use crate::core::observation::{
    validate_observation, NormalizedObservation, SourceRecordIdConfidence,
};
use crate::core::pricing::{
    combine_pricing_status, estimate_model_cost, estimate_model_cost_breakdown, CostPeriodSummary,
    ModelTokenUsage, WidgetCostSummary,
};
use crate::core::profile::{
    empty_period_usage_trend, intensity_for_cost, model_label, period_bounds, source_label,
    PeriodProfileSummary, ProfileCostDrivers, ProfileDayBucket, ProfilePeakDay, ProfilePeriod,
    ProfileSummary, RankedProfileBreakdown, YearProfileSummary,
};
use crate::core::profile_rollup::{
    bucket_start_utc, normalize_model_key, PROFILE_ROLLUP_BUCKET_SECONDS,
    PROFILE_ROLLUP_SCHEMA_VERSION,
};
use crate::core::usage_series::{
    average_tokens_per_bucket, bucket_index_for, empty_usage_buckets, WidgetUsageSeries,
    WIDGET_USAGE_ACTIVE_THRESHOLD_MINUTES, WIDGET_USAGE_WINDOW_MINUTES,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertOutcome {
    Inserted,
    Duplicate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackingWindow {
    pub id: i64,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
}

pub const DEFAULT_RETENTION_DAYS: i64 = 365;
pub const DEFAULT_RETENTION_MIN_INTERVAL_HOURS: i64 = 24;

/// rollup readiness 状态机的 metadata key 与取值。migration 不预写这些；
/// 完整 rebuild + 翻转 ready + 写 schema_version 由 Task 5 拥有。
const ROLLUP_METADATA_STATE_KEY: &str = "state";
const ROLLUP_METADATA_SCHEMA_VERSION_KEY: &str = "schema_version";
const ROLLUP_STATE_INVALID: &str = "invalid";
const ROLLUP_STATE_READY: &str = "ready";
/// rebuild 用的 shadow 表名；rebuild 结束后 rename 为正式表，任何残留在 rebuild 起始处清理。
const ROLLUP_SHADOW_TABLE: &str = "usage_rollups_15m_rebuild";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetentionPolicy {
    pub observation_retention_days: i64,
    pub min_interval_hours: i64,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            observation_retention_days: DEFAULT_RETENTION_DAYS,
            min_interval_hours: DEFAULT_RETENTION_MIN_INTERVAL_HOURS,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetentionSkipReason {
    RecentlySucceeded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetentionOutcome {
    pub ran: bool,
    pub cutoff: DateTime<Utc>,
    pub deleted_observations: usize,
    pub skipped_reason: Option<RetentionSkipReason>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetentionDiagnostics {
    pub policy_days: i64,
    pub min_interval_hours: i64,
    pub last_success_at: Option<String>,
    pub last_deleted_observations: Option<usize>,
    pub last_failure_at: Option<String>,
    pub last_error_kind: Option<String>,
}

/// Profile rollup 读模型的就绪状态。
/// `Ready` 表示可直接读 rollup；`RebuildRequired` 携带稳定 reason 字符串供诊断日志分类。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileRollupStatus {
    Ready { schema_version: String },
    RebuildRequired { reason: &'static str },
}

/// rebuild / ensure 的结果：是否真正重建、当前 rollup 行数、写入的 schema_version。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileRollupRebuildOutcome {
    pub rebuilt: bool,
    pub rollup_row_count: usize,
    pub schema_version: String,
}

pub struct UsageStore {
    conn: Connection,
}

/// 只读诊断快照：暴露每个 connection 显式设置的 SQLite PRAGMA，供并发正确性验证与诊断日志使用。
/// 不含数据库路径或业务数据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionPragmas {
    pub journal_mode: String,
    pub synchronous: i64,
    pub busy_timeout: i64,
    pub wal_autocheckpoint: i64,
}

#[derive(Debug, Clone)]
struct ProfileObservationRow {
    source: String,
    model: Option<String>,
    input_tokens: i64,
    output_tokens: i64,
    cached_input_tokens: i64,
    cache_creation_input_tokens: i64,
    reasoning_output_tokens: i64,
    total_tokens: i64,
    observed_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
struct PricedProfileObservationRow {
    source: String,
    model: Option<String>,
    estimated_cost: f64,
    total_tokens: i64,
    drivers: crate::core::pricing::PricedCostDrivers,
    observed_at: DateTime<Utc>,
}

impl UsageStore {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        let store = Self { conn };
        store.apply_connection_policy()?;
        store.migrate()?;
        Ok(store)
    }

    /// 显式设置每个 connection 的 SQLite 策略（OVERRIDE C / spec "SQLite Connection Policy"）。
    /// 单 writer 多 reader：WAL 让 Profile reader 与 ingest writer 并发；synchronous=FULL 保持
    /// 与旧默认相当的 durability，不以耐久性换吞吐；busy_timeout 是并发正确性而非润色。
    /// journal_mode 是数据库级持久属性，其余为 connection-local，故每次 open 都显式重设。
    fn apply_connection_policy(&self) -> anyhow::Result<()> {
        // journal_mode 返回结果集，必须用 query_row 消费；WAL 一经设置持久保存于数据库文件。
        let mode: String = self
            .conn
            .query_row("pragma journal_mode = WAL", [], |row| row.get(0))?;
        if !mode.eq_ignore_ascii_case("wal") {
            anyhow::bail!("failed to enable WAL journal mode, got: {mode}");
        }
        self.conn.execute_batch(
            r#"
            pragma synchronous = FULL;
            pragma busy_timeout = 5000;
            pragma wal_autocheckpoint = 1000;
            "#,
        )?;
        Ok(())
    }

    /// 读取当前 connection 的关键 PRAGMA，用于并发正确性验证与诊断日志。
    pub fn connection_pragmas(&self) -> anyhow::Result<ConnectionPragmas> {
        let journal_mode: String = self
            .conn
            .query_row("pragma journal_mode", [], |row| row.get(0))?;
        let synchronous: i64 = self
            .conn
            .query_row("pragma synchronous", [], |row| row.get(0))?;
        let busy_timeout: i64 = self
            .conn
            .query_row("pragma busy_timeout", [], |row| row.get(0))?;
        let wal_autocheckpoint: i64 =
            self.conn
                .query_row("pragma wal_autocheckpoint", [], |row| row.get(0))?;
        Ok(ConnectionPragmas {
            journal_mode,
            synchronous,
            busy_timeout,
            wal_autocheckpoint,
        })
    }

    fn migrate(&self) -> anyhow::Result<()> {
        self.conn.execute_batch(
            r#"
            create table if not exists token_observations (
              id integer primary key autoincrement,
              tracking_window_id integer,
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
            create unique index if not exists idx_token_observations_dedupe_key
              on token_observations(dedupe_key);
            create index if not exists idx_token_observations_observed_at
              on token_observations(observed_at);
            create index if not exists idx_token_observations_source_session
              on token_observations(source, session_id);
            create index if not exists idx_token_observations_source_turn_boundary
              on token_observations(source, turn_boundary_id);
            create table if not exists tracking_windows (
              id integer primary key autoincrement,
              started_at text not null,
              ended_at text
            );
            create table if not exists file_baselines (
              source_path text primary key,
              byte_offset integer not null,
              updated_at text not null default (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
            );
            create table if not exists retention_state (
              key text primary key,
              value text not null
            );
            -- 派生读模型：15 分钟 UTC 桶聚合。token/count 字段 CHECK 非负且必须是 integer，
            -- 避免 SUM 溢出被 SQLite 静默提升为 REAL 后写回。model=NULL 在 key 中规范化为 ''。
            create table if not exists usage_rollups_15m (
              bucket_start_utc integer not null,
              source text not null,
              model text not null,
              input_tokens integer not null default 0 check(input_tokens >= 0 and typeof(input_tokens) = 'integer'),
              billable_uncached_input_tokens integer not null default 0 check(billable_uncached_input_tokens >= 0 and typeof(billable_uncached_input_tokens) = 'integer'),
              output_tokens integer not null default 0 check(output_tokens >= 0 and typeof(output_tokens) = 'integer'),
              cached_input_tokens integer not null default 0 check(cached_input_tokens >= 0 and typeof(cached_input_tokens) = 'integer'),
              cache_creation_input_tokens integer not null default 0 check(cache_creation_input_tokens >= 0 and typeof(cache_creation_input_tokens) = 'integer'),
              reasoning_output_tokens integer not null default 0 check(reasoning_output_tokens >= 0 and typeof(reasoning_output_tokens) = 'integer'),
              unattributed_total_tokens integer not null default 0 check(unattributed_total_tokens >= 0 and typeof(unattributed_total_tokens) = 'integer'),
              total_tokens integer not null default 0 check(total_tokens >= 0 and typeof(total_tokens) = 'integer'),
              observation_count integer not null default 0 check(observation_count >= 0 and typeof(observation_count) = 'integer'),
              primary key (bucket_start_utc, source, model)
            );
            -- rollup 版本与 ready/invalid 状态元数据。migration 只建表，不预写 schema_version/state；
            -- 完整 rebuild 并翻转到 ready 由 Task 5 拥有。
            create table if not exists usage_rollup_metadata (
              key text primary key,
              value text not null
            );
            "#,
        )?;
        self.ensure_column("token_observations", "tracking_window_id", "integer")?;
        self.conn.execute(
            "create index if not exists idx_token_observations_tracking_window_id on token_observations(tracking_window_id)",
            [],
        )?;
        self.conn.execute(
            "create index if not exists idx_token_observations_tracked_observed_at on token_observations(observed_at) where tracking_window_id is not null",
            [],
        )?;
        Ok(())
    }

    fn ensure_column(&self, table: &str, column: &str, definition: &str) -> anyhow::Result<()> {
        let mut stmt = self.conn.prepare(&format!("pragma table_info({table})"))?;
        let columns = stmt.query_map([], |row| row.get::<_, String>(1))?;
        for existing in columns {
            if existing? == column {
                return Ok(());
            }
        }
        self.conn.execute(
            &format!("alter table {table} add column {column} {definition}"),
            [],
        )?;
        Ok(())
    }

    fn retention_state_value(&self, key: &str) -> anyhow::Result<Option<String>> {
        self.conn
            .query_row(
                "select value from retention_state where key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn retention_diagnostics(
        &self,
        policy: RetentionPolicy,
    ) -> anyhow::Result<RetentionDiagnostics> {
        let last_deleted_observations = self
            .retention_state_value("last_deleted_observations")?
            .and_then(|value| value.parse::<usize>().ok());
        Ok(RetentionDiagnostics {
            policy_days: policy.observation_retention_days,
            min_interval_hours: policy.min_interval_hours,
            last_success_at: self.retention_state_value("last_success_at")?,
            last_deleted_observations,
            last_failure_at: self.retention_state_value("last_failure_at")?,
            last_error_kind: self.retention_state_value("last_error_kind")?,
        })
    }

    pub fn record_retention_failure(
        &self,
        now: DateTime<Utc>,
        error_kind: &str,
    ) -> anyhow::Result<()> {
        self.conn.execute(
            r#"
            insert into retention_state (key, value)
            values ('last_failure_at', ?1)
            on conflict(key) do update set value = excluded.value
            "#,
            params![now.to_rfc3339()],
        )?;
        self.conn.execute(
            r#"
            insert into retention_state (key, value)
            values ('last_error_kind', ?1)
            on conflict(key) do update set value = excluded.value
            "#,
            params![error_kind],
        )?;
        Ok(())
    }

    pub fn run_retention_if_due(
        &mut self,
        now: DateTime<Utc>,
        policy: RetentionPolicy,
    ) -> anyhow::Result<RetentionOutcome> {
        let cutoff = now - Duration::days(policy.observation_retention_days);
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let last_success_at = tx
            .query_row(
                "select value from retention_state where key = 'last_success_at'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        if let Some(last_success_at) = last_success_at {
            let last_success_at =
                DateTime::parse_from_rfc3339(&last_success_at)?.with_timezone(&Utc);
            if now < last_success_at + Duration::hours(policy.min_interval_hours) {
                return Ok(RetentionOutcome {
                    ran: false,
                    cutoff,
                    deleted_observations: 0,
                    skipped_reason: Some(RetentionSkipReason::RecentlySucceeded),
                });
            }
        }

        // raw 删除用 `?` 传播：raw 是唯一事实源，其删除失败是真实存储故障，必须整体 abort，
        // 不推进 last_success_at（与 rollup 维护失败的降级路径严格区分）。
        let deleted_observations = tx.execute(
            "delete from token_observations where observed_at < ?1",
            params![cutoff.to_rfc3339()],
        )?;

        // Ready 模式在同事务内维护 rollup 边界桶；invalid 模式只维护 raw、保持 rollup invalid。
        if rollup_state(&tx)?.as_deref() != Some(ROLLUP_STATE_INVALID) {
            // OVERRIDE D：rollup 是派生模型，其边界维护失败不能丢失已删除的 raw retention。
            // 不用 `?` 传播——捕获错误后回滚组合事务，改走 raw-only 降级并翻转 invalid。
            if maintain_rollup_boundary(&tx, cutoff).is_err() {
                drop(tx);
                return self.run_retention_raw_only_degraded(now, cutoff);
            }
        }

        write_retention_metadata(&tx, now, deleted_observations)?;
        tx.commit()?;

        Ok(RetentionOutcome {
            ran: true,
            cutoff,
            deleted_observations,
            skipped_reason: None,
        })
    }

    /// OVERRIDE D 降级路径：rollup 边界重建失败、组合事务已回滚后调用。
    /// 新事务只做 raw retention 并把 rollup 标记 invalid（等待 Task 5 完整重建），
    /// 保证 raw 删除与 last_success_at 仍然持久化；旧 rollup 不再参与查询。
    fn run_retention_raw_only_degraded(
        &mut self,
        now: DateTime<Utc>,
        cutoff: DateTime<Utc>,
    ) -> anyhow::Result<RetentionOutcome> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let deleted_observations = tx.execute(
            "delete from token_observations where observed_at < ?1",
            params![cutoff.to_rfc3339()],
        )?;
        tx.execute(
            r#"
            insert into usage_rollup_metadata (key, value)
            values (?1, ?2)
            on conflict(key) do update set value = excluded.value
            "#,
            params![ROLLUP_METADATA_STATE_KEY, ROLLUP_STATE_INVALID],
        )?;
        write_retention_metadata(&tx, now, deleted_observations)?;
        tx.commit()?;

        Ok(RetentionOutcome {
            ran: true,
            cutoff,
            deleted_observations,
            skipped_reason: None,
        })
    }

    pub fn insert_untracked_observation_for_test(
        &self,
        observation: &NormalizedObservation,
    ) -> anyhow::Result<InsertOutcome> {
        self.insert_observation_inner(observation, None)
    }

    /// 启动/rebuild 决策用的 rollup 就绪检查。
    ///
    /// 仅在此处扫描 raw 做一次全局 token/count 守恒 checksum；普通 Profile query（Task 6）
    /// 只读 `state`/`schema_version`，绝不每次读都重扫 raw。
    /// 判定 Ready 需三者同时成立：state==ready、schema_version==当前版本、全局守恒通过。
    /// 否则返回稳定 reason（state_missing / state_invalid / version_mismatch / checksum_mismatch）。
    pub fn profile_rollup_status(&self) -> anyhow::Result<ProfileRollupStatus> {
        let state = self.rollup_metadata_value(ROLLUP_METADATA_STATE_KEY)?;
        match state.as_deref() {
            None => {
                return Ok(ProfileRollupStatus::RebuildRequired {
                    reason: "state_missing",
                })
            }
            Some(ROLLUP_STATE_READY) => {}
            Some(_) => {
                // invalid 或任何非 ready 取值都需 rebuild。
                return Ok(ProfileRollupStatus::RebuildRequired {
                    reason: "state_invalid",
                });
            }
        }

        let schema_version = self.rollup_metadata_value(ROLLUP_METADATA_SCHEMA_VERSION_KEY)?;
        if schema_version.as_deref() != Some(PROFILE_ROLLUP_SCHEMA_VERSION) {
            return Ok(ProfileRollupStatus::RebuildRequired {
                reason: "version_mismatch",
            });
        }

        // 全局守恒 checksum：raw 与 rollup 的 8 个 token 分量 + observation_count 逐项相等。
        // 这是诊断级别的全局校验；逐 key 正确性由“增量 upsert 与 rebuild 共用同一
        // 分组 + clamp 方言”保证，不能仅凭全局和相等推断逐 key 正确。
        if self.rollup_conservation_totals()? != self.raw_conservation_totals()? {
            return Ok(ProfileRollupStatus::RebuildRequired {
                reason: "checksum_mismatch",
            });
        }

        Ok(ProfileRollupStatus::Ready {
            schema_version: PROFILE_ROLLUP_SCHEMA_VERSION.to_string(),
        })
    }

    /// 启动维护入口：Ready 直接返回 rebuilt:false（不重复重建）；否则触发原子 rebuild。
    pub fn ensure_profile_rollup_ready(&mut self) -> anyhow::Result<ProfileRollupRebuildOutcome> {
        if let ProfileRollupStatus::Ready { schema_version } = self.profile_rollup_status()? {
            return Ok(ProfileRollupRebuildOutcome {
                rebuilt: false,
                rollup_row_count: self.rollup_row_count()? as usize,
                schema_version,
            });
        }
        self.rebuild_profile_rollups()
    }

    /// 用 shadow table 原子重建整张 rollup：从 raw 全量聚合 → 校验守恒 → drop+rename 换表。
    ///
    /// 两段事务，保证“崩溃只剩完整旧状态或完整新状态”：
    /// 1) 先在独立事务持久化 state=invalid（rebuild 进行中标记）。若中途崩溃 reopen 后仍是
    ///    invalid，增量双写停用、等待再次 rebuild，不会看到半成品 ready。
    /// 2) rebuild body 在单个 Immediate 事务内完成 shadow 建表/聚合/校验/drop/rename/写 ready+
    ///    version；任一步失败 → 事务 drop 回滚 → 正式表与旧 version 完全不变，state 停留 invalid。
    ///    换表（drop 正式表 + rename shadow）位于同一事务，绝不出现半换状态。
    pub fn rebuild_profile_rollups(&mut self) -> anyhow::Result<ProfileRollupRebuildOutcome> {
        // 第 1 段：独立事务先标记 invalid（durable 的“重建进行中”）。
        self.mark_rollup_invalid()?;

        // 第 2 段：原子 rebuild body。
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        rebuild_profile_rollups_in_tx(&tx)?;
        // 写 schema_version 与 ready 状态（与换表同事务，保证一致翻转）。
        upsert_rollup_metadata(
            &tx,
            ROLLUP_METADATA_SCHEMA_VERSION_KEY,
            PROFILE_ROLLUP_SCHEMA_VERSION,
        )?;
        upsert_rollup_metadata(&tx, ROLLUP_METADATA_STATE_KEY, ROLLUP_STATE_READY)?;
        let rollup_row_count: i64 =
            tx.query_row("select count(*) from usage_rollups_15m", [], |row| {
                row.get(0)
            })?;
        tx.commit()?;

        Ok(ProfileRollupRebuildOutcome {
            rebuilt: true,
            rollup_row_count: rollup_row_count as usize,
            schema_version: PROFILE_ROLLUP_SCHEMA_VERSION.to_string(),
        })
    }

    /// 独立事务把 rollup state 置为 invalid（rebuild 起始处调用）。
    fn mark_rollup_invalid(&mut self) -> anyhow::Result<()> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        upsert_rollup_metadata(&tx, ROLLUP_METADATA_STATE_KEY, ROLLUP_STATE_INVALID)?;
        tx.commit()?;
        Ok(())
    }

    /// 读取 rollup metadata 单值（缺表/缺行返回 None）。
    fn rollup_metadata_value(&self, key: &str) -> anyhow::Result<Option<String>> {
        self.conn
            .query_row(
                "select value from usage_rollup_metadata where key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
    }

    fn rollup_row_count(&self) -> anyhow::Result<i64> {
        self.conn
            .query_row("select count(*) from usage_rollups_15m", [], |row| {
                row.get(0)
            })
            .map_err(Into::into)
    }

    /// tracked raw 侧全局守恒聚合：与 rebuild/upsert 相同的 clamp/CASE 方言，
    /// 顺序 input/billable/output/cached/cache_creation/reasoning/unattributed/total/count。
    fn raw_conservation_totals(&self) -> anyhow::Result<[i64; 9]> {
        self.conn
            .query_row(ROLLUP_RAW_CONSERVATION_SQL, [], |row| {
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
            })
            .map_err(Into::into)
    }

    /// rollup 侧全局守恒聚合（预聚合列直接求和），列序与 raw_conservation_totals 对齐。
    fn rollup_conservation_totals(&self) -> anyhow::Result<[i64; 9]> {
        self.conn
            .query_row(ROLLUP_ROLLUP_CONSERVATION_SQL, [], |row| {
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
            })
            .map_err(Into::into)
    }

    pub fn insert_observation_for_tracking_window(
        &self,
        observation: &NormalizedObservation,
        tracking_window_id: i64,
    ) -> anyhow::Result<InsertOutcome> {
        self.insert_observation_inner(observation, Some(tracking_window_id))
    }

    fn insert_observation_inner(
        &self,
        observation: &NormalizedObservation,
        tracking_window_id: Option<i64>,
    ) -> anyhow::Result<InsertOutcome> {
        validate_observation(observation)?;
        if let Some(tracking_window_id) = tracking_window_id {
            self.validate_tracking_window(tracking_window_id, observation.observed_at)?;
        }

        // 原子双写：raw insert 与 rollup upsert 位于同一事务，任一步失败先整体回滚。
        // 用 unchecked_transaction 从 &self 取事务，避免为适配 IngestScheduler 改成 &mut self。
        let tx = self.conn.unchecked_transaction()?;
        let inserted = insert_raw_observation(&tx, observation, tracking_window_id)?;

        // dedupe 命中：raw 与 rollup 都不变，直接提交空事务。
        if inserted == 0 {
            tx.commit()?;
            return Ok(InsertOutcome::Duplicate);
        }

        // untracked（测试用）观测永不写 rollup。
        if tracking_window_id.is_none() {
            tx.commit()?;
            return Ok(InsertOutcome::Inserted);
        }

        // OVERRIDE B：state=invalid 即 raw-only 降级，跳过 rollup upsert 且不报错；
        // 缺 state（migration 未写）视为“可增量双写”，从首条 insert 起就增量维护 rollup。
        // 增量维护不要求 schema_version==current（版本 gating 只在 Task 5/6 管查询就绪）。
        if rollup_state(&tx)?.as_deref() == Some(ROLLUP_STATE_INVALID) {
            tx.commit()?;
            return Ok(InsertOutcome::Inserted);
        }

        match upsert_profile_rollup(&tx, observation) {
            Ok(()) => {
                tx.commit()?;
                Ok(InsertOutcome::Inserted)
            }
            // OVERRIDE A：rollup 是派生模型，其写入失败不能丢失唯一事实源 raw。
            Err(_rollup_error) => {
                // 1. 回滚原双写事务（此时 raw 尚未持久化）。
                drop(tx);
                // 2/3. 新事务只写 raw 并持久化 state=invalid，进入 raw-only 降级模式。
                //     沿用相同 dedupe key，重试幂等。若此事务也失败则按 storage failure 上报，
                //     不能声称已保存。
                self.write_raw_only_and_mark_invalid(observation, tracking_window_id)?;
                // 4. raw 确已持久化，返回 Inserted 而非错误。
                Ok(InsertOutcome::Inserted)
            }
        }
    }

    /// raw-only 降级写入：新事务内写 raw observation + 标记 rollup state=invalid。
    /// 仅在检测到 rollup 写失败后调用；沿用相同 dedupe key 保证幂等重试。
    fn write_raw_only_and_mark_invalid(
        &self,
        observation: &NormalizedObservation,
        tracking_window_id: Option<i64>,
    ) -> anyhow::Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        insert_raw_observation(&tx, observation, tracking_window_id)?;
        tx.execute(
            r#"
            insert into usage_rollup_metadata (key, value)
            values (?1, ?2)
            on conflict(key) do update set value = excluded.value
            "#,
            params![ROLLUP_METADATA_STATE_KEY, ROLLUP_STATE_INVALID],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn validate_tracking_window(
        &self,
        tracking_window_id: i64,
        observed_at: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        let window = self
            .conn
            .query_row(
                "select started_at, ended_at from tracking_windows where id = ?1",
                params![tracking_window_id],
                |row| {
                    let started_at: String = row.get(0)?;
                    let ended_at: Option<String> = row.get(1)?;
                    Ok((started_at, ended_at))
                },
            )
            .optional()?
            .ok_or_else(|| anyhow::anyhow!("tracking window not found: {tracking_window_id}"))?;
        let started_at = DateTime::parse_from_rfc3339(&window.0)?.with_timezone(&Utc);
        let ended_at = window
            .1
            .map(|value| {
                DateTime::parse_from_rfc3339(&value).map(|parsed| parsed.with_timezone(&Utc))
            })
            .transpose()?;
        if observed_at < started_at || ended_at.is_some_and(|ended_at| observed_at >= ended_at) {
            anyhow::bail!("observation outside tracking window: {tracking_window_id}");
        }
        Ok(())
    }

    pub fn today_total(&self, now: DateTime<Local>) -> anyhow::Result<i64> {
        let (start, next) = local_day_bounds(now);
        let total = self.conn.query_row(
            r#"
            select coalesce(sum(total_tokens), 0)
            from token_observations
            where tracking_window_id is not null
              and observed_at >= ?1 and observed_at < ?2
            "#,
            params![
                start.with_timezone(&Utc).to_rfc3339(),
                next.with_timezone(&Utc).to_rfc3339(),
            ],
            |row| row.get(0),
        )?;
        Ok(total)
    }

    pub fn latest_turn_delta(&self) -> anyhow::Result<i64> {
        let latest: Option<(String, Option<String>, Option<String>, Option<String>, i64)> = self
            .conn
            .query_row(
                r#"
                select source, session_id, turn_boundary_id, turn_id, total_tokens
                from token_observations
                where tracking_window_id is not null
                order by observed_at desc, id desc
                limit 1
                "#,
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .optional()?;
        let Some((source, session_id, turn_boundary_id, turn_id, latest_total)) = latest else {
            return Ok(0);
        };
        let boundary = turn_boundary_id.or(turn_id);
        let Some(boundary) = boundary else {
            return Ok(latest_total);
        };
        let total = self.conn.query_row(
            r#"
            select coalesce(sum(total_tokens), 0)
            from token_observations
            where source = ?1
              and coalesce(session_id, '') = coalesce(?2, '')
              and coalesce(turn_boundary_id, turn_id, '') = ?3
              and tracking_window_id is not null
            "#,
            params![source, session_id, boundary],
            |row| row.get(0),
        )?;
        Ok(total)
    }

    pub fn state_revision(&self) -> anyhow::Result<i64> {
        self.conn
            .query_row(
                r#"
                select coalesce(max(id), 0)
                from token_observations
                where tracking_window_id is not null
                "#,
                [],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    pub fn last_observed_at(&self) -> anyhow::Result<Option<DateTime<Utc>>> {
        let value: Option<String> = self.conn.query_row(
            r#"
            select max(observed_at)
            from token_observations
            where tracking_window_id is not null
            "#,
            [],
            |row| row.get(0),
        )?;
        value
            .map(|timestamp| {
                DateTime::parse_from_rfc3339(&timestamp)
                    .map(|parsed| parsed.with_timezone(&Utc))
                    .map_err(Into::into)
            })
            .transpose()
    }

    fn tracked_observation_count_between(
        &self,
        start_at: DateTime<Utc>,
        end_at: DateTime<Utc>,
    ) -> anyhow::Result<i64> {
        Ok(self.conn.query_row(
            r#"
            select count(*)
            from token_observations
            where tracking_window_id is not null
              and observed_at >= ?1
              and observed_at < ?2
            "#,
            params![start_at.to_rfc3339(), end_at.to_rfc3339()],
            |row| row.get(0),
        )?)
    }

    fn usage_buckets_for_window(
        &self,
        window_end: DateTime<Utc>,
    ) -> anyhow::Result<Vec<crate::core::usage_series::WidgetUsageBucket>> {
        let window_start = window_end - chrono::Duration::minutes(WIDGET_USAGE_WINDOW_MINUTES);
        let mut buckets = empty_usage_buckets(window_end);
        let mut stmt = self.conn.prepare(
            r#"
            select observed_at, total_tokens
            from token_observations
            where tracking_window_id is not null
              and observed_at >= ?1
              and observed_at < ?2
            order by observed_at asc
            "#,
        )?;
        let rows = stmt.query_map(
            params![window_start.to_rfc3339(), window_end.to_rfc3339()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )?;

        for row in rows {
            let (observed_at, total_tokens) = row?;
            let observed_at = DateTime::parse_from_rfc3339(&observed_at)?.with_timezone(&Utc);
            if let Some(index) = bucket_index_for(observed_at, window_end) {
                if let Some(bucket) = buckets.get_mut(index) {
                    bucket.total_tokens += total_tokens;
                }
            }
        }

        Ok(buckets)
    }

    pub fn usage_series_at(&self, now: DateTime<Utc>) -> anyhow::Result<WidgetUsageSeries> {
        let window_start = now - chrono::Duration::minutes(WIDGET_USAGE_WINDOW_MINUTES);
        let buckets = self.usage_buckets_for_window(now)?;
        let previous_day_buckets =
            self.usage_buckets_for_window(now - chrono::Duration::hours(24))?;

        let latest_bucket_start = buckets
            .last()
            .map(|bucket| bucket.start_at)
            .unwrap_or(window_start);
        let latest_bucket_tokens = buckets
            .last()
            .map(|bucket| bucket.total_tokens)
            .unwrap_or(0);
        let latest_bucket_observation_count =
            self.tracked_observation_count_between(latest_bucket_start, now)?;
        let last_observed_at = self.last_observed_at()?;
        let latest_bucket_active = last_observed_at
            .map(|observed_at| {
                latest_bucket_observation_count > 0
                    && observed_at >= latest_bucket_start
                    && observed_at < now
                    && now - observed_at
                        <= chrono::Duration::minutes(WIDGET_USAGE_ACTIVE_THRESHOLD_MINUTES)
            })
            .unwrap_or(false);

        Ok(WidgetUsageSeries {
            window_minutes: WIDGET_USAGE_WINDOW_MINUTES,
            bucket_minutes: crate::core::usage_series::WIDGET_USAGE_BUCKET_MINUTES,
            generated_at: now,
            state_revision: self.state_revision()?,
            average_tokens_per_bucket: average_tokens_per_bucket(&buckets),
            latest_bucket_tokens,
            latest_bucket_active,
            buckets,
            previous_day_buckets,
        })
    }

    pub fn cost_summary_between(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> anyhow::Result<CostPeriodSummary> {
        let mut stmt = self.conn.prepare(
            r#"
            select coalesce(model, '') as model,
                   coalesce(sum(input_tokens), 0),
                   coalesce(sum(output_tokens), 0),
                   coalesce(sum(cached_input_tokens), 0),
                   coalesce(sum(cache_creation_input_tokens), 0),
                   coalesce(sum(reasoning_output_tokens), 0),
                   coalesce(sum(total_tokens), 0)
            from token_observations
            where tracking_window_id is not null
              and observed_at >= ?1
              and observed_at < ?2
            group by coalesce(model, '')
            "#,
        )?;
        let rows = stmt.query_map(params![start.to_rfc3339(), end.to_rfc3339()], |row| {
            Ok(ModelTokenUsage {
                model: match row.get::<_, String>(0)?.as_str() {
                    "" => None,
                    value => Some(value.to_string()),
                },
                input_tokens: row.get(1)?,
                output_tokens: row.get(2)?,
                cached_input_tokens: row.get(3)?,
                cache_creation_input_tokens: row.get(4)?,
                reasoning_output_tokens: row.get(5)?,
                total_tokens: row.get(6)?,
            })
        })?;

        let mut estimated_cost = 0.0;
        let mut total_tokens = 0;
        let mut statuses = Vec::new();
        for row in rows {
            let cost = estimate_model_cost(&row?);
            estimated_cost += cost.estimated_cost;
            total_tokens += cost.total_tokens;
            statuses.push(cost.pricing_status);
        }

        Ok(CostPeriodSummary {
            estimated_cost,
            total_tokens,
            pricing_status: combine_pricing_status(&statuses),
        })
    }

    pub fn widget_cost_summary_at(
        &self,
        now_utc: DateTime<Utc>,
        now_local: DateTime<Local>,
    ) -> anyhow::Result<WidgetCostSummary> {
        let (today_start, _) = local_day_bounds(now_local);
        let today = self.cost_summary_between(today_start.with_timezone(&Utc), now_utc)?;
        let seven_days = self.cost_summary_between(now_utc - chrono::Duration::days(7), now_utc)?;
        Ok(WidgetCostSummary {
            generated_at: now_utc,
            currency: "CNY".to_string(),
            today,
            seven_days,
        })
    }

    fn profile_rows_between(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> anyhow::Result<Vec<ProfileObservationRow>> {
        let mut stmt = self.conn.prepare(
            r#"
            select source, model, input_tokens, output_tokens, cached_input_tokens,
                   cache_creation_input_tokens, reasoning_output_tokens, total_tokens, observed_at
            from token_observations
            where tracking_window_id is not null
              and observed_at >= ?1
              and observed_at < ?2
            order by observed_at asc, id asc
            "#,
        )?;
        let rows = stmt.query_map(params![start.to_rfc3339(), end.to_rfc3339()], |row| {
            let observed_at: String = row.get(8)?;
            Ok(ProfileObservationRow {
                source: row.get(0)?,
                model: row.get(1)?,
                input_tokens: row.get(2)?,
                output_tokens: row.get(3)?,
                cached_input_tokens: row.get(4)?,
                cache_creation_input_tokens: row.get(5)?,
                reasoning_output_tokens: row.get(6)?,
                total_tokens: row.get(7)?,
                observed_at: DateTime::parse_from_rfc3339(&observed_at)
                    .map(|parsed| parsed.with_timezone(&Utc))
                    .map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            8,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    fn row_usage(row: &ProfileObservationRow) -> ModelTokenUsage {
        ModelTokenUsage {
            model: row.model.clone(),
            input_tokens: row.input_tokens,
            output_tokens: row.output_tokens,
            cached_input_tokens: row.cached_input_tokens,
            cache_creation_input_tokens: row.cache_creation_input_tokens,
            reasoning_output_tokens: row.reasoning_output_tokens,
            total_tokens: row.total_tokens,
        }
    }

    fn priced_rows(rows: &[ProfileObservationRow]) -> Vec<PricedProfileObservationRow> {
        rows.iter()
            .map(|row| {
                let priced = estimate_model_cost_breakdown(&Self::row_usage(row));
                PricedProfileObservationRow {
                    source: row.source.clone(),
                    model: row.model.clone(),
                    estimated_cost: priced.estimated_cost,
                    total_tokens: priced.total_tokens,
                    drivers: priced.drivers,
                    observed_at: row.observed_at,
                }
            })
            .collect()
    }

    fn ranked_breakdown(
        rows: &[PricedProfileObservationRow],
        key_for: impl Fn(&PricedProfileObservationRow) -> (String, String),
    ) -> Vec<RankedProfileBreakdown> {
        let mut grouped: BTreeMap<String, (String, f64, i64)> = BTreeMap::new();
        for row in rows {
            let (key, label) = key_for(row);
            let entry = grouped.entry(key).or_insert((label, 0.0, 0));
            entry.1 += row.estimated_cost;
            entry.2 += row.total_tokens;
        }
        let total_token_sum: i64 = grouped.values().map(|(_, _, tokens)| *tokens).sum();
        let mut ranked = grouped
            .into_iter()
            .map(
                |(key, (label, estimated_cost, total_tokens))| RankedProfileBreakdown {
                    key,
                    label,
                    estimated_cost,
                    total_tokens,
                    share: if total_token_sum > 0 {
                        total_tokens as f64 / total_token_sum as f64
                    } else {
                        0.0
                    },
                },
            )
            .collect::<Vec<_>>();
        ranked.sort_by(|a, b| {
            b.total_tokens
                .cmp(&a.total_tokens)
                .then_with(|| a.label.cmp(&b.label))
        });
        if ranked.len() > 10 {
            let omitted = ranked.split_off(9);
            let other_estimated_cost: f64 = omitted.iter().map(|row| row.estimated_cost).sum();
            let other_total_tokens: i64 = omitted.iter().map(|row| row.total_tokens).sum();
            ranked.push(RankedProfileBreakdown {
                key: "other".to_string(),
                label: "Other".to_string(),
                estimated_cost: other_estimated_cost,
                total_tokens: other_total_tokens,
                share: if total_token_sum > 0 {
                    other_total_tokens as f64 / total_token_sum as f64
                } else {
                    0.0
                },
            });
        }
        ranked
    }

    fn cost_drivers(rows: &[PricedProfileObservationRow]) -> ProfileCostDrivers {
        let mut drivers = ProfileCostDrivers::default();
        let mut input_tokens = 0;
        for row in rows {
            drivers.input_cost += row.drivers.input_cost;
            drivers.output_cost += row.drivers.output_cost;
            drivers.reasoning_output_cost += row.drivers.reasoning_output_cost;
            drivers.cache_creation_input_cost += row.drivers.cache_creation_input_cost;
            drivers.cached_input_cost += row.drivers.cached_input_cost;
            drivers.unattributed_cost += row.drivers.unattributed_cost;
            drivers.cached_input_tokens += row.drivers.cached_input_tokens;
            input_tokens += row.drivers.input_tokens;
        }
        drivers.cache_read_ratio = if input_tokens > 0 {
            (drivers.cached_input_tokens as f64 / input_tokens as f64).clamp(0.0, 1.0)
        } else {
            0.0
        };
        drivers
    }

    fn year_profile_from_rows(
        rows: &[PricedProfileObservationRow],
        now_local: DateTime<Local>,
    ) -> YearProfileSummary {
        let today = now_local.date_naive();
        let first_day = today - chrono::Days::new(364);
        let mut days = (0..365)
            .map(|offset| {
                let local_date = first_day + chrono::Days::new(offset);
                (local_date, (0.0_f64, 0_i64))
            })
            .collect::<BTreeMap<_, _>>();

        for row in rows {
            let local_date = row.observed_at.with_timezone(&Local).date_naive();
            if let Some((estimated_cost, total_tokens)) = days.get_mut(&local_date) {
                *estimated_cost += row.estimated_cost;
                *total_tokens += row.total_tokens;
            }
        }

        let max_cost = days
            .values()
            .map(|(estimated_cost, _)| *estimated_cost)
            .fold(0.0_f64, f64::max);
        let estimated_cost: f64 = days.values().map(|(cost, _)| *cost).sum();
        let total_tokens: i64 = days.values().map(|(_, tokens)| *tokens).sum();
        let active_days = days
            .values()
            .filter(|(cost, tokens)| *cost > 0.0 || *tokens > 0)
            .count();
        let peak_day = days
            .iter()
            .max_by(|(_, (left_cost, _)), (_, (right_cost, _))| {
                left_cost
                    .partial_cmp(right_cost)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .and_then(|(local_date, (cost, tokens))| {
                if *cost > 0.0 || *tokens > 0 {
                    Some(ProfilePeakDay {
                        local_date: *local_date,
                        estimated_cost: *cost,
                        total_tokens: *tokens,
                    })
                } else {
                    None
                }
            });
        let buckets = days
            .into_iter()
            .map(
                |(local_date, (estimated_cost, total_tokens))| ProfileDayBucket {
                    local_date,
                    estimated_cost,
                    total_tokens,
                    intensity: intensity_for_cost(estimated_cost, max_cost),
                },
            )
            .collect();

        YearProfileSummary {
            days: buckets,
            estimated_cost,
            total_tokens,
            active_days,
            average_active_day_cost: if active_days > 0 {
                estimated_cost / active_days as f64
            } else {
                0.0
            },
            peak_day,
        }
    }

    fn period_usage_trend(
        period: ProfilePeriod,
        rows: &[PricedProfileObservationRow],
        now_utc: DateTime<Utc>,
        now_local: DateTime<Local>,
    ) -> anyhow::Result<crate::core::profile::PeriodUsageTrend> {
        let mut trend = empty_period_usage_trend(period, now_utc, now_local)?;
        for row in rows {
            if let Some(bucket) = trend.buckets.iter_mut().find(|bucket| {
                row.observed_at >= bucket.started_at && row.observed_at < bucket.ended_at
            }) {
                if let Some(total_tokens) = bucket.total_tokens.as_mut() {
                    *total_tokens += row.total_tokens;
                }
            }
        }
        Ok(trend)
    }

    pub fn profile_summary_at(
        &self,
        period: ProfilePeriod,
        now_utc: DateTime<Utc>,
        now_local: DateTime<Local>,
    ) -> anyhow::Result<ProfileSummary> {
        let first_year_day = now_local.date_naive() - chrono::Days::new(364);
        let year_start = first_year_day
            .and_hms_opt(0, 0, 0)
            .and_then(|start| start.and_local_timezone(Local).single())
            .ok_or_else(|| anyhow::anyhow!("invalid local year profile start: {first_year_day}"))?;
        let year_rows =
            Self::priced_rows(&self.profile_rows_between(year_start.with_timezone(&Utc), now_utc)?);
        let (period_start, period_end) = period_bounds(period, now_utc, now_local)?;
        let period_rows = Self::priced_rows(&self.profile_rows_between(period_start, period_end)?);
        let period_estimated_cost: f64 = period_rows.iter().map(|row| row.estimated_cost).sum();
        let period_total_tokens: i64 = period_rows.iter().map(|row| row.total_tokens).sum();
        let trend = Self::period_usage_trend(period, &period_rows, now_utc, now_local)?;

        Ok(ProfileSummary {
            generated_at: now_utc,
            currency: "CNY".to_string(),
            year_profile: Self::year_profile_from_rows(&year_rows, now_local),
            selected_period: PeriodProfileSummary {
                period,
                started_at: period_start,
                ended_at: period_end,
                estimated_cost: period_estimated_cost,
                total_tokens: period_total_tokens,
                trend,
                model_breakdown: Self::ranked_breakdown(&period_rows, |row| {
                    let label = model_label(row.model.as_deref());
                    let key = if label == "Unknown" {
                        "unknown".to_string()
                    } else {
                        label.to_ascii_lowercase()
                    };
                    (key, label)
                }),
                source_breakdown: Self::ranked_breakdown(&period_rows, |row| {
                    let label = source_label(&row.source);
                    let key = if label == "Unknown" {
                        "unknown".to_string()
                    } else {
                        row.source.trim().to_ascii_lowercase()
                    };
                    (key, label)
                }),
                cost_drivers: Self::cost_drivers(&period_rows),
            },
        })
    }

    pub fn open_tracking_window(&self, started_at: DateTime<Utc>) -> anyhow::Result<i64> {
        self.conn.execute(
            "insert into tracking_windows (started_at) values (?1)",
            params![started_at.to_rfc3339()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn close_tracking_window(&self, ended_at: DateTime<Utc>) -> anyhow::Result<()> {
        self.conn.execute(
            "update tracking_windows set ended_at = ?1 where ended_at is null",
            params![ended_at.to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn active_tracking_windows(&self) -> anyhow::Result<Vec<TrackingWindow>> {
        let mut stmt = self.conn.prepare(
            "select id, started_at, ended_at from tracking_windows where ended_at is null order by id",
        )?;
        let rows = stmt.query_map([], |row| {
            let started_at: String = row.get(1)?;
            let ended_at: Option<String> = row.get(2)?;
            Ok(TrackingWindow {
                id: row.get(0)?,
                started_at: DateTime::parse_from_rfc3339(&started_at)
                    .unwrap()
                    .with_timezone(&Utc),
                ended_at: ended_at.map(|value| {
                    DateTime::parse_from_rfc3339(&value)
                        .unwrap()
                        .with_timezone(&Utc)
                }),
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn tracking_windows_for_ingest(&self) -> anyhow::Result<Vec<TrackingWindow>> {
        let mut stmt = self
            .conn
            .prepare("select id, started_at, ended_at from tracking_windows order by id")?;
        let rows = stmt.query_map([], |row| {
            let started_at: String = row.get(1)?;
            let ended_at: Option<String> = row.get(2)?;
            Ok(TrackingWindow {
                id: row.get(0)?,
                started_at: DateTime::parse_from_rfc3339(&started_at)
                    .unwrap()
                    .with_timezone(&Utc),
                ended_at: ended_at.map(|value| {
                    DateTime::parse_from_rfc3339(&value)
                        .unwrap()
                        .with_timezone(&Utc)
                }),
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn recent_observation_metadata(
        &self,
        limit: i64,
    ) -> anyhow::Result<Vec<serde_json::Value>> {
        let mut stmt = self.conn.prepare(
            r#"
            select tracking_window_id, source, adapter_version, source_record_id_confidence, session_id, turn_id,
                   turn_boundary_id, source_path, line_no, byte_offset, total_tokens,
                   cumulative_total_tokens, model, cwd, observed_at, created_at
            from token_observations
            order by observed_at desc, id desc
            limit ?1
            "#,
        )?;
        let rows = stmt.query_map([limit], |row| {
            Ok(serde_json::json!({
                "tracking_window_id": row.get::<_, Option<i64>>(0)?,
                "source": row.get::<_, String>(1)?,
                "adapter_version": row.get::<_, String>(2)?,
                "source_record_id_confidence": row.get::<_, String>(3)?,
                "session_id": row.get::<_, Option<String>>(4)?,
                "turn_id": row.get::<_, Option<String>>(5)?,
                "turn_boundary_id": row.get::<_, Option<String>>(6)?,
                "source_path": row.get::<_, Option<String>>(7)?,
                "line_no": row.get::<_, Option<i64>>(8)?,
                "byte_offset": row.get::<_, Option<i64>>(9)?,
                "total_tokens": row.get::<_, i64>(10)?,
                "cumulative_total_tokens": row.get::<_, Option<i64>>(11)?,
                "model": row.get::<_, Option<String>>(12)?,
                "cwd": row.get::<_, Option<String>>(13)?,
                "observed_at": row.get::<_, String>(14)?,
                "created_at": row.get::<_, String>(15)?,
            }))
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn set_file_baseline(&self, source_path: &str, byte_offset: i64) -> anyhow::Result<()> {
        self.conn.execute(
            r#"
            insert into file_baselines (source_path, byte_offset)
            values (?1, ?2)
            on conflict(source_path) do update set byte_offset = excluded.byte_offset,
              updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
            "#,
            params![source_path, byte_offset],
        )?;
        Ok(())
    }

    pub fn file_baseline(&self, source_path: &str) -> anyhow::Result<Option<i64>> {
        let value = self
            .conn
            .query_row(
                "select byte_offset from file_baselines where source_path = ?1",
                params![source_path],
                |row| row.get(0),
            )
            .optional()?;
        Ok(value)
    }
}

/// 读取 rollup 状态机当前 state（None 表示 migration 未写、尚未 rebuild）。
/// 在事务内读取，保证与同事务写入一致。
fn rollup_state(tx: &rusqlite::Transaction<'_>) -> anyhow::Result<Option<String>> {
    tx.query_row(
        "select value from usage_rollup_metadata where key = ?1",
        params![ROLLUP_METADATA_STATE_KEY],
        |row| row.get(0),
    )
    .optional()
    .map_err(Into::into)
}

/// 写 retention 成功元数据：推进 last_success_at、记录本次删除数、清理失败标记。
/// 抽成独立函数供 Ready 主路径与 raw-only 降级路径共用，避免两处口径漂移。
fn write_retention_metadata(
    tx: &rusqlite::Transaction<'_>,
    now: DateTime<Utc>,
    deleted_observations: usize,
) -> anyhow::Result<()> {
    tx.execute(
        r#"
        insert into retention_state (key, value)
        values ('last_success_at', ?1)
        on conflict(key) do update set value = excluded.value
        "#,
        params![now.to_rfc3339()],
    )?;
    tx.execute(
        r#"
        insert into retention_state (key, value)
        values ('last_deleted_observations', ?1)
        on conflict(key) do update set value = excluded.value
        "#,
        params![deleted_observations.to_string()],
    )?;
    tx.execute(
        "delete from retention_state where key in ('last_failure_at', 'last_error_kind')",
        [],
    )?;
    Ok(())
}

/// Ready 模式 rollup 边界维护（在 raw 删除后、metadata 更新前于同事务内执行）。
///
/// cutoff 无需 15 分钟对齐；用 `bucket_start_utc` 取 cutoff 所属桶起点，与增量写入对齐。
/// 步骤：删除完全过期桶（< cutoff_bucket）→ 删除旧边界聚合（= cutoff_bucket）→
/// 从保留的 tracked raw rows（raw 删除已剔除 < cutoff 的行）重建 `[cutoff_bucket, cutoff_bucket+900)`。
/// 重建 SELECT 复用 `upsert_profile_rollup` 相同的 billable/unattributed CASE 表达式，
/// 保证增量 upsert / full rebuild / retention rebuild 同一数值域。整数域运算，不得转 REAL。
fn maintain_rollup_boundary(
    tx: &rusqlite::Transaction<'_>,
    cutoff: DateTime<Utc>,
) -> anyhow::Result<()> {
    let cutoff_bucket = bucket_start_utc(cutoff);
    let boundary_end = cutoff_bucket + PROFILE_ROLLUP_BUCKET_SECONDS;

    // 删除完全过期桶与旧边界聚合（<= cutoff_bucket）。
    tx.execute(
        "delete from usage_rollups_15m where bucket_start_utc <= ?1",
        params![cutoff_bucket],
    )?;

    // 从保留 raw 行重建边界桶：观测落在半开窗口 [cutoff_bucket, cutoff_bucket+900)，
    // 仅统计 tracked（tracking_window_id is not null）行；raw 删除已剔除 < cutoff 的贡献。
    // 时间戳用 strftime('%s') 转 unix 秒后再 floor 到桶，避免依赖 rfc3339 字符串比较。
    tx.execute(
        r#"
        insert into usage_rollups_15m (
          bucket_start_utc, source, model,
          input_tokens, billable_uncached_input_tokens, output_tokens,
          cached_input_tokens, cache_creation_input_tokens, reasoning_output_tokens,
          unattributed_total_tokens, total_tokens, observation_count
        )
        select
          ?1 as bucket_start_utc,
          source,
          coalesce(model, '') as model,
          sum(input_tokens),
          sum(max(input_tokens - cached_input_tokens - cache_creation_input_tokens, 0)),
          sum(output_tokens),
          sum(cached_input_tokens),
          sum(cache_creation_input_tokens),
          sum(reasoning_output_tokens),
          sum(case when input_tokens = 0 and output_tokens = 0 and cached_input_tokens = 0
                    and cache_creation_input_tokens = 0 and reasoning_output_tokens = 0
                   then max(total_tokens, 0) else 0 end),
          sum(total_tokens),
          count(*)
        from token_observations
        where tracking_window_id is not null
          and cast(strftime('%s', observed_at) as integer) >= ?1
          and cast(strftime('%s', observed_at) as integer) < ?2
        group by source, coalesce(model, '')
        "#,
        params![cutoff_bucket, boundary_end],
    )?;
    Ok(())
}

/// 写入 raw observation（唯一事实源）。封装原 23 列 insert-or-ignore，列映射保持不变。
/// 返回受影响行数：1 表示新插入，0 表示 dedupe 命中。
fn insert_raw_observation(
    tx: &rusqlite::Transaction<'_>,
    observation: &NormalizedObservation,
    tracking_window_id: Option<i64>,
) -> anyhow::Result<usize> {
    let dedupe_key = compute_dedupe_key(observation);
    let confidence = match observation.source_record_id_confidence {
        SourceRecordIdConfidence::Exact => "exact",
        SourceRecordIdConfidence::Fallback => "fallback",
    };
    let inserted = tx.execute(
        r#"
        insert or ignore into token_observations (
          tracking_window_id, source, adapter_version, source_record_id, source_record_id_confidence,
          session_id, turn_id, turn_boundary_id, source_path, line_no, byte_offset,
          input_tokens, output_tokens, cached_input_tokens, cache_creation_input_tokens,
          reasoning_output_tokens, total_tokens, cumulative_total_tokens, model, cwd,
          observed_at, token_payload_hash, dedupe_key
        ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23)
        "#,
        params![
            tracking_window_id,
            observation.source,
            observation.adapter_version,
            observation.source_record_id,
            confidence,
            observation.session_id,
            observation.turn_id,
            observation.turn_boundary_id,
            observation.source_path,
            observation.line_no,
            observation.byte_offset,
            observation.input_tokens,
            observation.output_tokens,
            observation.cached_input_tokens,
            observation.cache_creation_input_tokens,
            observation.reasoning_output_tokens,
            observation.total_tokens,
            observation.cumulative_total_tokens,
            observation.model,
            observation.cwd,
            observation.observed_at.to_rfc3339(),
            observation.token_payload_hash,
            dedupe_key,
        ],
    )?;
    Ok(inserted)
}

/// 增量维护派生 rollup：把该观测累加进对应 (bucket, source, model) 桶。
///
/// billable_uncached_input_tokens 与 unattributed_total_tokens 用 SQLite CASE/max 表达，
/// 与 Task 2 core 函数（`billable_uncached_input_tokens`/`unattributed_total_tokens`）语义一致，
/// 保证增量 upsert == 未来 full rebuild == retention rebuild 使用同一数值域。
/// model=NULL 规范化为 ''，但不改动 raw 表中的 NULL。整数域运算，不得转 REAL。
fn upsert_profile_rollup(
    tx: &rusqlite::Transaction<'_>,
    observation: &NormalizedObservation,
) -> anyhow::Result<()> {
    let bucket = bucket_start_utc(observation.observed_at);
    let model_key = normalize_model_key(observation.model.as_deref());
    tx.execute(
        r#"
        insert into usage_rollups_15m (
          bucket_start_utc, source, model,
          input_tokens, billable_uncached_input_tokens, output_tokens,
          cached_input_tokens, cache_creation_input_tokens, reasoning_output_tokens,
          unattributed_total_tokens, total_tokens, observation_count
        ) values (
          ?1, ?2, ?3,
          ?4,
          max(?4 - ?7 - ?8, 0),
          ?5,
          ?7, ?8, ?6,
          case when ?4 = 0 and ?5 = 0 and ?7 = 0 and ?8 = 0 and ?6 = 0 then max(?9, 0) else 0 end,
          ?9, 1
        )
        on conflict(bucket_start_utc, source, model) do update set
          input_tokens = input_tokens + excluded.input_tokens,
          billable_uncached_input_tokens =
            billable_uncached_input_tokens + excluded.billable_uncached_input_tokens,
          output_tokens = output_tokens + excluded.output_tokens,
          cached_input_tokens = cached_input_tokens + excluded.cached_input_tokens,
          cache_creation_input_tokens =
            cache_creation_input_tokens + excluded.cache_creation_input_tokens,
          reasoning_output_tokens = reasoning_output_tokens + excluded.reasoning_output_tokens,
          unattributed_total_tokens =
            unattributed_total_tokens + excluded.unattributed_total_tokens,
          total_tokens = total_tokens + excluded.total_tokens,
          observation_count = observation_count + excluded.observation_count
        "#,
        params![
            bucket,
            observation.source,
            model_key,
            observation.input_tokens,
            observation.output_tokens,
            observation.reasoning_output_tokens,
            observation.cached_input_tokens,
            observation.cache_creation_input_tokens,
            observation.total_tokens,
        ],
    )?;
    Ok(())
}

/// upsert 单条 rollup metadata（state / schema_version 等），key 冲突时覆盖。
fn upsert_rollup_metadata(
    tx: &rusqlite::Transaction<'_>,
    key: &str,
    value: &str,
) -> anyhow::Result<()> {
    tx.execute(
        r#"
        insert into usage_rollup_metadata (key, value)
        values (?1, ?2)
        on conflict(key) do update set value = excluded.value
        "#,
        params![key, value],
    )?;
    Ok(())
}

/// shadow table 原子重建 body（在单个 Immediate 事务内执行；不含 metadata 翻转）。
///
/// 步骤：清理残留 shadow → 建与正式表约束一致的 shadow（含 typeof integer CHECK）→
/// 一条 INSERT..SELECT 从所有 tracked raw 行按 (bucket, source, coalesce(model,'')) 聚合 →
/// 全局守恒校验（8 分量 + count 相等）→ drop 正式表 → rename shadow 为正式表。
/// 时间桶用 `cast(unixepoch(observed_at) as integer) / 900 * 900`，与 Rust `bucket_start_utc`
/// 的 div_euclid 对齐（observed_at 恒为 post-1970 正 epoch，整除与 div_euclid 一致）。
/// billable/unattributed CASE 表达式与 `upsert_profile_rollup` 完全同一方言，杜绝二义。
fn rebuild_profile_rollups_in_tx(tx: &rusqlite::Transaction<'_>) -> anyhow::Result<()> {
    // 清理任何残留 shadow（上次崩溃遗留），保证从干净状态建表。
    tx.execute(&format!("drop table if exists {ROLLUP_SHADOW_TABLE}"), [])?;

    // shadow 表 schema 必须与正式表逐字一致（含非负 + typeof integer CHECK），
    // 避免 SUM 溢出被静默提升为 REAL 后写回。
    tx.execute(&rollup_table_ddl(ROLLUP_SHADOW_TABLE), [])?;

    // 一条 INSERT..SELECT 全量聚合所有 tracked raw 行。
    tx.execute(
        &format!(
            r#"
            insert into {ROLLUP_SHADOW_TABLE} (
              bucket_start_utc, source, model,
              input_tokens, billable_uncached_input_tokens, output_tokens,
              cached_input_tokens, cache_creation_input_tokens, reasoning_output_tokens,
              unattributed_total_tokens, total_tokens, observation_count
            )
            select
              cast(unixepoch(observed_at) as integer) / {bucket} * {bucket} as bucket_start_utc,
              source,
              coalesce(model, '') as model,
              sum(input_tokens),
              sum(max(input_tokens - cached_input_tokens - cache_creation_input_tokens, 0)),
              sum(output_tokens),
              sum(cached_input_tokens),
              sum(cache_creation_input_tokens),
              sum(reasoning_output_tokens),
              sum(case when input_tokens = 0 and output_tokens = 0 and cached_input_tokens = 0
                        and cache_creation_input_tokens = 0 and reasoning_output_tokens = 0
                       then max(total_tokens, 0) else 0 end),
              sum(total_tokens),
              count(*)
            from token_observations
            where tracking_window_id is not null
            group by bucket_start_utc, source, coalesce(model, '')
            "#,
            bucket = PROFILE_ROLLUP_BUCKET_SECONDS
        ),
        [],
    )?;

    // 全局守恒诊断：tracked raw 与 shadow 的 8 分量 + count 必须逐项相等。
    let raw_totals = conservation_totals_in_tx(tx, ROLLUP_RAW_CONSERVATION_SQL)?;
    let shadow_totals = conservation_totals_in_tx(
        tx,
        &ROLLUP_ROLLUP_CONSERVATION_SQL.replace("usage_rollups_15m", ROLLUP_SHADOW_TABLE),
    )?;
    if raw_totals != shadow_totals {
        anyhow::bail!("profile rollup rebuild conservation mismatch");
    }

    // 换表：drop 正式表 + rename shadow，二者同事务，绝不出现半换状态。
    tx.execute("drop table usage_rollups_15m", [])?;
    tx.execute(
        &format!("alter table {ROLLUP_SHADOW_TABLE} rename to usage_rollups_15m"),
        [],
    )?;
    Ok(())
}

/// 在事务内执行一条守恒聚合 SQL，返回 9 元组（列序见 ROLLUP_RAW_CONSERVATION_SQL）。
fn conservation_totals_in_tx(
    tx: &rusqlite::Transaction<'_>,
    sql: &str,
) -> anyhow::Result<[i64; 9]> {
    tx.query_row(sql, [], |row| {
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
    })
    .map_err(Into::into)
}

/// 正式表 / shadow 表共用的建表 DDL（只替换表名）。约束与 migrate() 中 usage_rollups_15m 一致。
fn rollup_table_ddl(table: &str) -> String {
    format!(
        r#"
        create table {table} (
          bucket_start_utc integer not null,
          source text not null,
          model text not null,
          input_tokens integer not null default 0 check(input_tokens >= 0 and typeof(input_tokens) = 'integer'),
          billable_uncached_input_tokens integer not null default 0 check(billable_uncached_input_tokens >= 0 and typeof(billable_uncached_input_tokens) = 'integer'),
          output_tokens integer not null default 0 check(output_tokens >= 0 and typeof(output_tokens) = 'integer'),
          cached_input_tokens integer not null default 0 check(cached_input_tokens >= 0 and typeof(cached_input_tokens) = 'integer'),
          cache_creation_input_tokens integer not null default 0 check(cache_creation_input_tokens >= 0 and typeof(cache_creation_input_tokens) = 'integer'),
          reasoning_output_tokens integer not null default 0 check(reasoning_output_tokens >= 0 and typeof(reasoning_output_tokens) = 'integer'),
          unattributed_total_tokens integer not null default 0 check(unattributed_total_tokens >= 0 and typeof(unattributed_total_tokens) = 'integer'),
          total_tokens integer not null default 0 check(total_tokens >= 0 and typeof(total_tokens) = 'integer'),
          observation_count integer not null default 0 check(observation_count >= 0 and typeof(observation_count) = 'integer'),
          primary key (bucket_start_utc, source, model)
        )
        "#
    )
}

/// tracked raw 全局守恒聚合 SQL；列序：
/// input / billable / output / cached / cache_creation / reasoning / unattributed / total / count。
/// billable/unattributed 与 upsert_profile_rollup 同一 clamp/CASE 方言。
const ROLLUP_RAW_CONSERVATION_SQL: &str = r#"
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
"#;

/// rollup 侧全局守恒聚合 SQL（预聚合列直接求和），列序与 ROLLUP_RAW_CONSERVATION_SQL 对齐。
/// rebuild body 通过替换表名复用此 SQL 对 shadow 表做同口径聚合。
const ROLLUP_ROLLUP_CONSERVATION_SQL: &str = r#"
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
"#;
