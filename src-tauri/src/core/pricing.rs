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

/// 15 分钟 rollup 桶的聚合定价入参。
///
/// 与 `ModelTokenUsage` 的关键差异：`billable_uncached_input_tokens` 和
/// `unattributed_total_tokens` 都是逐行预聚合量。聚合定价必须直接使用它们，
/// 不得再从分量 clamp 或由 `total_tokens` 反推——否则与 raw 逐行定价不等价。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AggregatedModelTokenUsage {
    pub model: Option<String>,
    pub input_tokens: i64,
    pub billable_uncached_input_tokens: i64,
    pub output_tokens: i64,
    pub cached_input_tokens: i64,
    pub cache_creation_input_tokens: i64,
    pub reasoning_output_tokens: i64,
    pub unattributed_total_tokens: i64,
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
    Keywords {
        required: &'static [&'static str],
        forbidden: &'static [&'static str],
        forbidden_prefixes: &'static [&'static str],
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct MatchScore {
    kind: u8,
    required_count: u8,
    version_specificity: u8,
    priority: u32,
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

const fn usd_rule(
    id: &'static str,
    priority: u32,
    model_match: ModelMatch,
    input_per_1m: f64,
    cached_input_per_1m: Option<f64>,
    cache_creation_input_per_1m: Option<f64>,
    output_per_1m: f64,
) -> PricingRule {
    PricingRule {
        id,
        priority,
        model_match,
        currency: Currency::Usd,
        input_per_1m,
        output_per_1m,
        cached_input_per_1m,
        cache_creation_input_per_1m,
        reasoning_output_per_1m: Some(output_per_1m),
    }
}

const fn cny_rule(
    id: &'static str,
    priority: u32,
    model_match: ModelMatch,
    input_per_1m: f64,
    cached_input_per_1m: Option<f64>,
    cache_creation_input_per_1m: Option<f64>,
    output_per_1m: f64,
) -> PricingRule {
    PricingRule {
        id,
        priority,
        model_match,
        currency: Currency::Cny,
        input_per_1m,
        output_per_1m,
        cached_input_per_1m,
        cache_creation_input_per_1m,
        reasoning_output_per_1m: Some(output_per_1m),
    }
}

const PRICING_RULES: &[PricingRule] = &[
    usd_rule(
        "gpt-5.6-sol",
        160,
        ModelMatch::Keywords {
            required: &["gpt", "5.6", "sol"],
            forbidden: &[],
            forbidden_prefixes: &[],
        },
        5.00,
        Some(0.50),
        Some(6.25),
        30.00,
    ),
    usd_rule(
        "gpt-5.6-terra",
        159,
        ModelMatch::Keywords {
            required: &["gpt", "5.6", "terra"],
            forbidden: &[],
            forbidden_prefixes: &[],
        },
        2.50,
        Some(0.25),
        Some(3.125),
        15.00,
    ),
    usd_rule(
        "gpt-5.6-luna",
        158,
        ModelMatch::Keywords {
            required: &["gpt", "5.6", "luna"],
            forbidden: &[],
            forbidden_prefixes: &[],
        },
        1.00,
        Some(0.10),
        Some(1.25),
        6.00,
    ),
    usd_rule(
        "gpt-5.5-pro",
        157,
        ModelMatch::Keywords {
            required: &["gpt", "5.5", "pro"],
            forbidden: &[],
            forbidden_prefixes: &[],
        },
        30.00,
        None,
        None,
        180.00,
    ),
    usd_rule(
        "gpt-5.5",
        156,
        ModelMatch::Keywords {
            required: &["gpt", "5.5"],
            forbidden: &["pro"],
            forbidden_prefixes: &[],
        },
        5.00,
        Some(0.50),
        None,
        30.00,
    ),
    usd_rule(
        "gpt-5.4-pro",
        155,
        ModelMatch::Keywords {
            required: &["gpt", "5.4", "pro"],
            forbidden: &[],
            forbidden_prefixes: &[],
        },
        30.00,
        None,
        None,
        180.00,
    ),
    usd_rule(
        "gpt-5.4-mini",
        154,
        ModelMatch::Keywords {
            required: &["gpt", "5.4", "mini"],
            forbidden: &[],
            forbidden_prefixes: &[],
        },
        0.75,
        Some(0.075),
        None,
        4.50,
    ),
    usd_rule(
        "gpt-5.4-nano",
        153,
        ModelMatch::Keywords {
            required: &["gpt", "5.4", "nano"],
            forbidden: &[],
            forbidden_prefixes: &[],
        },
        0.20,
        Some(0.02),
        None,
        1.25,
    ),
    usd_rule(
        "gpt-5.4",
        152,
        ModelMatch::Keywords {
            required: &["gpt", "5.4"],
            forbidden: &["mini", "nano", "pro"],
            forbidden_prefixes: &[],
        },
        2.50,
        Some(0.25),
        None,
        15.00,
    ),
    usd_rule(
        "gpt-5.2-pro",
        151,
        ModelMatch::Keywords {
            required: &["gpt", "5.2", "pro"],
            forbidden: &[],
            forbidden_prefixes: &[],
        },
        21.00,
        None,
        None,
        168.00,
    ),
    usd_rule(
        "gpt-5.2",
        150,
        ModelMatch::Keywords {
            required: &["gpt", "5.2"],
            forbidden: &["pro"],
            forbidden_prefixes: &[],
        },
        1.75,
        Some(0.175),
        None,
        14.00,
    ),
    usd_rule(
        "gpt-5.1",
        149,
        ModelMatch::Keywords {
            required: &["gpt", "5.1"],
            forbidden: &[],
            forbidden_prefixes: &[],
        },
        1.25,
        Some(0.125),
        None,
        10.00,
    ),
    usd_rule(
        "gpt-5-pro",
        148,
        ModelMatch::Keywords {
            required: &["gpt", "5", "pro"],
            forbidden: &[],
            forbidden_prefixes: &["5."],
        },
        15.00,
        None,
        None,
        120.00,
    ),
    usd_rule(
        "gpt-5-mini",
        147,
        ModelMatch::Keywords {
            required: &["gpt", "5", "mini"],
            forbidden: &[],
            forbidden_prefixes: &["5."],
        },
        0.25,
        Some(0.025),
        None,
        2.00,
    ),
    usd_rule(
        "gpt-5-nano",
        146,
        ModelMatch::Keywords {
            required: &["gpt", "5", "nano"],
            forbidden: &[],
            forbidden_prefixes: &["5."],
        },
        0.05,
        Some(0.005),
        None,
        0.40,
    ),
    usd_rule(
        "gpt-5",
        145,
        ModelMatch::Keywords {
            required: &["gpt", "5"],
            forbidden: &["mini", "nano", "pro"],
            forbidden_prefixes: &["5."],
        },
        1.25,
        Some(0.125),
        None,
        10.00,
    ),
    usd_rule(
        "gpt-4.1-mini",
        144,
        ModelMatch::Keywords {
            required: &["gpt", "4.1", "mini"],
            forbidden: &[],
            forbidden_prefixes: &[],
        },
        0.40,
        Some(0.10),
        None,
        1.60,
    ),
    usd_rule(
        "gpt-4.1-nano",
        143,
        ModelMatch::Keywords {
            required: &["gpt", "4.1", "nano"],
            forbidden: &[],
            forbidden_prefixes: &[],
        },
        0.10,
        Some(0.025),
        None,
        0.40,
    ),
    usd_rule(
        "gpt-4.1",
        142,
        ModelMatch::Keywords {
            required: &["gpt", "4.1"],
            forbidden: &["mini", "nano"],
            forbidden_prefixes: &[],
        },
        2.00,
        Some(0.50),
        None,
        8.00,
    ),
    usd_rule(
        "gpt-4o-2024-05-13",
        141,
        ModelMatch::Keywords {
            required: &["gpt", "4o", "2024", "05", "13"],
            forbidden: &["mini"],
            forbidden_prefixes: &[],
        },
        5.00,
        None,
        None,
        15.00,
    ),
    usd_rule(
        "gpt-4o-mini",
        140,
        ModelMatch::Keywords {
            required: &["gpt", "4o", "mini"],
            forbidden: &[],
            forbidden_prefixes: &[],
        },
        0.15,
        Some(0.075),
        None,
        0.60,
    ),
    usd_rule(
        "gpt-4o",
        139,
        ModelMatch::Keywords {
            required: &["gpt", "4o"],
            forbidden: &["mini"],
            forbidden_prefixes: &[],
        },
        2.50,
        Some(1.25),
        None,
        10.00,
    ),
    usd_rule(
        "o1-pro",
        138,
        ModelMatch::Keywords {
            required: &["o1", "pro"],
            forbidden: &[],
            forbidden_prefixes: &[],
        },
        150.00,
        None,
        None,
        600.00,
    ),
    usd_rule(
        "o1-mini",
        137,
        ModelMatch::Keywords {
            required: &["o1", "mini"],
            forbidden: &[],
            forbidden_prefixes: &[],
        },
        1.10,
        Some(0.55),
        None,
        4.40,
    ),
    usd_rule(
        "o1",
        136,
        ModelMatch::Keywords {
            required: &["o1"],
            forbidden: &["mini", "pro"],
            forbidden_prefixes: &[],
        },
        15.00,
        Some(7.50),
        None,
        60.00,
    ),
    usd_rule(
        "o3-pro",
        135,
        ModelMatch::Keywords {
            required: &["o3", "pro"],
            forbidden: &[],
            forbidden_prefixes: &[],
        },
        20.00,
        None,
        None,
        80.00,
    ),
    usd_rule(
        "o3-mini",
        134,
        ModelMatch::Keywords {
            required: &["o3", "mini"],
            forbidden: &[],
            forbidden_prefixes: &[],
        },
        1.10,
        Some(0.55),
        None,
        4.40,
    ),
    usd_rule(
        "o3",
        133,
        ModelMatch::Keywords {
            required: &["o3"],
            forbidden: &["mini", "pro", "deep", "research"],
            forbidden_prefixes: &[],
        },
        2.00,
        Some(0.50),
        None,
        8.00,
    ),
    usd_rule(
        "o4-mini",
        132,
        ModelMatch::Keywords {
            required: &["o4", "mini"],
            forbidden: &[],
            forbidden_prefixes: &[],
        },
        1.10,
        Some(0.275),
        None,
        4.40,
    ),
    usd_rule(
        "claude-fable-5",
        120,
        ModelMatch::Keywords {
            required: &["claude", "fable", "5"],
            forbidden: &[],
            forbidden_prefixes: &[],
        },
        10.00,
        Some(1.00),
        Some(12.50),
        50.00,
    ),
    usd_rule(
        "claude-mythos-5",
        119,
        ModelMatch::Keywords {
            required: &["claude", "mythos", "5"],
            forbidden: &["preview"],
            forbidden_prefixes: &[],
        },
        10.00,
        Some(1.00),
        Some(12.50),
        50.00,
    ),
    usd_rule(
        "claude-opus-4-8",
        118,
        ModelMatch::Keywords {
            required: &["claude", "opus", "4.8"],
            forbidden: &[],
            forbidden_prefixes: &[],
        },
        5.00,
        Some(0.50),
        Some(6.25),
        25.00,
    ),
    usd_rule(
        "claude-opus-4-7",
        117,
        ModelMatch::Keywords {
            required: &["claude", "opus", "4.7"],
            forbidden: &[],
            forbidden_prefixes: &[],
        },
        5.00,
        Some(0.50),
        Some(6.25),
        25.00,
    ),
    usd_rule(
        "claude-opus-4-6",
        116,
        ModelMatch::Keywords {
            required: &["claude", "opus", "4.6"],
            forbidden: &[],
            forbidden_prefixes: &[],
        },
        5.00,
        Some(0.50),
        Some(6.25),
        25.00,
    ),
    usd_rule(
        "claude-opus-4-5",
        115,
        ModelMatch::Keywords {
            required: &["claude", "opus", "4.5"],
            forbidden: &[],
            forbidden_prefixes: &[],
        },
        5.00,
        Some(0.50),
        Some(6.25),
        25.00,
    ),
    // Introductory pricing is valid through 2026-08-31; revisit before 2026-09-01.
    usd_rule(
        "claude-sonnet-5",
        114,
        ModelMatch::Keywords {
            required: &["claude", "sonnet", "5"],
            forbidden: &[],
            forbidden_prefixes: &[],
        },
        2.00,
        Some(0.20),
        Some(2.50),
        10.00,
    ),
    usd_rule(
        "claude-sonnet-4-6",
        113,
        ModelMatch::Keywords {
            required: &["claude", "sonnet", "4.6"],
            forbidden: &[],
            forbidden_prefixes: &[],
        },
        3.00,
        Some(0.30),
        Some(3.75),
        15.00,
    ),
    usd_rule(
        "claude-sonnet-4-5",
        112,
        ModelMatch::Keywords {
            required: &["claude", "sonnet", "4.5"],
            forbidden: &[],
            forbidden_prefixes: &[],
        },
        3.00,
        Some(0.30),
        Some(3.75),
        15.00,
    ),
    usd_rule(
        "claude-sonnet",
        111,
        ModelMatch::Keywords {
            required: &["claude", "sonnet"],
            forbidden: &["5", "4.6", "4.5"],
            forbidden_prefixes: &[],
        },
        3.00,
        Some(0.30),
        Some(3.75),
        15.00,
    ),
    usd_rule(
        "claude-haiku-4-5",
        110,
        ModelMatch::Keywords {
            required: &["claude", "haiku", "4.5"],
            forbidden: &[],
            forbidden_prefixes: &[],
        },
        1.00,
        Some(0.10),
        Some(1.25),
        5.00,
    ),
    usd_rule(
        "claude-haiku-3-5",
        109,
        ModelMatch::Keywords {
            required: &["claude", "haiku", "3.5"],
            forbidden: &[],
            forbidden_prefixes: &[],
        },
        0.80,
        Some(0.08),
        Some(1.00),
        4.00,
    ),
    usd_rule(
        "kimi-k2.6",
        90,
        ModelMatch::Keywords {
            required: &["kimi", "2.6"],
            forbidden: &[],
            forbidden_prefixes: &[],
        },
        0.95,
        Some(0.16),
        Some(0.95),
        4.00,
    ),
    usd_rule(
        "kimi-k2.5",
        89,
        ModelMatch::Keywords {
            required: &["kimi", "2.5"],
            forbidden: &[],
            forbidden_prefixes: &[],
        },
        0.60,
        Some(0.10),
        Some(0.60),
        3.00,
    ),
    usd_rule(
        "gemini-2.5-flash",
        88,
        ModelMatch::Keywords {
            required: &["gemini", "2.5", "flash"],
            forbidden: &[],
            forbidden_prefixes: &[],
        },
        0.30,
        Some(0.075),
        Some(0.30),
        2.50,
    ),
    usd_rule(
        "gemini-2.5-pro",
        87,
        ModelMatch::Keywords {
            required: &["gemini", "2.5", "pro"],
            forbidden: &[],
            forbidden_prefixes: &[],
        },
        1.25,
        Some(0.3125),
        Some(1.25),
        10.00,
    ),
    usd_rule(
        "deepseek-v4-flash",
        86,
        ModelMatch::Keywords {
            required: &["deepseek", "v4", "flash"],
            forbidden: &[],
            forbidden_prefixes: &[],
        },
        0.14,
        Some(0.0028),
        Some(0.14),
        0.28,
    ),
    usd_rule(
        "deepseek-v4-pro",
        85,
        ModelMatch::Keywords {
            required: &["deepseek", "v4", "pro"],
            forbidden: &[],
            forbidden_prefixes: &[],
        },
        0.435,
        Some(0.0036),
        Some(0.435),
        0.87,
    ),
    cny_rule(
        "qwen3-max",
        84,
        ModelMatch::Keywords {
            required: &["qwen3", "max"],
            forbidden: &[],
            forbidden_prefixes: &["qwen3.", "3."],
        },
        3.00,
        Some(3.00),
        Some(3.00),
        12.00,
    ),
    cny_rule(
        "qwen-max",
        83,
        ModelMatch::Keywords {
            required: &["qwen", "max"],
            forbidden: &["qwen3"],
            forbidden_prefixes: &[],
        },
        2.40,
        Some(2.40),
        Some(2.40),
        9.60,
    ),
    cny_rule(
        "doubao-seed-1.6",
        82,
        ModelMatch::Keywords {
            required: &["doubao", "seed", "1.6"],
            forbidden: &[],
            forbidden_prefixes: &[],
        },
        0.40,
        Some(0.16),
        Some(0.40),
        4.00,
    ),
];

const FALLBACK_PRICING_RULE: PricingRule = PricingRule {
    id: "fallback-component-rates",
    priority: 0,
    model_match: ModelMatch::Prefix(""),
    currency: Currency::Cny,
    input_per_1m: DEFAULT_INPUT_CNY_PER_1M_TOKENS,
    output_per_1m: DEFAULT_OUTPUT_CNY_PER_1M_TOKENS,
    cached_input_per_1m: Some(DEFAULT_CACHED_INPUT_CNY_PER_1M_TOKENS),
    cache_creation_input_per_1m: Some(DEFAULT_CACHE_CREATION_INPUT_CNY_PER_1M_TOKENS),
    reasoning_output_per_1m: Some(DEFAULT_REASONING_OUTPUT_CNY_PER_1M_TOKENS),
};

fn tokenize_model(model: &str) -> Vec<String> {
    let lower = model.to_ascii_lowercase();
    let segments: Vec<&str> = lower
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|segment| !segment.is_empty())
        .collect();

    let mut tokens = Vec::new();
    for segment in &segments {
        push_unique_token(&mut tokens, segment);
        if let Some((alpha, numeric)) = split_alpha_numeric(segment) {
            push_unique_token(&mut tokens, alpha);
            push_unique_token(&mut tokens, numeric);
        }
    }

    for pair in segments.windows(2) {
        let left = pair[0];
        let right = pair[1];
        if is_short_numeric(left) && is_short_numeric(right) {
            push_unique_token(&mut tokens, &format!("{left}.{right}"));
        }
        if is_alpha_token(left) && is_short_numeric(right) {
            push_unique_token(&mut tokens, &format!("{left}{right}"));
        }
        if let Some((_alpha, numeric)) = split_alpha_numeric(left) {
            if is_short_numeric(numeric) && is_short_numeric(right) {
                push_unique_token(&mut tokens, &format!("{left}.{right}"));
                push_unique_token(&mut tokens, &format!("{numeric}.{right}"));
            }
        }
    }

    tokens
}

