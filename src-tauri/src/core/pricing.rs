use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const DEFAULT_AVERAGE_CNY_PER_1M_TOKENS: f64 = 6.5;
pub const DEFAULT_INPUT_CNY_PER_1M_TOKENS: f64 = 3.0;
pub const DEFAULT_OUTPUT_CNY_PER_1M_TOKENS: f64 = 12.0;
pub const DEFAULT_CACHED_INPUT_CNY_PER_1M_TOKENS: f64 = 0.5;
pub const DEFAULT_CACHE_CREATION_INPUT_CNY_PER_1M_TOKENS: f64 = 3.0;
pub const DEFAULT_REASONING_OUTPUT_CNY_PER_1M_TOKENS: f64 = 12.0;
pub const STATIC_USD_CNY_RATE_2026_07: f64 = 7.25;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PricingStatus {
    Rule,
    Fallback,
    Mixed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CostPeriodSummary {
    pub estimated_cost: f64,
    pub total_tokens: i64,
    pub pricing_status: PricingStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WidgetCostSummary {
    pub generated_at: DateTime<Utc>,
    pub currency: String,
    pub today: CostPeriodSummary,
    pub seven_days: CostPeriodSummary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelTokenUsage {
    pub model: Option<String>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cached_input_tokens: i64,
    pub cache_creation_input_tokens: i64,
    pub reasoning_output_tokens: i64,
    pub total_tokens: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PricedModelCost {
    pub estimated_cost: f64,
    pub total_tokens: i64,
    pub pricing_status: PricingStatus,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PricedCostDrivers {
    pub input_cost: f64,
    pub output_cost: f64,
    pub reasoning_output_cost: f64,
    pub cache_creation_input_cost: f64,
    pub cached_input_cost: f64,
    pub unattributed_cost: f64,
    pub cached_input_tokens: i64,
    pub input_tokens: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PricedModelCostBreakdown {
    pub estimated_cost: f64,
    pub total_tokens: i64,
    pub pricing_status: PricingStatus,
    pub drivers: PricedCostDrivers,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Currency {
    Cny,
    Usd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelMatch {
    Exact(&'static str),
    Prefix(&'static str),
}

#[derive(Debug, Clone, Copy)]
pub struct PricingRule {
    pub id: &'static str,
    pub priority: u32,
    pub model_match: ModelMatch,
    pub currency: Currency,
    pub input_per_1m: f64,
    pub output_per_1m: f64,
    pub cached_input_per_1m: Option<f64>,
    pub cache_creation_input_per_1m: Option<f64>,
    pub reasoning_output_per_1m: Option<f64>,
}

const PRICING_RULES: &[PricingRule] = &[
    PricingRule {
        id: "gpt-5.5",
        priority: 100,
        model_match: ModelMatch::Prefix("gpt-5.5"),
        currency: Currency::Usd,
        input_per_1m: 5.00,
        output_per_1m: 30.00,
        cached_input_per_1m: Some(0.50),
        cache_creation_input_per_1m: Some(5.00),
        reasoning_output_per_1m: Some(30.00),
    },
    PricingRule {
        id: "gpt-5.4",
        priority: 95,
        model_match: ModelMatch::Prefix("gpt-5.4"),
        currency: Currency::Usd,
        input_per_1m: 2.50,
        output_per_1m: 15.00,
        cached_input_per_1m: Some(0.25),
        cache_creation_input_per_1m: Some(2.50),
        reasoning_output_per_1m: Some(15.00),
    },
    PricingRule {
        id: "gpt-5",
        priority: 90,
        model_match: ModelMatch::Prefix("gpt-5"),
        currency: Currency::Usd,
        input_per_1m: 1.25,
        output_per_1m: 10.00,
        cached_input_per_1m: Some(0.125),
        cache_creation_input_per_1m: Some(1.25),
        reasoning_output_per_1m: Some(10.00),
    },
    PricingRule {
        id: "claude-sonnet",
        priority: 80,
        model_match: ModelMatch::Prefix("claude-sonnet"),
        currency: Currency::Usd,
        input_per_1m: 3.00,
        output_per_1m: 15.00,
        cached_input_per_1m: Some(0.30),
        cache_creation_input_per_1m: Some(3.75),
        reasoning_output_per_1m: Some(15.00),
    },
    PricingRule {
        id: "kimi-k2.6",
        priority: 70,
        model_match: ModelMatch::Prefix("kimi-k2.6"),
        currency: Currency::Usd,
        input_per_1m: 0.95,
        output_per_1m: 4.00,
        cached_input_per_1m: Some(0.16),
        cache_creation_input_per_1m: Some(0.95),
        reasoning_output_per_1m: Some(4.00),
    },
    PricingRule {
        id: "kimi-k2.5",
        priority: 69,
        model_match: ModelMatch::Prefix("kimi-k2.5"),
        currency: Currency::Usd,
        input_per_1m: 0.60,
        output_per_1m: 3.00,
        cached_input_per_1m: Some(0.10),
        cache_creation_input_per_1m: Some(0.60),
        reasoning_output_per_1m: Some(3.00),
    },
    PricingRule {
        id: "gemini-2.5-flash",
        priority: 68,
        model_match: ModelMatch::Prefix("gemini-2.5-flash"),
        currency: Currency::Usd,
        input_per_1m: 0.30,
        output_per_1m: 2.50,
        cached_input_per_1m: Some(0.075),
        cache_creation_input_per_1m: Some(0.30),
        reasoning_output_per_1m: Some(2.50),
    },
    PricingRule {
        id: "gemini-2.5-pro",
        priority: 67,
        model_match: ModelMatch::Prefix("gemini-2.5-pro"),
        currency: Currency::Usd,
        input_per_1m: 1.25,
        output_per_1m: 10.00,
        cached_input_per_1m: Some(0.3125),
        cache_creation_input_per_1m: Some(1.25),
        reasoning_output_per_1m: Some(10.00),
    },
    PricingRule {
        id: "deepseek-v4-flash",
        priority: 66,
        model_match: ModelMatch::Prefix("deepseek-v4-flash"),
        currency: Currency::Usd,
        input_per_1m: 0.14,
        output_per_1m: 0.28,
        cached_input_per_1m: Some(0.0028),
        cache_creation_input_per_1m: Some(0.14),
        reasoning_output_per_1m: Some(0.28),
    },
    PricingRule {
        id: "deepseek-v4-pro",
        priority: 65,
        model_match: ModelMatch::Prefix("deepseek-v4-pro"),
        currency: Currency::Usd,
        input_per_1m: 0.435,
        output_per_1m: 0.87,
        cached_input_per_1m: Some(0.0036),
        cache_creation_input_per_1m: Some(0.435),
        reasoning_output_per_1m: Some(0.87),
    },
    PricingRule {
        id: "qwen3-max",
        priority: 64,
        model_match: ModelMatch::Prefix("qwen3-max"),
        currency: Currency::Cny,
        input_per_1m: 3.00,
        output_per_1m: 12.00,
        cached_input_per_1m: Some(3.00),
        cache_creation_input_per_1m: Some(3.00),
        reasoning_output_per_1m: Some(12.00),
    },
    PricingRule {
        id: "qwen-max",
        priority: 63,
        model_match: ModelMatch::Prefix("qwen-max"),
        currency: Currency::Cny,
        input_per_1m: 2.40,
        output_per_1m: 9.60,
        cached_input_per_1m: Some(2.40),
        cache_creation_input_per_1m: Some(2.40),
        reasoning_output_per_1m: Some(9.60),
    },
    PricingRule {
        id: "doubao-seed-1.6",
        priority: 62,
        model_match: ModelMatch::Prefix("doubao-seed-1.6"),
        currency: Currency::Cny,
        input_per_1m: 0.40,
        output_per_1m: 4.00,
        cached_input_per_1m: Some(0.16),
        cache_creation_input_per_1m: Some(0.40),
        reasoning_output_per_1m: Some(4.00),
    },
];

const FALLBACK_PRICING_RULE: PricingRule = PricingRule {
    id: "fallback-average",
    priority: 0,
    model_match: ModelMatch::Prefix(""),
    currency: Currency::Cny,
    input_per_1m: DEFAULT_INPUT_CNY_PER_1M_TOKENS,
    output_per_1m: DEFAULT_OUTPUT_CNY_PER_1M_TOKENS,
    cached_input_per_1m: Some(DEFAULT_CACHED_INPUT_CNY_PER_1M_TOKENS),
    cache_creation_input_per_1m: Some(DEFAULT_CACHE_CREATION_INPUT_CNY_PER_1M_TOKENS),
    reasoning_output_per_1m: Some(DEFAULT_REASONING_OUTPUT_CNY_PER_1M_TOKENS),
};

fn model_matches(rule: PricingRule, model: &str) -> bool {
    match rule.model_match {
        ModelMatch::Exact(expected) => model.eq_ignore_ascii_case(expected),
        ModelMatch::Prefix(prefix) => model
            .get(..prefix.len())
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(prefix)),
    }
}

fn match_rank(rule: PricingRule) -> u32 {
    match rule.model_match {
        ModelMatch::Exact(_) => rule.priority * 2 + 1,
        ModelMatch::Prefix(_) => rule.priority * 2,
    }
}

fn find_rule(model: Option<&str>) -> Option<PricingRule> {
    let model = model?;
    PRICING_RULES
        .iter()
        .copied()
        .filter(|rule| model_matches(*rule, model))
        .max_by_key(|rule| match_rank(*rule))
}

fn rate_to_cny(rule: PricingRule, value: f64) -> f64 {
    match rule.currency {
        Currency::Cny => value,
        Currency::Usd => value * STATIC_USD_CNY_RATE_2026_07,
    }
}

fn price_tokens(tokens: i64, price_per_1m_cny: f64) -> f64 {
    tokens.max(0) as f64 * price_per_1m_cny / 1_000_000.0
}

fn billable_uncached_input_tokens(usage: &ModelTokenUsage) -> i64 {
    usage.input_tokens - usage.cached_input_tokens - usage.cache_creation_input_tokens
}

fn has_component_tokens(usage: &ModelTokenUsage) -> bool {
    usage.input_tokens > 0
        || usage.output_tokens > 0
        || usage.cached_input_tokens > 0
        || usage.cache_creation_input_tokens > 0
        || usage.reasoning_output_tokens > 0
}

fn estimate_cost_drivers_with_rule(
    usage: &ModelTokenUsage,
    rule: PricingRule,
) -> PricedCostDrivers {
    let input = rate_to_cny(rule, rule.input_per_1m);
    let output = rate_to_cny(rule, rule.output_per_1m);
    let cached_input = rate_to_cny(rule, rule.cached_input_per_1m.unwrap_or(rule.input_per_1m));
    let cache_creation = rate_to_cny(
        rule,
        rule.cache_creation_input_per_1m
            .unwrap_or(rule.input_per_1m),
    );
    let reasoning_output = rate_to_cny(
        rule,
        rule.reasoning_output_per_1m.unwrap_or(rule.output_per_1m),
    );

    PricedCostDrivers {
        input_cost: price_tokens(billable_uncached_input_tokens(usage), input),
        output_cost: price_tokens(usage.output_tokens, output),
        reasoning_output_cost: price_tokens(usage.reasoning_output_tokens, reasoning_output),
        cache_creation_input_cost: price_tokens(usage.cache_creation_input_tokens, cache_creation),
        cached_input_cost: price_tokens(usage.cached_input_tokens, cached_input),
        cached_input_tokens: usage.cached_input_tokens.max(0),
        input_tokens: usage.input_tokens.max(0),
        ..PricedCostDrivers::default()
    }
}

pub fn estimate_model_cost_breakdown(usage: &ModelTokenUsage) -> PricedModelCostBreakdown {
    let (rule, pricing_status) = match find_rule(usage.model.as_deref()) {
        Some(rule) => (rule, PricingStatus::Rule),
        None => (FALLBACK_PRICING_RULE, PricingStatus::Fallback),
    };

    if !has_component_tokens(usage) {
        let estimated_cost = price_tokens(usage.total_tokens, DEFAULT_AVERAGE_CNY_PER_1M_TOKENS);
        return PricedModelCostBreakdown {
            estimated_cost,
            total_tokens: usage.total_tokens.max(0),
            pricing_status,
            drivers: PricedCostDrivers {
                unattributed_cost: estimated_cost,
                ..PricedCostDrivers::default()
            },
        };
    }

    let drivers = estimate_cost_drivers_with_rule(usage, rule);
    PricedModelCostBreakdown {
        estimated_cost: drivers.input_cost
            + drivers.output_cost
            + drivers.reasoning_output_cost
            + drivers.cache_creation_input_cost
            + drivers.cached_input_cost,
        total_tokens: usage.total_tokens.max(0),
        pricing_status,
        drivers,
    }
}

pub fn estimate_model_cost(usage: &ModelTokenUsage) -> PricedModelCost {
    let breakdown = estimate_model_cost_breakdown(usage);
    PricedModelCost {
        estimated_cost: breakdown.estimated_cost,
        total_tokens: breakdown.total_tokens,
        pricing_status: breakdown.pricing_status,
    }
}

pub fn combine_pricing_status(statuses: &[PricingStatus]) -> PricingStatus {
    let has_rule = statuses
        .iter()
        .any(|status| matches!(status, PricingStatus::Rule));
    let has_fallback = statuses
        .iter()
        .any(|status| matches!(status, PricingStatus::Fallback));
    let has_mixed = statuses
        .iter()
        .any(|status| matches!(status, PricingStatus::Mixed));

    if has_mixed || (has_rule && has_fallback) {
        PricingStatus::Mixed
    } else if has_fallback {
        PricingStatus::Fallback
    } else {
        PricingStatus::Rule
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(model: Option<&str>) -> ModelTokenUsage {
        ModelTokenUsage {
            model: model.map(str::to_string),
            input_tokens: 1_000_000,
            output_tokens: 500_000,
            cached_input_tokens: 250_000,
            cache_creation_input_tokens: 100_000,
            reasoning_output_tokens: 50_000,
            total_tokens: 1_900_000,
        }
    }

    fn assert_close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 0.000_001,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn matched_model_rule_uses_category_prices() {
        let cost = estimate_model_cost(&usage(Some("gpt-5.5")));

        assert_eq!(cost.total_tokens, 1_900_000);
        assert_eq!(cost.pricing_status, PricingStatus::Rule);
        assert_close(cost.estimated_cost, 147.71875);
    }

    #[test]
    fn cached_input_tokens_are_priced_as_input_subset() {
        let cost = estimate_model_cost(&ModelTokenUsage {
            model: Some("gpt-5.5".to_string()),
            input_tokens: 1_000_000,
            output_tokens: 0,
            cached_input_tokens: 900_000,
            cache_creation_input_tokens: 0,
            reasoning_output_tokens: 0,
            total_tokens: 1_000_000,
        });

        assert_eq!(cost.pricing_status, PricingStatus::Rule);
        assert_close(cost.estimated_cost, 6.8875);
    }

    #[test]
    fn cached_input_tokens_cannot_make_uncached_input_negative() {
        let cost = estimate_model_cost(&ModelTokenUsage {
            model: Some("gpt-5.5".to_string()),
            input_tokens: 100_000,
            output_tokens: 0,
            cached_input_tokens: 120_000,
            cache_creation_input_tokens: 30_000,
            reasoning_output_tokens: 0,
            total_tokens: 100_000,
        });

        assert_eq!(cost.pricing_status, PricingStatus::Rule);
        assert_close(cost.estimated_cost, 1.5225);
    }

    #[test]
    fn model_rule_matching_is_case_insensitive() {
        let cost = estimate_model_cost(&usage(Some("GPT-5.5")));

        assert_eq!(cost.pricing_status, PricingStatus::Rule);
        assert_close(cost.estimated_cost, 147.71875);
    }

    #[test]
    fn prefix_rule_matches_snapshot_model_ids() {
        let cost = estimate_model_cost(&usage(Some("gpt-5.5-20260701")));

        assert_eq!(cost.pricing_status, PricingStatus::Rule);
        assert_close(cost.estimated_cost, 147.71875);
    }

    #[test]
    fn gpt_5_4_rule_takes_precedence_over_generic_gpt_5() {
        let cost = estimate_model_cost(&usage(Some("gpt-5.4-20260701")));

        assert_eq!(cost.pricing_status, PricingStatus::Rule);
        assert_close(cost.estimated_cost, 73.859375);
    }

    #[test]
    fn verified_usd_rules_are_converted_to_cny() {
        let deepseek = estimate_model_cost(&usage(Some("deepseek-v4-flash")));
        let gemini = estimate_model_cost(&usage(Some("gemini-2.5-flash")));

        assert_eq!(deepseek.pricing_status, PricingStatus::Rule);
        assert_close(deepseek.estimated_cost, 1.882825);
        assert_eq!(gemini.pricing_status, PricingStatus::Rule);
        assert_close(gemini.estimated_cost, 11.7359375);
    }

    #[test]
    fn verified_cny_rules_use_cny_prices_directly() {
        let qwen = estimate_model_cost(&usage(Some("qwen-max")));
        let doubao = estimate_model_cost(&usage(Some("doubao-seed-1.6")));

        assert_eq!(qwen.pricing_status, PricingStatus::Rule);
        assert_close(qwen.estimated_cost, 7.68);
        assert_eq!(doubao.pricing_status, PricingStatus::Rule);
        assert_close(doubao.estimated_cost, 2.54);
    }

    #[test]
    fn unknown_model_uses_average_fallback() {
        let cost = estimate_model_cost(&usage(Some("unknown-internal-model")));

        assert_eq!(cost.pricing_status, PricingStatus::Fallback);
        assert_close(cost.estimated_cost, 8.975);
    }

    #[test]
    fn missing_model_uses_average_fallback() {
        let cost = estimate_model_cost(&usage(None));

        assert_eq!(cost.pricing_status, PricingStatus::Fallback);
        assert_close(cost.estimated_cost, 8.975);
    }

    #[test]
    fn fallback_uses_total_tokens_when_components_are_missing() {
        let cost = estimate_model_cost(&ModelTokenUsage {
            model: Some("unknown-internal-model".to_string()),
            input_tokens: 0,
            output_tokens: 0,
            cached_input_tokens: 0,
            cache_creation_input_tokens: 0,
            reasoning_output_tokens: 0,
            total_tokens: 1_000_000,
        });

        assert_eq!(cost.pricing_status, PricingStatus::Fallback);
        assert_close(cost.estimated_cost, DEFAULT_AVERAGE_CNY_PER_1M_TOKENS);
    }

    #[test]
    fn combines_pricing_statuses_for_periods() {
        assert_eq!(combine_pricing_status(&[]), PricingStatus::Rule);
        assert_eq!(
            combine_pricing_status(&[PricingStatus::Rule]),
            PricingStatus::Rule
        );
        assert_eq!(
            combine_pricing_status(&[PricingStatus::Fallback]),
            PricingStatus::Fallback
        );
        assert_eq!(
            combine_pricing_status(&[PricingStatus::Rule, PricingStatus::Fallback]),
            PricingStatus::Mixed
        );
        assert_eq!(
            combine_pricing_status(&[PricingStatus::Mixed, PricingStatus::Rule]),
            PricingStatus::Mixed
        );
    }
}
