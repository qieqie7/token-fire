use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

pub const WIDGET_USAGE_WINDOW_MINUTES: i64 = 360;
pub const WIDGET_USAGE_BUCKET_MINUTES: i64 = 30;
pub const WIDGET_USAGE_ACTIVE_THRESHOLD_MINUTES: i64 = 5;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WidgetUsageBucket {
    pub start_at: DateTime<Utc>,
    pub end_at: DateTime<Utc>,
    pub total_tokens: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WidgetUsageSeries {
    pub window_minutes: i64,
    pub bucket_minutes: i64,
    pub generated_at: DateTime<Utc>,
    pub state_revision: i64,
    pub average_tokens_per_bucket: f64,
    pub latest_bucket_tokens: i64,
    pub latest_bucket_active: bool,
    pub buckets: Vec<WidgetUsageBucket>,
    pub previous_day_buckets: Vec<WidgetUsageBucket>,
}

pub fn empty_usage_buckets(now: DateTime<Utc>) -> Vec<WidgetUsageBucket> {
    let bucket_duration = Duration::minutes(WIDGET_USAGE_BUCKET_MINUTES);
    let window_start = now - Duration::minutes(WIDGET_USAGE_WINDOW_MINUTES);
    (0..(WIDGET_USAGE_WINDOW_MINUTES / WIDGET_USAGE_BUCKET_MINUTES))
        .map(|index| {
            let start_at = window_start + bucket_duration * index as i32;
            WidgetUsageBucket {
                start_at,
                end_at: start_at + bucket_duration,
                total_tokens: 0,
            }
        })
        .collect()
}

pub fn bucket_index_for(observed_at: DateTime<Utc>, now: DateTime<Utc>) -> Option<usize> {
    let window_start = now - Duration::minutes(WIDGET_USAGE_WINDOW_MINUTES);
    if observed_at < window_start || observed_at >= now {
        return None;
    }
    let elapsed = observed_at - window_start;
    Some(
        (elapsed.num_seconds() / Duration::minutes(WIDGET_USAGE_BUCKET_MINUTES).num_seconds())
            as usize,
    )
}

pub fn average_tokens_per_bucket(buckets: &[WidgetUsageBucket]) -> f64 {
    if buckets.is_empty() {
        return 0.0;
    }
    let total: i64 = buckets.iter().map(|bucket| bucket.total_tokens).sum();
    total as f64 / buckets.len() as f64
}
