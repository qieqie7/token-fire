# TokenFire Pricing Evidence

Last reviewed: 2026-07-10

## How TokenFire Calculates Cost

TokenFire stores token observations, then computes cost at read time:

1. Query tracked observations for a time window.
2. Group rows by normalized `model`.
3. Sum normalized token categories for each model group.
4. Match the model group against static in-memory pricing rules.
5. If a matching rule exists and component token fields exist, price each category with that rule.
6. If no matching rule exists and component token fields exist, price each category with `FALLBACK_PRICING_RULE`.
7. If component token fields are all zero, price `total_tokens` with `DEFAULT_AVERAGE_CNY_PER_1M_TOKENS`.
8. Sum all model costs into the period total.

Frontend code does not calculate prices. It only displays backend values.

Component token fields are `input_tokens`, `output_tokens`, `cached_input_tokens`,
`cache_creation_input_tokens`, and `reasoning_output_tokens`. TokenFire treats a
component-token row as priced even when the model is unknown, because
`FALLBACK_PRICING_RULE` carries default component rates:

- input: `DEFAULT_INPUT_CNY_PER_1M_TOKENS`
- output: `DEFAULT_OUTPUT_CNY_PER_1M_TOKENS`
- cached input: `DEFAULT_CACHED_INPUT_CNY_PER_1M_TOKENS`
- cache creation input: `DEFAULT_CACHE_CREATION_INPUT_CNY_PER_1M_TOKENS`
- reasoning output: `DEFAULT_REASONING_OUTPUT_CNY_PER_1M_TOKENS`

Only rows without component token evidence use the single blended average
`DEFAULT_AVERAGE_CNY_PER_1M_TOKENS` against `total_tokens`.

## Current Rules

Rules live in `src-tauri/src/core/pricing.rs`.

All USD prices are converted to CNY with `STATIC_USD_CNY_RATE_2026_07 = 7.25`.