fn push_unique_token(tokens: &mut Vec<String>, token: &str) {
    if !tokens.iter().any(|existing| existing == token) {
        tokens.push(token.to_string());
    }
}

fn split_alpha_numeric(segment: &str) -> Option<(&str, &str)> {
    let split_at = segment
        .char_indices()
        .find_map(|(index, ch)| ch.is_ascii_digit().then_some(index))?;
    if split_at == 0 {
        return None;
    }
    let (alpha, numeric) = segment.split_at(split_at);
    if alpha.chars().all(|ch| ch.is_ascii_alphabetic())
        && numeric.chars().all(|ch| ch.is_ascii_digit())
    {
        Some((alpha, numeric))
    } else {
        None
    }
}

fn is_short_numeric(value: &str) -> bool {
    !value.is_empty() && value.len() <= 4 && value.chars().all(|ch| ch.is_ascii_digit())
}

fn is_alpha_token(value: &str) -> bool {
    !value.is_empty() && value.chars().all(|ch| ch.is_ascii_alphabetic())
}

fn has_token(tokens: &[String], expected: &str) -> bool {
    tokens.iter().any(|token| token == expected)
}

fn has_token_with_prefix(tokens: &[String], prefix: &str) -> bool {
    tokens.iter().any(|token| token.starts_with(prefix))
}

