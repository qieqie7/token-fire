use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageTotals {
    pub today_total_tokens: i64,
    pub latest_turn_delta_tokens: i64,
}