| Rule | Match | Input / 1M | Cached input / 1M | Cache creation / 1M | Output / 1M | Reasoning output / 1M | Evidence |
| --- | --- | ---: | ---: | ---: | ---: | ---: | --- |
| `gpt-5.6-sol` | keywords `gpt`, `5.6`, `sol` | $5.00 | $0.50 | $6.25 | $30.00 | $30.00 | OpenAI official pricing |
| `gpt-5.6-terra` | keywords `gpt`, `5.6`, `terra` | $2.50 | $0.25 | $3.125 | $15.00 | $15.00 | OpenAI official pricing |
| `gpt-5.6-luna` | keywords `gpt`, `5.6`, `luna` | $1.00 | $0.10 | $1.25 | $6.00 | $6.00 | OpenAI official pricing |
| `gpt-5.5-pro` | keywords `gpt`, `5.5`, `pro` | $30.00 | input fallback | input fallback | $180.00 | $180.00 | OpenAI official pricing |
| `gpt-5.5` | keywords `gpt`, `5.5`; excludes `pro` | $5.00 | $0.50 | input fallback | $30.00 | $30.00 | OpenAI official pricing |
| `gpt-5.4-pro` | keywords `gpt`, `5.4`, `pro` | $30.00 | input fallback | input fallback | $180.00 | $180.00 | OpenAI official pricing |
| `gpt-5.4-mini` | keywords `gpt`, `5.4`, `mini` | $0.75 | $0.075 | input fallback | $4.50 | $4.50 | OpenAI official pricing |
| `gpt-5.4-nano` | keywords `gpt`, `5.4`, `nano` | $0.20 | $0.02 | input fallback | $1.25 | $1.25 | OpenAI official pricing |
| `gpt-5.4` | keywords `gpt`, `5.4`; excludes `mini`, `nano`, `pro` | $2.50 | $0.25 | input fallback | $15.00 | $15.00 | OpenAI official pricing |
| `gpt-5.2-pro` | keywords `gpt`, `5.2`, `pro` | $21.00 | input fallback | input fallback | $168.00 | $168.00 | OpenAI official pricing |
| `gpt-5.2` | keywords `gpt`, `5.2`; excludes `pro` | $1.75 | $0.175 | input fallback | $14.00 | $14.00 | OpenAI official pricing |
| `gpt-5.1` | keywords `gpt`, `5.1` | $1.25 | $0.125 | input fallback | $10.00 | $10.00 | OpenAI official pricing |
| `gpt-5-pro` | keywords `gpt`, `5`, `pro`; excludes future `5.x` | $15.00 | input fallback | input fallback | $120.00 | $120.00 | OpenAI official pricing |
| `gpt-5-mini` | keywords `gpt`, `5`, `mini`; excludes future `5.x` | $0.25 | $0.025 | input fallback | $2.00 | $2.00 | OpenAI official pricing |
| `gpt-5-nano` | keywords `gpt`, `5`, `nano`; excludes future `5.x` | $0.05 | $0.005 | input fallback | $0.40 | $0.40 | OpenAI official pricing |
| `gpt-5` | keywords `gpt`, `5`; excludes `mini`, `nano`, `pro`, and future `5.x` | $1.25 | $0.125 | input fallback | $10.00 | $10.00 | OpenAI official pricing |
| `gpt-4.1-mini` | keywords `gpt`, `4.1`, `mini` | $0.40 | $0.10 | input fallback | $1.60 | $1.60 | OpenAI official pricing |
| `gpt-4.1-nano` | keywords `gpt`, `4.1`, `nano` | $0.10 | $0.025 | input fallback | $0.40 | $0.40 | OpenAI official pricing |
| `gpt-4.1` | keywords `gpt`, `4.1`; excludes `mini`, `nano` | $2.00 | $0.50 | input fallback | $8.00 | $8.00 | OpenAI official pricing |
| `gpt-4o-2024-05-13` | keywords `gpt`, `4o`, `2024`, `05`, `13`; excludes `mini` | $5.00 | input fallback | input fallback | $15.00 | $15.00 | OpenAI official pricing |
| `gpt-4o-mini` | keywords `gpt`, `4o`, `mini` | $0.15 | $0.075 | input fallback | $0.60 | $0.60 | OpenAI official pricing |
| `gpt-4o` | keywords `gpt`, `4o`; excludes `mini` | $2.50 | $1.25 | input fallback | $10.00 | $10.00 | OpenAI official pricing |
| `o1-pro` | keywords `o1`, `pro` | $150.00 | input fallback | input fallback | $600.00 | $600.00 | OpenAI official pricing |
| `o1-mini` | keywords `o1`, `mini` | $1.10 | $0.55 | input fallback | $4.40 | $4.40 | OpenAI official pricing |
| `o1` | keyword `o1`; excludes `mini`, `pro` | $15.00 | $7.50 | input fallback | $60.00 | $60.00 | OpenAI official pricing |
| `o3-pro` | keywords `o3`, `pro` | $20.00 | input fallback | input fallback | $80.00 | $80.00 | OpenAI official pricing |
| `o3-mini` | keywords `o3`, `mini` | $1.10 | $0.55 | input fallback | $4.40 | $4.40 | OpenAI official pricing |
| `o3` | keyword `o3`; excludes `mini`, `pro`, `deep`, `research` | $2.00 | $0.50 | input fallback | $8.00 | $8.00 | OpenAI official pricing |
| `o4-mini` | keywords `o4`, `mini` | $1.10 | $0.275 | input fallback | $4.40 | $4.40 | OpenAI official pricing |
| `claude-fable-5` | keywords `claude`, `fable`, `5` | $10.00 | $1.00 | $12.50 | $50.00 | $50.00 | Anthropic official pricing |
| `claude-mythos-5` | keywords `claude`, `mythos`, `5`; excludes `preview` | $10.00 | $1.00 | $12.50 | $50.00 | $50.00 | Anthropic official pricing |
| `claude-opus-4-8` | keywords `claude`, `opus`, `4.8` | $5.00 | $0.50 | $6.25 | $25.00 | $25.00 | Anthropic official pricing |
| `claude-opus-4-7` | keywords `claude`, `opus`, `4.7` | $5.00 | $0.50 | $6.25 | $25.00 | $25.00 | Anthropic official pricing |
| `claude-opus-4-6` | keywords `claude`, `opus`, `4.6` | $5.00 | $0.50 | $6.25 | $25.00 | $25.00 | Anthropic official pricing |
| `claude-opus-4-5` | keywords `claude`, `opus`, `4.5` | $5.00 | $0.50 | $6.25 | $25.00 | $25.00 | Anthropic official pricing |
| `claude-sonnet-5` | keywords `claude`, `sonnet`, `5` | $2.00 | $0.20 | $2.50 | $10.00 | $10.00 | Anthropic introductory pricing |
| `claude-sonnet-4-6` | keywords `claude`, `sonnet`, `4.6` | $3.00 | $0.30 | $3.75 | $15.00 | $15.00 | Anthropic official pricing |
| `claude-sonnet-4-5` | keywords `claude`, `sonnet`, `4.5` | $3.00 | $0.30 | $3.75 | $15.00 | $15.00 | Anthropic official pricing |
| `claude-sonnet` | keywords `claude`, `sonnet`; excludes `5`, `4.6`, `4.5`; matcher guard rejects numeric major versions `6+` | $3.00 | $0.30 | $3.75 | $15.00 | $15.00 | Anthropic official pricing |
| `claude-haiku-4-5` | keywords `claude`, `haiku`, `4.5` | $1.00 | $0.10 | $1.25 | $5.00 | $5.00 | Anthropic official pricing |
| `claude-haiku-3-5` | keywords `claude`, `haiku`, `3.5` | $0.80 | $0.08 | $1.00 | $4.00 | $4.00 | Anthropic official pricing |
| `kimi-k2.6` | keywords `kimi`, `2.6` | $0.95 | $0.16 | $0.95 | $4.00 | $4.00 | Provider public pricing |
| `kimi-k2.5` | keywords `kimi`, `2.5` | $0.60 | $0.10 | $0.60 | $3.00 | $3.00 | Provider public pricing |
| `gemini-2.5-flash` | keywords `gemini`, `2.5`, `flash` | $0.30 | $0.075 | $0.30 | $2.50 | $2.50 | Provider public pricing |
| `gemini-2.5-pro` | keywords `gemini`, `2.5`, `pro` | $1.25 | $0.3125 | $1.25 | $10.00 | $10.00 | Provider public pricing |
| `deepseek-v4-flash` | keywords `deepseek`, `v4`, `flash` | $0.14 | $0.0028 | $0.14 | $0.28 | $0.28 | Provider public pricing |
| `deepseek-v4-pro` | keywords `deepseek`, `v4`, `pro` | $0.435 | $0.0036 | $0.435 | $0.87 | $0.87 | Provider public pricing |
| `qwen3-max` | keywords `qwen3`, `max`; rejects `qwen3.x` future minors | CNY 3.00 | CNY 3.00 | CNY 3.00 | CNY 12.00 | CNY 12.00 | Provider public pricing |
| `qwen-max` | keywords `qwen`, `max`; excludes `qwen3`; matcher guard rejects Qwen major versions `4+` adjacent to `qwen` | CNY 2.40 | CNY 2.40 | CNY 2.40 | CNY 9.60 | CNY 9.60 | Provider public pricing |
| `doubao-seed-1.6` | keywords `doubao`, `seed`, `1.6` | CNY 0.40 | CNY 0.16 | CNY 0.40 | CNY 4.00 | CNY 4.00 | Provider public pricing |
| `fallback-component-rates` | no matching rule with component token fields | CNY 3.00 | CNY 0.50 | CNY 3.00 | CNY 12.00 | CNY 12.00 | TokenFire default component rates |