fn has_prefixed_major_version_at_least(tokens: &[String], prefix: &str, minimum: u16) -> bool {
    tokens
        .iter()
        .filter_map(|token| token.strip_prefix(prefix))
        .filter_map(parse_major_version_token)
        .any(|version| version >= minimum)
}

fn parse_major_version_token(token: &str) -> Option<u16> {
    if token.is_empty() || token.len() > 3 || !token.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    token.parse().ok()
}

fn hits_future_version_guard(rule_id: &str, tokens: &[String]) -> bool {
    // Guard broad legacy rules from inheriting prices for unreviewed future majors.
    match rule_id {
        "claude-sonnet" => has_prefixed_major_version_at_least(tokens, "sonnet", 6),
        "qwen-max" => has_prefixed_major_version_at_least(tokens, "qwen", 4),
        _ => false,
    }
}

fn version_specificity(required: &[&str]) -> u8 {
    required
        .iter()
        .filter(|token| token.chars().any(|ch| ch == '.'))
        .count() as u8
}

fn prefix_matches(model: &str, prefix: &str) -> bool {
    model
        .get(..prefix.len())
        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(prefix))
}

fn match_score(rule: PricingRule, model: &str, tokens: &[String]) -> Option<MatchScore> {
    match rule.model_match {
        ModelMatch::Exact(expected) if model.eq_ignore_ascii_case(expected) => Some(MatchScore {
            kind: 3,
            required_count: u8::MAX,
            version_specificity: u8::MAX,
            priority: rule.priority,
        }),
        ModelMatch::Exact(_) => None,
        ModelMatch::Keywords {
            required,
            forbidden,
            forbidden_prefixes,
        } => {
            let required_matches = required.iter().all(|expected| has_token(tokens, expected));
            let forbidden_matches = forbidden.iter().any(|blocked| has_token(tokens, blocked));
            let prefix_matches = forbidden_prefixes
                .iter()
                .any(|prefix| has_token_with_prefix(tokens, prefix));
            let future_version_matches = hits_future_version_guard(rule.id, tokens);

            if required_matches && !forbidden_matches && !prefix_matches && !future_version_matches
            {
                Some(MatchScore {
                    kind: 2,
                    required_count: required.len() as u8,
                    version_specificity: version_specificity(required),
                    priority: rule.priority,
                })
            } else {
                None
            }
        }
        ModelMatch::Prefix(prefix) if prefix_matches(model, prefix) => Some(MatchScore {
            kind: 1,
            required_count: 0,
            version_specificity: 0,
            priority: rule.priority,
        }),
        ModelMatch::Prefix(_) => None,
    }
}

