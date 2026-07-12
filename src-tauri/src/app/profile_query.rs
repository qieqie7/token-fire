//! Profile 查询的异步编排：把 owned database path 交给 `spawn_blocking`，让 SQLite 与定价
//! 计算离开 Tauri 主线程；await 返回后在异步侧记录非敏感诊断日志（不含路径/observation 内容）。

use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, Local, Utc};
use serde_json::json;

use crate::app::logging::{LogFile, RuntimeLogSinks};
use crate::core::profile::ProfilePeriod;
use crate::core::usage_store::{ProfileQueryOutcome, ProfileQuerySource, UsageStore};

/// 在 blocking 线程上打开 store 并执行 Profile 查询。closure 只捕获 owned 的
/// database path / period / time——不捕获借用的 State 或日志 sink，保证不跨异步边界持有借用。
pub async fn query_profile_summary(
    database: PathBuf,
    period: ProfilePeriod,
    now_utc: DateTime<Utc>,
) -> anyhow::Result<ProfileQueryOutcome> {
    tauri::async_runtime::spawn_blocking(move || {
        let now_local = now_utc.with_timezone(&Local);
        let store = UsageStore::open(&database)?;
        store.profile_summary_with_diagnostics_at(period, now_utc, now_local)
    })
    .await
    .map_err(|error| anyhow::anyhow!("profile query task failed: {error}"))?
}

/// 记录一次 Profile 查询的诊断。成功记来源/耗时/rollup 行数/schema 版本；失败只记耗时与
/// 稳定 error_kind——绝不写入原始 SQLite 错误文本（可能含数据库路径）或 observation 明细。
pub fn log_profile_query_result(
    sinks: &RuntimeLogSinks,
    elapsed: Duration,
    result: &anyhow::Result<ProfileQueryOutcome>,
) {
    match result {
        Ok(outcome) => {
            let source = match outcome.source {
                ProfileQuerySource::Rollup => "rollup",
                ProfileQuerySource::RawFallback => "raw_fallback",
            };
            sinks.write(
                LogFile::Db,
                "db",
                "info",
                "profile_query_completed",
                json!({
                    "profile_query_source": source,
                    "profile_query_duration_ms": elapsed.as_millis() as u64,
                    "rollup_row_count": outcome.rollup_row_count as u64,
                    "rollup_schema_version": outcome.rollup_schema_version,
                }),
            );
        }
        Err(_) => {
            // 稳定 error_kind，不透传底层 SQLite 文本（可能含路径）。
            sinks.write(
                LogFile::Db,
                "db",
                "warn",
                "profile_query_failed",
                json!({
                    "profile_query_duration_ms": elapsed.as_millis() as u64,
                    "error_kind": "profile_query_failed",
                }),
            );
        }
    }
}
