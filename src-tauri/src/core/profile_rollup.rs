use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};

use crate::core::pricing::AggregatedModelTokenUsage;

/// 15 分钟 UTC 桶宽（秒）；Profile 年尺度读取以此对齐聚合行，避免扫描原始观测。
pub const PROFILE_ROLLUP_BUCKET_SECONDS: i64 = 15 * 60;
/// rollup 读模型 schema 版本；聚合语义变更时递增以便重算失效。
pub const PROFILE_ROLLUP_SCHEMA_VERSION: &str = "1";

/// 单个 (bucket, source, model) 桶的纯聚合读模型。
///
/// 约束：所有 token 字段非负；`billable_uncached_input_tokens` 与
/// `unattributed_total_tokens` 是逐行预聚合的独立累加量，不能由其它分量或
/// `total_tokens` 反推——先累加再 clamp 与逐行 clamp 再求和并不等价。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProfileTokenAggregate {
    pub bucket_start_utc: i64,
    pub source: String,
    pub model: Option<String>,
    pub input_tokens: i64,
    pub billable_uncached_input_tokens: i64,
    pub output_tokens: i64,
    pub cached_input_tokens: i64,
    pub cache_creation_input_tokens: i64,
    pub reasoning_output_tokens: i64,
    pub unattributed_total_tokens: i64,
    pub total_tokens: i64,
    pub observation_count: i64,
}

/// 将时间戳向下对齐到所属 15 分钟桶起点（UTC 秒）。
/// 用 `div_euclid` 保证负时间戳也向下取整（不会向 0 取整跨桶）。
pub fn bucket_start_utc(timestamp: DateTime<Utc>) -> i64 {
    timestamp
        .timestamp()
        .div_euclid(PROFILE_ROLLUP_BUCKET_SECONDS)
        * PROFILE_ROLLUP_BUCKET_SECONDS
}

/// 逐行计费用未缓存 input tokens：input - cached - cache_creation，clamp 到 0。
/// 单行数值小，`saturating_sub` 足够（不会溢出），负结果归零避免虚增成本。
pub fn billable_uncached_input_tokens(input: i64, cached: i64, creation: i64) -> i64 {
    input.saturating_sub(cached).saturating_sub(creation).max(0)
}

/// 仅当一条观测没有任何分量 token（只有 total）时，才把 total 记为未归因量。
/// 有分量时未归因量为 0——避免与分量重复计价。
pub fn unattributed_total_tokens(
    input: i64,
    output: i64,
    cached: i64,
    creation: i64,
    reasoning: i64,
    total: i64,
) -> i64 {
    if [input, output, cached, creation, reasoning]
        .into_iter()
        .all(|value| value == 0)
    {
        total.max(0)
    } else {
        0
    }
}

/// 存储 key 规范化：`model = None` 记为空字符串 `""`（缺失模型语义）。
pub fn normalize_model_key(model: Option<&str>) -> String {
    model.unwrap_or("").to_string()
}

/// 读回时把空字符串 key 还原为 `None`，恢复缺失模型语义。
pub fn model_from_key(key: &str) -> Option<String> {
    if key.is_empty() {
        None
    } else {
        Some(key.to_string())
    }
}

impl ProfileTokenAggregate {
    /// 桶身份 key：(bucket, source, 规范化 model)。合并前必须相同。
    fn identity_key(&self) -> (i64, &str, String) {
        (
            self.bucket_start_utc,
            self.source.as_str(),
            normalize_model_key(self.model.as_deref()),
        )
    }