Rows without component token fields use `DEFAULT_AVERAGE_CNY_PER_1M_TOKENS`
against `total_tokens` instead of the component-rate fallback.

## Evidence

### Official Sources

- OpenAI pricing: `https://platform.openai.com/docs/pricing.md`
- Anthropic pricing: `https://docs.anthropic.com/en/docs/about-claude/pricing.md`
- Anthropic model overview: `https://docs.anthropic.com/en/docs/about-claude/models/overview.md`
- Anthropic model IDs and versions: `https://docs.anthropic.com/en/docs/about-claude/models/model-ids-and-versions.md`

### OpenAI Official Pricing

OpenAI's pricing source lists model families, token categories, and standard
input/output/cache prices. TokenFire stores those static prices in
`PRICING_RULES` and converts USD prices to CNY at read time.

The GPT 5.6 tier rules intentionally require a tier keyword:

- `gpt-5.6-sol`
- `gpt-5.6-terra`
- `gpt-5.6-luna`

Ambiguous `gpt 5.6` does not map to any tier and falls back to
`FALLBACK_PRICING_RULE` when component tokens exist.

Future version strings are rejected until reviewed. For example, `gpt-5.7` and
`gpt-5.7-pro` fall back instead of inheriting `gpt-5` prices.

### Anthropic Official Pricing

