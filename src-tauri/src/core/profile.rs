use chrono::{DateTime, Datelike, Days, Local, NaiveDate, TimeZone, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProfilePeriod {
    #[serde(rename = "today", alias = "1d", alias = "one_day")]
    Today,
    #[serde(rename = "this_week", alias = "1w", alias = "one_week")]
    ThisWeek,
    #[serde(rename = "this_month", alias = "1m", alias = "one_month")]
    ThisMonth,
    #[serde(rename = "this_year", alias = "1y", alias = "one_year")]
    ThisYear,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfileSummary {
    pub generated_at: DateTime<Utc>,
    pub currency: String,
    pub year_profile: YearProfileSummary,
    pub selected_period: PeriodProfileSummary,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct YearProfileSummary {
    pub days: Vec<ProfileDayBucket>,
    pub estimated_cost: f64,
    pub total_tokens: i64,
    pub active_days: usize,
    pub average_active_day_cost: f64,
    pub peak_day: Option<ProfilePeakDay>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfileDayBucket {
    pub local_date: NaiveDate,
    pub estimated_cost: f64,
    pub total_tokens: i64,
    pub intensity: u8,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfilePeakDay {
    pub local_date: NaiveDate,
    pub estimated_cost: f64,
    pub total_tokens: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PeriodProfileSummary {
    pub period: ProfilePeriod,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub estimated_cost: f64,
    pub total_tokens: i64,
    pub model_breakdown: Vec<RankedProfileBreakdown>,
    pub source_breakdown: Vec<RankedProfileBreakdown>,
    pub cost_drivers: ProfileCostDrivers,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RankedProfileBreakdown {
    pub key: String,
    pub label: String,
    pub estimated_cost: f64,
    pub total_tokens: i64,
    pub share: f64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct ProfileCostDrivers {
    pub input_cost: f64,
    pub output_cost: f64,
    pub reasoning_output_cost: f64,
    pub cache_creation_input_cost: f64,
    pub cached_input_cost: f64,
    pub unattributed_cost: f64,
    pub cached_input_tokens: i64,
    pub cache_read_ratio: f64,
}

fn local_midnight_utc(date: NaiveDate) -> anyhow::Result<DateTime<Utc>> {
    let midnight = date
        .and_hms_opt(0, 0, 0)
        .ok_or_else(|| anyhow::anyhow!("invalid local midnight date: {date}"))?;
    let local = Local
        .from_local_datetime(&midnight)
        .single()
        .ok_or_else(|| anyhow::anyhow!("ambiguous or invalid local midnight: {date}"))?;
    Ok(local.with_timezone(&Utc))
}

pub fn period_bounds(
    period: ProfilePeriod,
    now_utc: DateTime<Utc>,
    now_local: DateTime<Local>,
) -> anyhow::Result<(DateTime<Utc>, DateTime<Utc>)> {
    let end = now_utc;
    let start = match period {
        ProfilePeriod::Today => local_midnight_utc(now_local.date_naive())?,
        ProfilePeriod::ThisWeek => {
            let days_from_monday = now_local.weekday().num_days_from_monday() as u64;
            let start_date = now_local
                .date_naive()
                .checked_sub_days(Days::new(days_from_monday))
                .ok_or_else(|| anyhow::anyhow!("invalid local week start"))?;
            local_midnight_utc(start_date)?
        }
        ProfilePeriod::ThisMonth => {
            let start_date = NaiveDate::from_ymd_opt(now_local.year(), now_local.month(), 1)
                .ok_or_else(|| anyhow::anyhow!("invalid local month start"))?;
            local_midnight_utc(start_date)?
        }
        ProfilePeriod::ThisYear => {
            let start_date = NaiveDate::from_ymd_opt(now_local.year(), 1, 1)
                .ok_or_else(|| anyhow::anyhow!("invalid local year start"))?;
            local_midnight_utc(start_date)?
        }
    };
    Ok((start, end))
}

pub fn source_label(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "" => "Unknown".to_string(),
        "traex" => "TraeX".to_string(),
        "codex" => "Codex".to_string(),
        "claude" => "Claude".to_string(),
        "cursor" => "Cursor".to_string(),
        "gpt" => "GPT".to_string(),
        value => value.to_string(),
    }
}

pub fn model_label(raw: Option<&str>) -> String {
    let value = raw.unwrap_or("").trim();
    if value.is_empty() {
        "Unknown".to_string()
    } else {
        value.to_string()
    }
}

pub fn intensity_for_cost(cost: f64, max_cost: f64) -> u8 {
    if cost <= 0.0 || max_cost <= 0.0 || !cost.is_finite() || !max_cost.is_finite() {
        return 0;
    }
    let ratio = cost / max_cost;
    if ratio >= 0.75 {
        4
    } else if ratio >= 0.5 {
        3
    } else if ratio >= 0.25 {
        2
    } else {
        1
    }
}