    /// 合并同一 (bucket, source, model) 的另一聚合行。
    /// 无单行上限校验，故一律用 checked add：溢出返回错误，绝不静默 wrap。
    pub fn merge(&mut self, other: &ProfileTokenAggregate) -> Result<()> {
        if self.identity_key() != other.identity_key() {
            return Err(anyhow!(
                "cannot merge aggregates with different (bucket, source, model) keys"
            ));
        }
        self.input_tokens = checked_add(self.input_tokens, other.input_tokens, "input_tokens")?;
        self.billable_uncached_input_tokens = checked_add(
            self.billable_uncached_input_tokens,
            other.billable_uncached_input_tokens,
            "billable_uncached_input_tokens",
        )?;
        self.output_tokens = checked_add(self.output_tokens, other.output_tokens, "output_tokens")?;
        self.cached_input_tokens = checked_add(
            self.cached_input_tokens,
            other.cached_input_tokens,
            "cached_input_tokens",
        )?;
        self.cache_creation_input_tokens = checked_add(
            self.cache_creation_input_tokens,
            other.cache_creation_input_tokens,
            "cache_creation_input_tokens",
        )?;
        self.reasoning_output_tokens = checked_add(
            self.reasoning_output_tokens,
            other.reasoning_output_tokens,
            "reasoning_output_tokens",
        )?;
        self.unattributed_total_tokens = checked_add(
            self.unattributed_total_tokens,
            other.unattributed_total_tokens,
            "unattributed_total_tokens",
        )?;
        self.total_tokens = checked_add(self.total_tokens, other.total_tokens, "total_tokens")?;
        self.observation_count = checked_add(
            self.observation_count,
            other.observation_count,
            "observation_count",
        )?;
        Ok(())
    }

    /// 转成聚合定价入参；预聚合量直接透传，定价侧不得再 clamp/反推。
    pub fn to_aggregated_usage(&self) -> AggregatedModelTokenUsage {
        AggregatedModelTokenUsage {
            model: self.model.clone(),
            input_tokens: self.input_tokens,
            billable_uncached_input_tokens: self.billable_uncached_input_tokens,
            output_tokens: self.output_tokens,
            cached_input_tokens: self.cached_input_tokens,
            cache_creation_input_tokens: self.cache_creation_input_tokens,
            reasoning_output_tokens: self.reasoning_output_tokens,
            unattributed_total_tokens: self.unattributed_total_tokens,
            total_tokens: self.total_tokens,
        }
    }
}

