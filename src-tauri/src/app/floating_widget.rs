use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::app::status::{status_label, UiStatus};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WidgetState {
    pub today_total_tokens: i64,
    pub latest_turn_delta_tokens: i64,
    pub status: UiStatus,
    pub status_label: String,
    pub state_revision: i64,
    pub last_observed_at: Option<DateTime<Utc>>,
}

impl WidgetState {
    pub fn new(today_total_tokens: i64, latest_turn_delta_tokens: i64, status: UiStatus) -> Self {
        Self::new_with_revision(
            today_total_tokens,
            latest_turn_delta_tokens,
            status,
            0,
            None,
        )
    }

    pub fn new_with_revision(
        today_total_tokens: i64,
        latest_turn_delta_tokens: i64,
        status: UiStatus,
        state_revision: i64,
        last_observed_at: Option<DateTime<Utc>>,
    ) -> Self {
        Self {
            today_total_tokens,
            latest_turn_delta_tokens,
            status,
            status_label: status_label(status).to_string(),
            state_revision,
            last_observed_at,
        }
    }
}
