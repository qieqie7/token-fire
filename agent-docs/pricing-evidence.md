# TokenFire Pricing Evidence

Last reviewed: 2026-07-03

## How TokenFire Calculates Cost

TokenFire stores token observations, then computes cost at read time:

1. Query tracked observations for a time window.
2. Group rows by normalized `model`.
3. Sum normalized token categories for each model group.
4. Match the model group against static in-memory pricing rules.
5. Price each category and sum model costs.
6. Sum all model costs into the period total.
7. If a model has no matching rule, use `DEFAULT_AVERAGE_CNY_PER_1M_TOKENS`.

Frontend code does not calculate prices. It only displays backend values.

## Current Rules

Rules live in `src-tauri/src/core/pricing.rs`.

All USD prices are converted to CNY with `STATIC_USD_CNY_RATE_2026_07 = 7.25`.

| Rule | Match | Input / 1M | Cached input / 1M | Cache creation / 1M | Output / 1M | Reasoning output / 1M | Source |
| --- | --- | ---: | ---: | ---: | ---: | ---: | --- |
| `gpt-5.5` | prefix `gpt-5.5` | $5.00 | $0.50 | $5.00 | $30.00 | $30.00 | User-provided project baseline |
| `gpt-5.4` | prefix `gpt-5.4` | $2.50 | $0.25 | $2.50 | $15.00 | $15.00 | User-provided project baseline |
| `gpt-5` | prefix `gpt-5` | $1.25 | $0.125 | $1.25 | $10.00 | $10.00 | Public Azure/OpenAI-market references; OpenAI direct pricing page was not accessible for verification |
| `claude-sonnet` | prefix `claude-sonnet` | $3.00 | $0.30 | $3.75 | $15.00 | $15.00 | Anthropic official pricing |

## Evidence

### Anthropic Official Pricing

Source: `https://platform.claude.com/docs/en/about-claude/pricing?no_interactive=1`

The official pricing table lists columns:

- `Base Input Tokens`
- `5m Cache Writes`
- `1h Cache Writes`
- `Cache Hits & Refreshes`
- `Output Tokens`

Relevant rows:

- `Claude Sonnet 4.6`: `$3 / MTok`, `$3.75 / MTok`, `$6 / MTok`, `$0.30 / MTok`, `$15 / MTok`
- `Claude Sonnet 4.5`: `$3 / MTok`, `$3.75 / MTok`, `$6 / MTok`, `$0.30 / MTok`, `$15 / MTok`
- `Claude Sonnet 4`: `$3 / MTok`, `$3.75 / MTok`, `$6 / MTok`, `$0.30 / MTok`, `$15 / MTok`

TokenFire maps:

- `input_tokens` -> Base Input Tokens
- `cache_creation_input_tokens` -> 5m Cache Writes
- `cached_input_tokens` -> Cache Hits & Refreshes
- `output_tokens` -> Output Tokens
- `reasoning_output_tokens` -> Output Tokens

### OpenAI GPT-5 Family

OpenAI's direct pricing page `https://openai.com/api/pricing/` returned a Cloudflare challenge during this review, so it was not usable as direct evidence.

Web search results for `gpt-5.5` pricing were inconsistent across third-party pages, so TokenFire does not treat those pages as source of truth.

Current project baseline comes from the user:

- `GPT-5.5`: input `$5.00 / 1M`, cached input `$0.50 / 1M`, output `$30.00 / 1M`
- `GPT-5.4`: input `$2.50 / 1M`, cached input `$0.25 / 1M`, output `$15.00 / 1M`

For generic `gpt-5`, public Azure/OpenAI-market references consistently showed approximately input `$1.25 / 1M`, cached input `$0.125 / 1M`, output `$10.00 / 1M`. This is kept as the generic fallback rule for `gpt-5*` models not matched by a more specific rule.

## Known Limitations

- Enterprise discounts, batch pricing, priority/flex tiers, regional premiums, and provider-specific platform markups are not modeled.
- `reasoning_effort` is not persisted as a normalized field.
- OpenAI direct pricing needs a future manual re-check when the official page is accessible.
- These prices are estimates for local visibility, not provider billing settlement.