Anthropic's pricing source lists columns:

- `Base Input Tokens`
- `5m Cache Writes`
- `1h Cache Writes`
- `Cache Hits & Refreshes`
- `Output Tokens`

TokenFire maps:

- `input_tokens` -> Base Input Tokens
- `cache_creation_input_tokens` -> 5m Cache Writes
- `cached_input_tokens` -> Cache Hits & Refreshes
- `output_tokens` -> Output Tokens
- `reasoning_output_tokens` -> Output Tokens

Relevant Anthropic rows:

- `claude-sonnet-5`: introductory input `$2 / MTok`, 5m cache write `$2.50 / MTok`, cache hit `$0.20 / MTok`, output `$10 / MTok`
- `claude-sonnet-4-6` and `claude-sonnet-4-5`: input `$3 / MTok`, 5m cache write `$3.75 / MTok`, cache hit `$0.30 / MTok`, output `$15 / MTok`
- `claude-haiku-4-5`: input `$1 / MTok`, 5m cache write `$1.25 / MTok`, cache hit `$0.10 / MTok`, output `$5 / MTok`
- `claude-opus-4.x`: input `$5 / MTok`, 5m cache write `$6.25 / MTok`, cache hit `$0.50 / MTok`, output `$25 / MTok`
- `claude-fable-5` and `claude-mythos-5`: input `$10 / MTok`, 5m cache write `$12.50 / MTok`, cache hit `$1.00 / MTok`, output `$50 / MTok`

`claude-sonnet-5` introductory pricing expires after 2026-08-31 and must be
revisited before 2026-09-01.

### Keyword Matching Examples

Model matching tokenizes provider prefixes, separators, case variants, and
alpha-numeric runs. Matches must satisfy all required keywords and cannot hit a
forbidden keyword, forbidden prefix, or provider-aware future-version guard.

| Input model string | Result |
| --- | --- |
| `openai/gpt-5.6-sol-20260709` | `gpt-5.6-sol` |
| `gpt 5.6` | fallback |
| `gpt-5.7` | fallback |
| `gpt-4o-2024-08-06` | `gpt-4o` |
| `anthropic/claude-sonnet-5` | `claude-sonnet-5` |
| `claude sonnet 6` | fallback |
| `claude-sonnet-10` | fallback |
| `kimi 2.6` | `kimi-k2.6` |
| `qwen-max-2025-01-25` | `qwen-max` |
| `qwen3.5-max` | fallback |
| `qwen4-max` | fallback |
| `qwen10-max` | fallback |
| `qwen-10-max` | fallback |

## Known Limitations

- `claude-sonnet-5` introductory pricing expires after 2026-08-31 and must be revisited before 2026-09-01.
- Claude 1-hour cache writes cannot be distinguished from 5-minute cache writes in the current schema.
- `None` cached-input/cache-write prices fall back to input price by current formula.
- Enterprise discounts, batch pricing, priority/flex tiers, regional premiums, and provider-specific platform markups are not modeled.
- `reasoning_effort` is not persisted as a normalized field.
- These prices are estimates for local visibility, not provider billing settlement.

## Review Notes

Code confirmed:

- `estimate_model_cost_breakdown` uses `find_rule`; unknown models receive `FALLBACK_PRICING_RULE`.
- `has_component_tokens` gates category pricing. If all component token fields are zero, cost uses `DEFAULT_AVERAGE_CNY_PER_1M_TOKENS` against `total_tokens`.
- `rate_to_cny` converts USD-denominated rules with `STATIC_USD_CNY_RATE_2026_07`; CNY-denominated rules are used directly.

Not verified in this documentation task:

- No live network re-check was performed.
- Provider pricing may change after this review date.