fn find_rule(model: Option<&str>) -> Option<PricingRule> {
    let model = model?;
    let tokens = tokenize_model(model);
    PRICING_RULES
        .iter()
        .copied()
        .filter_map(|rule| match_score(rule, model, &tokens).map(|score| (rule, score)))
        .max_by_key(|(_rule, score)| *score)
        .map(|(rule, _score)| rule)
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

/// 聚合桶定价入口。与逐行 raw 定价等价的前提：
/// - 只解析一次 pricing rule / fallback；
/// - 直接使用预聚合的 `billable_uncached_input_tokens`，不再从分量 clamp；
/// - `unattributed_total_tokens` 按平均价计入 `unattributed_cost`；
/// - `total_tokens` 仅作展示总量，不反推分量。
pub fn estimate_aggregated_model_cost_breakdown(
    usage: &AggregatedModelTokenUsage,
) -> PricedModelCostBreakdown {
    let (rule, pricing_status) = match find_rule(usage.model.as_deref()) {
        Some(rule) => (rule, PricingStatus::Rule),
        None => (FALLBACK_PRICING_RULE, PricingStatus::Fallback),
    };

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

    let drivers = PricedCostDrivers {
        // 用预聚合的 billable 值直接定价，不再 clamp——逐行 clamp 已在聚合阶段完成。
        input_cost: price_tokens(usage.billable_uncached_input_tokens, input),
        output_cost: price_tokens(usage.output_tokens, output),
        reasoning_output_cost: price_tokens(usage.reasoning_output_tokens, reasoning_output),
        cache_creation_input_cost: price_tokens(usage.cache_creation_input_tokens, cache_creation),
        cached_input_cost: price_tokens(usage.cached_input_tokens, cached_input),
        // 只有原始 total-only 观测才进 unattributed，按平均价计价。
        unattributed_cost: price_tokens(
            usage.unattributed_total_tokens,
            DEFAULT_AVERAGE_CNY_PER_1M_TOKENS,
        ),
        cached_input_tokens: usage.cached_input_tokens.max(0),
        input_tokens: usage.input_tokens.max(0),
    };

    PricedModelCostBreakdown {
        estimated_cost: drivers.input_cost
            + drivers.output_cost
            + drivers.reasoning_output_cost
            + drivers.cache_creation_input_cost
            + drivers.cached_input_cost
            + drivers.unattributed_cost,
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

    fn rule_id_for(model: &str) -> Option<&'static str> {
        find_rule(Some(model)).map(|rule| rule.id)
    }

    fn assert_rule_id(model: &str, expected: &'static str) {
        assert_eq!(rule_id_for(model), Some(expected), "model: {model}");
    }

    fn expected_usage_cost_usd(
        input_per_1m: f64,
        cached_input_per_1m: Option<f64>,
        cache_creation_input_per_1m: Option<f64>,
        output_per_1m: f64,
    ) -> f64 {
        let cached_input_per_1m = cached_input_per_1m.unwrap_or(input_per_1m);
        let cache_creation_input_per_1m = cache_creation_input_per_1m.unwrap_or(input_per_1m);
        let usd = 0.65 * input_per_1m
            + 0.25 * cached_input_per_1m
            + 0.10 * cache_creation_input_per_1m
            + 0.50 * output_per_1m
            + 0.05 * output_per_1m;
        usd * STATIC_USD_CNY_RATE_2026_07
    }

    fn assert_rule_cost(model: &str, expected: f64) {
        let cost = estimate_model_cost(&usage(Some(model)));
        assert_eq!(cost.pricing_status, PricingStatus::Rule, "model: {model}");
        assert_close(cost.estimated_cost, expected);
    }

    fn test_rule(id: &'static str, priority: u32, model_match: ModelMatch) -> PricingRule {
        PricingRule {
            id,
            priority,
            model_match,
            currency: Currency::Usd,
            input_per_1m: 1.0,
            output_per_1m: 1.0,
            cached_input_per_1m: Some(1.0),
            cache_creation_input_per_1m: Some(1.0),
            reasoning_output_per_1m: Some(1.0),
        }
    }

    fn assert_has_token(tokens: &[String], expected: &str) {
        assert!(
            tokens.iter().any(|token| token == expected),
            "expected token {expected:?} in {tokens:?}"
        );
    }

    #[test]
    fn tokenize_model_preserves_provider_prefixes_versions_and_compact_tokens() {
        let gpt = tokenize_model("openai/GPT-5.6-Sol-20260709");
        assert_has_token(&gpt, "openai");
        assert_has_token(&gpt, "gpt");
        assert_has_token(&gpt, "5.6");
        assert_has_token(&gpt, "sol");
        assert_has_token(&gpt, "20260709");

        let kimi = tokenize_model("moonshot/kimi-k2.6");
        assert_has_token(&kimi, "kimi");
        assert_has_token(&kimi, "k2.6");
        assert_has_token(&kimi, "2.6");

        let qwen = tokenize_model("qwen3-max-latest");
        assert_has_token(&qwen, "qwen3");
        assert_has_token(&qwen, "qwen");
        assert_has_token(&qwen, "3");
        assert_has_token(&qwen, "max");
    }

    #[test]
    fn keyword_match_blocks_forbidden_tokens_and_prefixes() {
        let rule = test_rule(
            "gpt-5",
            100,
            ModelMatch::Keywords {
                required: &["gpt", "5"],
                forbidden: &["mini", "nano", "pro"],
                forbidden_prefixes: &["5."],
            },
        );

        let plain = "gpt-5";
        let plain_tokens = tokenize_model(plain);
        assert!(match_score(rule, plain, &plain_tokens).is_some());

        let pro = "gpt-5-pro";
        let pro_tokens = tokenize_model(pro);
        assert!(match_score(rule, pro, &pro_tokens).is_none());

        let future = "gpt-5.7";
        let future_tokens = tokenize_model(future);
        assert!(match_score(rule, future, &future_tokens).is_none());
    }

    #[test]
    fn keyword_match_ranking_prefers_more_specific_rules_over_priority_only() {
        let generic = test_rule(
            "gpt-5",
            200,
            ModelMatch::Keywords {
                required: &["gpt", "5"],
                forbidden: &[],
                forbidden_prefixes: &[],
            },
        );
        let specific = test_rule(
            "gpt-5.6-sol",
            100,
            ModelMatch::Keywords {
                required: &["gpt", "5.6", "sol"],
                forbidden: &[],
                forbidden_prefixes: &[],
            },
        );

        let model = "openai/gpt-5.6-sol-20260709";
        let tokens = tokenize_model(model);
        let generic_score = match_score(generic, model, &tokens).expect("generic score");
        let specific_score = match_score(specific, model, &tokens).expect("specific score");

        assert!(specific_score > generic_score);
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
    fn gpt_5_6_rules_match_tier_keywords_and_reject_unknown_tier() {
        assert_rule_id("openai/gpt-5.6-sol-20260709", "gpt-5.6-sol");
        assert_rule_id("GPT 5.6 Sol", "gpt-5.6-sol");
        assert_rule_id("gpt_5_6_terra", "gpt-5.6-terra");
        assert_rule_id("gpt-5.6-luna", "gpt-5.6-luna");

        let ambiguous = estimate_model_cost(&usage(Some("gpt 5.6")));
        assert_eq!(ambiguous.pricing_status, PricingStatus::Fallback);
        assert_close(ambiguous.estimated_cost, 8.975);
    }

    #[test]
    fn gpt_5_6_rules_use_official_standard_prices() {
        assert_rule_cost(
            "openai/gpt-5.6-sol-20260709",
            expected_usage_cost_usd(5.0, Some(0.5), Some(6.25), 30.0),
        );
        assert_rule_cost(
            "gpt-5.6-terra",
            expected_usage_cost_usd(2.5, Some(0.25), Some(3.125), 15.0),
        );
        assert_rule_cost(
            "gpt-5.6-luna",
            expected_usage_cost_usd(1.0, Some(0.1), Some(1.25), 6.0),
        );
    }

    #[test]
    fn gpt_5_family_rules_match_known_variants_and_reject_future_versions() {
        assert_rule_id("gpt-5.5-pro", "gpt-5.5-pro");
        assert_rule_id("gpt-5.5", "gpt-5.5");
        assert_rule_id("gpt-5.4-mini", "gpt-5.4-mini");
        assert_rule_id("gpt-5.4-nano", "gpt-5.4-nano");
        assert_rule_id("gpt-5.4-pro", "gpt-5.4-pro");
        assert_rule_id("gpt-5.4", "gpt-5.4");
        assert_rule_id("gpt-5.2-pro", "gpt-5.2-pro");
        assert_rule_id("gpt-5.2", "gpt-5.2");
        assert_rule_id("gpt-5.1", "gpt-5.1");
        assert_rule_id("gpt-5-pro", "gpt-5-pro");
        assert_rule_id("gpt-5-mini", "gpt-5-mini");
        assert_rule_id("gpt-5-nano", "gpt-5-nano");
        assert_rule_id("gpt-5", "gpt-5");

        assert_eq!(rule_id_for("gpt-5.7"), None);
        assert_eq!(rule_id_for("gpt-5.7-pro"), None);
    }

    #[test]
    fn gpt_5_family_rules_keep_existing_regression_costs() {
        assert_rule_cost("gpt-5.5", 147.71875);
        assert_rule_cost("gpt-5.4", 73.859375);
        assert_rule_cost(
            "gpt-5.5-pro",
            expected_usage_cost_usd(30.0, None, None, 180.0),
        );
    }

    #[test]
    fn gpt_4_and_o_series_rules_match_known_variants() {
        assert_rule_id("gpt-4.1-mini", "gpt-4.1-mini");
        assert_rule_id("gpt-4.1-nano", "gpt-4.1-nano");
        assert_rule_id("gpt-4.1", "gpt-4.1");
        assert_rule_id("gpt-4o-2024-08-06", "gpt-4o");
        assert_rule_id("gpt-4o-2024-05-13", "gpt-4o-2024-05-13");
        assert_rule_id("gpt-4o-mini", "gpt-4o-mini");
        assert_rule_id("gpt-4o-mini-2024-05-13", "gpt-4o-mini");
        assert_rule_id("gpt-4o", "gpt-4o");
        assert_rule_id("o1-pro", "o1-pro");
        assert_rule_id("o1-mini", "o1-mini");
        assert_rule_id("o1", "o1");
        assert_rule_id("o3-pro", "o3-pro");
        assert_rule_id("o3-mini", "o3-mini");
        assert_rule_id("o3", "o3");
        assert_rule_id("o4-mini", "o4-mini");
    }

    #[test]
    fn gpt_4_and_o_series_rules_use_official_standard_prices() {
        assert_rule_cost(
            "gpt-4o-mini",
            expected_usage_cost_usd(0.15, Some(0.075), None, 0.6),
        );
        assert_rule_cost(
            "o4-mini",
            expected_usage_cost_usd(1.1, Some(0.275), None, 4.4),
        );
    }

    #[test]
    fn existing_provider_rules_match_flexible_ids_and_reject_future_qwen_versions() {
        assert_rule_id("moonshot/kimi 2.6", "kimi-k2.6");
        assert_rule_id("kimi-k2.5", "kimi-k2.5");
        assert_rule_id("google/gemini 2.5 flash", "gemini-2.5-flash");
        assert_rule_id("gemini-2.5-pro", "gemini-2.5-pro");
        assert_rule_id("deepseek v4 flash", "deepseek-v4-flash");
        assert_rule_id("deepseek-v4-pro", "deepseek-v4-pro");
        assert_rule_id("qwen3-max-latest", "qwen3-max");
        assert_rule_id("qwen-max-latest", "qwen-max");
        assert_rule_id("qwen-max-2025-01-25", "qwen-max");
        assert_rule_id("doubao seed 1.6", "doubao-seed-1.6");

        assert_eq!(rule_id_for("qwen3.5-max"), None);
        assert_eq!(rule_id_for("qwen4-max"), None);
        assert_eq!(rule_id_for("qwen10-max"), None);
        assert_eq!(rule_id_for("qwen-10-max"), None);
    }

    #[test]
    fn claude_rules_match_known_models_and_reject_unknown_versions() {
        assert_rule_id("anthropic/claude-fable-5", "claude-fable-5");
        assert_rule_id("claude mythos 5", "claude-mythos-5");
        assert_rule_id("claude-opus-4-8", "claude-opus-4-8");
        assert_rule_id("claude-opus-4-7", "claude-opus-4-7");
        assert_rule_id("claude-opus-4-6", "claude-opus-4-6");
        assert_rule_id("claude-opus-4-5", "claude-opus-4-5");
        assert_rule_id("anthropic/claude-sonnet-5", "claude-sonnet-5");
        assert_rule_id("claude sonnet 4.6", "claude-sonnet-4-6");
        assert_rule_id("claude-sonnet-4-5-20250929", "claude-sonnet-4-5");
        assert_rule_id("claude-haiku-4-5-20251001", "claude-haiku-4-5");
        assert_rule_id("claude-haiku-3-5", "claude-haiku-3-5");

        assert_eq!(rule_id_for("claude mythos preview"), None);
        assert_eq!(rule_id_for("claude sonnet 6"), None);
        assert_eq!(rule_id_for("claude-sonnet-10"), None);
    }

    #[test]
    fn claude_rules_use_official_prices_with_sonnet_5_introductory_until_2026_08_31() {
        assert_rule_cost(
            "claude-fable-5",
            expected_usage_cost_usd(10.0, Some(1.0), Some(12.5), 50.0),
        );
        assert_rule_cost(
            "claude-opus-4-8",
            expected_usage_cost_usd(5.0, Some(0.5), Some(6.25), 25.0),
        );
        assert_rule_cost(
            "claude-sonnet-5",
            expected_usage_cost_usd(2.0, Some(0.2), Some(2.5), 10.0),
        );
        assert_rule_cost(
            "claude-haiku-4-5",
            expected_usage_cost_usd(1.0, Some(0.1), Some(1.25), 5.0),
        );
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
    fn unknown_model_uses_component_fallback_rates() {
        let cost = estimate_model_cost(&usage(Some("unknown-internal-model")));

        assert_eq!(cost.pricing_status, PricingStatus::Fallback);
        assert_close(cost.estimated_cost, 8.975);
    }

    #[test]
    fn missing_model_uses_component_fallback_rates() {
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
    fn aggregated_pricing_matches_per_row_raw_for_mixed_bucket() {
        // 一条 component + 一条 total-only 落同 model：聚合定价必须逐分量、
        // unattributed 与总额都等于两条 raw 分别定价再求和。
        let component = ModelTokenUsage {
            model: Some("gpt-5.5".to_string()),
            input_tokens: 1_000_000,
            output_tokens: 500_000,
            cached_input_tokens: 250_000,
            cache_creation_input_tokens: 100_000,
            reasoning_output_tokens: 50_000,
            total_tokens: 1_900_000,
        };
        let total_only = ModelTokenUsage {
            model: Some("gpt-5.5".to_string()),
            input_tokens: 0,
            output_tokens: 0,
            cached_input_tokens: 0,
            cache_creation_input_tokens: 0,
            reasoning_output_tokens: 0,
            total_tokens: 1_000_000,
        };
        let raw_component = estimate_model_cost_breakdown(&component);
        let raw_total_only = estimate_model_cost_breakdown(&total_only);

        // 聚合：billable 逐行 clamp 后求和；unattributed 只来自 total-only 行。
        let aggregated = estimate_aggregated_model_cost_breakdown(&AggregatedModelTokenUsage {
            model: Some("gpt-5.5".to_string()),
            input_tokens: 1_000_000,
            billable_uncached_input_tokens: (1_000_000i64 - 250_000 - 100_000).max(0),
            output_tokens: 500_000,
            cached_input_tokens: 250_000,
            cache_creation_input_tokens: 100_000,
            reasoning_output_tokens: 50_000,
            unattributed_total_tokens: 1_000_000,
            total_tokens: 2_900_000,
        });

        assert_eq!(aggregated.pricing_status, PricingStatus::Rule);
        assert_close(
            aggregated.drivers.input_cost,
            raw_component.drivers.input_cost + raw_total_only.drivers.input_cost,
        );
        assert_close(
            aggregated.drivers.unattributed_cost,
            raw_component.drivers.unattributed_cost + raw_total_only.drivers.unattributed_cost,
        );
        assert_close(
            aggregated.estimated_cost,
            raw_component.estimated_cost + raw_total_only.estimated_cost,
        );
        assert_eq!(aggregated.total_tokens, 2_900_000);
    }

    #[test]
    fn aggregated_pricing_uses_fallback_for_unknown_model() {
        let aggregated = estimate_aggregated_model_cost_breakdown(&AggregatedModelTokenUsage {
            model: Some("unknown-internal-model".to_string()),
            input_tokens: 0,
            billable_uncached_input_tokens: 0,
            output_tokens: 0,
            cached_input_tokens: 0,
            cache_creation_input_tokens: 0,
            reasoning_output_tokens: 0,
            unattributed_total_tokens: 1_000_000,
            total_tokens: 1_000_000,
        });

        assert_eq!(aggregated.pricing_status, PricingStatus::Fallback);
        assert_close(
            aggregated.drivers.unattributed_cost,
            DEFAULT_AVERAGE_CNY_PER_1M_TOKENS,
        );
        assert_close(aggregated.estimated_cost, DEFAULT_AVERAGE_CNY_PER_1M_TOKENS);
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