/// checked 累加；溢出返回带字段名的错误，供上层定位。
fn checked_add(lhs: i64, rhs: i64, field: &str) -> Result<i64> {
    lhs.checked_add(rhs)
        .ok_or_else(|| anyhow!("profile rollup overflow while accumulating {field}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::pricing::{
        estimate_aggregated_model_cost_breakdown, estimate_model_cost_breakdown, ModelTokenUsage,
        PricingStatus,
    };
    use chrono::{TimeZone, Utc};

    fn assert_close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 0.000_001,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn bucket_start_utc_floors_to_fifteen_minute_boundary() {
        let ts = Utc.with_ymd_and_hms(2026, 7, 11, 12, 14, 59).unwrap();
        let expected = Utc
            .with_ymd_and_hms(2026, 7, 11, 12, 0, 0)
            .unwrap()
            .timestamp();
        assert_eq!(bucket_start_utc(ts), expected);
    }

    #[test]
    fn billable_uncached_input_clamps_to_zero() {
        assert_eq!(billable_uncached_input_tokens(10, 8, 7), 0);
        assert_eq!(
            billable_uncached_input_tokens(1_000_000, 250_000, 100_000),
            650_000
        );
    }

    #[test]
    fn unattributed_only_when_no_components() {
        assert_eq!(
            unattributed_total_tokens(0, 0, 0, 0, 0, 1_000_000),
            1_000_000
        );
        assert_eq!(unattributed_total_tokens(1, 0, 0, 0, 0, 1_000_000), 0);
    }

    #[test]
    fn model_key_normalizes_none_to_empty_and_back() {
        assert_eq!(normalize_model_key(None), "");
        assert_eq!(normalize_model_key(Some("gpt-5.5")), "gpt-5.5");
        assert_eq!(model_from_key(""), None);
        assert_eq!(model_from_key("gpt-5.5"), Some("gpt-5.5".to_string()));
    }

    // 一条 component observation + 一条 total-only observation 落在同 model/bucket，
    // 聚合后定价必须逐分量、unattributed 与总额都等于两条 raw 分别定价再求和。
    #[test]
    fn merged_bucket_prices_equal_per_row_raw_pricing() {
        let bucket = bucket_start_utc(Utc.with_ymd_and_hms(2026, 7, 11, 12, 14, 59).unwrap());

        let component = ProfileTokenAggregate {
            bucket_start_utc: bucket,
            source: "traex".to_string(),
            model: Some("gpt-5.5".to_string()),
            input_tokens: 1_000_000,
            billable_uncached_input_tokens: billable_uncached_input_tokens(
                1_000_000, 250_000, 100_000,
            ),
            output_tokens: 500_000,
            cached_input_tokens: 250_000,
            cache_creation_input_tokens: 100_000,
            reasoning_output_tokens: 50_000,
            unattributed_total_tokens: unattributed_total_tokens(
                1_000_000, 500_000, 250_000, 100_000, 50_000, 1_900_000,
            ),
            total_tokens: 1_900_000,
            observation_count: 1,
        };
        let total_only = ProfileTokenAggregate {
            bucket_start_utc: bucket,
            source: "traex".to_string(),
            model: Some("gpt-5.5".to_string()),
            input_tokens: 0,
            billable_uncached_input_tokens: billable_uncached_input_tokens(0, 0, 0),
            output_tokens: 0,
            cached_input_tokens: 0,
            cache_creation_input_tokens: 0,
            reasoning_output_tokens: 0,
            unattributed_total_tokens: unattributed_total_tokens(0, 0, 0, 0, 0, 1_000_000),
            total_tokens: 1_000_000,
            observation_count: 1,
        };

        let mut merged = component.clone();
        merged.merge(&total_only).expect("merge same key");
        assert_eq!(merged.observation_count, 2);
        assert_eq!(merged.total_tokens, 2_900_000);
        assert_eq!(merged.unattributed_total_tokens, 1_000_000);
        assert_eq!(merged.billable_uncached_input_tokens, 650_000);

        let aggregated = estimate_aggregated_model_cost_breakdown(&merged.to_aggregated_usage());

        // Raw path: 两条 observation 分别定价。
        let raw_component = estimate_model_cost_breakdown(&ModelTokenUsage {
            model: Some("gpt-5.5".to_string()),
            input_tokens: 1_000_000,
            output_tokens: 500_000,
            cached_input_tokens: 250_000,
            cache_creation_input_tokens: 100_000,
            reasoning_output_tokens: 50_000,
            total_tokens: 1_900_000,
        });
        let raw_total_only = estimate_model_cost_breakdown(&ModelTokenUsage {
            model: Some("gpt-5.5".to_string()),
            input_tokens: 0,
            output_tokens: 0,
            cached_input_tokens: 0,
            cache_creation_input_tokens: 0,
            reasoning_output_tokens: 0,
            total_tokens: 1_000_000,
        });

        assert_eq!(aggregated.pricing_status, PricingStatus::Rule);
        assert_close(
            aggregated.drivers.input_cost,
            raw_component.drivers.input_cost + raw_total_only.drivers.input_cost,
        );
        assert_close(
            aggregated.drivers.output_cost,
            raw_component.drivers.output_cost + raw_total_only.drivers.output_cost,
        );
        assert_close(
            aggregated.drivers.reasoning_output_cost,
            raw_component.drivers.reasoning_output_cost
                + raw_total_only.drivers.reasoning_output_cost,
        );
        assert_close(
            aggregated.drivers.cache_creation_input_cost,
            raw_component.drivers.cache_creation_input_cost
                + raw_total_only.drivers.cache_creation_input_cost,
        );
        assert_close(
            aggregated.drivers.cached_input_cost,
            raw_component.drivers.cached_input_cost + raw_total_only.drivers.cached_input_cost,
        );
        assert_close(
            aggregated.drivers.unattributed_cost,
            raw_component.drivers.unattributed_cost + raw_total_only.drivers.unattributed_cost,
        );
        assert_close(
            aggregated.estimated_cost,
            raw_component.estimated_cost + raw_total_only.estimated_cost,
        );
        assert_eq!(
            aggregated.total_tokens,
            raw_component.total_tokens + raw_total_only.total_tokens
        );
    }

    #[test]
    fn merge_rejects_mismatched_keys() {
        let base = ProfileTokenAggregate {
            bucket_start_utc: 0,
            source: "traex".to_string(),
            model: Some("gpt-5.5".to_string()),
            ..ProfileTokenAggregate::default()
        };
        let other_model = ProfileTokenAggregate {
            model: Some("gpt-5.4".to_string()),
            ..base.clone()
        };
        assert!(base.clone().merge(&other_model).is_err());
    }

    #[test]
    fn merge_errors_on_overflow_instead_of_wrapping() {
        let mut a = ProfileTokenAggregate {
            total_tokens: i64::MAX,
            ..ProfileTokenAggregate::default()
        };
        let b = ProfileTokenAggregate {
            total_tokens: 1,
            ..ProfileTokenAggregate::default()
        };
        assert!(a.merge(&b).is_err());
    }
}
