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
    pub trend: PeriodUsageTrend,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PeriodTrendUnit {
    #[serde(rename = "hour")]
    Hour,
    #[serde(rename = "day")]
    Day,
    #[serde(rename = "month")]
    Month,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PeriodUsageTrend {
    pub unit: PeriodTrendUnit,
    pub buckets: Vec<PeriodUsageTrendBucket>,
    pub x_ticks: Vec<PeriodUsageTrendTick>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PeriodUsageTrendBucket {
    pub key: String,
    pub label: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub total_tokens: Option<i64>,
    pub is_future: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeriodUsageTrendTick {
    pub bucket_key: String,
    pub label: String,
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

fn local_datetime_utc(date: NaiveDate, hour: u32) -> anyhow::Result<DateTime<Utc>> {
    let local_time = date
        .and_hms_opt(hour, 0, 0)
        .ok_or_else(|| anyhow::anyhow!("invalid local datetime: {date} {hour}:00:00"))?;
    let local = match Local.from_local_datetime(&local_time) {
        chrono::LocalResult::Single(value) => value,
        chrono::LocalResult::Ambiguous(earliest, _) => earliest,
        chrono::LocalResult::None => Local
            .from_local_datetime(&(local_time + chrono::Duration::hours(1)))
            .earliest()
            .ok_or_else(|| {
                anyhow::anyhow!("invalid local datetime near DST gap: {date} {hour}:00:00")
            })?,
    };
    Ok(local.with_timezone(&Utc))
}

// Repeated DST hours use the earliest instant; skipped DST hours use the next
// available instant. The UI still keeps 24 local hour labels.

fn next_month_start(year: i32, month: u32) -> anyhow::Result<NaiveDate> {
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    NaiveDate::from_ymd_opt(next_year, next_month, 1)
        .ok_or_else(|| anyhow::anyhow!("invalid next month start: {next_year}-{next_month}-01"))
}

fn last_day_of_month(year: i32, month: u32) -> anyhow::Result<NaiveDate> {
    next_month_start(year, month)?
        .checked_sub_days(Days::new(1))
        .ok_or_else(|| anyhow::anyhow!("invalid last day of month: {year}-{month}"))
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

pub fn empty_period_usage_trend(
    period: ProfilePeriod,
    now_utc: DateTime<Utc>,
    now_local: DateTime<Local>,
) -> anyhow::Result<PeriodUsageTrend> {
    let mut buckets = Vec::new();
    let mut x_ticks = Vec::new();

    match period {
        ProfilePeriod::Today => {
            let local_date = now_local.date_naive();
            for hour in 0..24_u32 {
                let started_at = local_datetime_utc(local_date, hour)?;
                let ended_at = if hour == 23 {
                    local_midnight_utc(local_date + Days::new(1))?
                } else {
                    local_datetime_utc(local_date, hour + 1)?
                };
                let is_future = started_at > now_utc;
                let key = format!("h{hour:02}");
                if matches!(hour, 0 | 6 | 12 | 18 | 23) {
                    x_ticks.push(PeriodUsageTrendTick {
                        bucket_key: key.clone(),
                        label: hour.to_string(),
                    });
                }
                buckets.push(PeriodUsageTrendBucket {
                    key,
                    label: hour.to_string(),
                    started_at,
                    ended_at,
                    total_tokens: (!is_future).then_some(0),
                    is_future,
                });
            }
            Ok(PeriodUsageTrend {
                unit: PeriodTrendUnit::Hour,
                buckets,
                x_ticks,
            })
        }
        ProfilePeriod::ThisWeek => {
            let days_from_monday = now_local.weekday().num_days_from_monday() as u64;
            let monday = now_local
                .date_naive()
                .checked_sub_days(Days::new(days_from_monday))
                .ok_or_else(|| anyhow::anyhow!("invalid local week start"))?;
            let labels = ["一", "二", "三", "四", "五", "六", "日"];
            for day_index in 0..7_usize {
                let local_date = monday + Days::new(day_index as u64);
                let started_at = local_midnight_utc(local_date)?;
                let ended_at = local_midnight_utc(local_date + Days::new(1))?;
                let is_future = started_at > now_utc;
                let key = format!("d{day_index}");
                x_ticks.push(PeriodUsageTrendTick {
                    bucket_key: key.clone(),
                    label: labels[day_index].to_string(),
                });
                buckets.push(PeriodUsageTrendBucket {
                    key,
                    label: labels[day_index].to_string(),
                    started_at,
                    ended_at,
                    total_tokens: (!is_future).then_some(0),
                    is_future,
                });
            }
            Ok(PeriodUsageTrend {
                unit: PeriodTrendUnit::Day,
                buckets,
                x_ticks,
            })
        }
        ProfilePeriod::ThisMonth => {
            let year = now_local.year();
            let month = now_local.month();
            let first_day = NaiveDate::from_ymd_opt(year, month, 1)
                .ok_or_else(|| anyhow::anyhow!("invalid local month start"))?;
            let last_day = last_day_of_month(year, month)?;
            let month_len = last_day.day();
            for day in 1..=month_len {
                let local_date = first_day + Days::new((day - 1) as u64);
                let started_at = local_midnight_utc(local_date)?;
                let ended_at = local_midnight_utc(local_date + Days::new(1))?;
                let is_future = started_at > now_utc;
                let key = format!("d{day:02}");
                if day == 1 || day == 10 || day == 20 || day == month_len {
                    let label = if day == month_len {
                        "月末".to_string()
                    } else {
                        day.to_string()
                    };
                    if !x_ticks.iter().any(|tick| tick.bucket_key == key) {
                        x_ticks.push(PeriodUsageTrendTick {
                            bucket_key: key.clone(),
                            label,
                        });
                    }
                }
                buckets.push(PeriodUsageTrendBucket {
                    key,
                    label: day.to_string(),
                    started_at,
                    ended_at,
                    total_tokens: (!is_future).then_some(0),
                    is_future,
                });
            }
            Ok(PeriodUsageTrend {
                unit: PeriodTrendUnit::Day,
                buckets,
                x_ticks,
            })
        }
        ProfilePeriod::ThisYear => {
            let year = now_local.year();
            for month in 1..=12_u32 {
                let local_date = NaiveDate::from_ymd_opt(year, month, 1)
                    .ok_or_else(|| anyhow::anyhow!("invalid local month bucket"))?;
                let started_at = local_midnight_utc(local_date)?;
                let ended_at = local_midnight_utc(next_month_start(year, month)?)?;
                let is_future = started_at > now_utc;
                let key = format!("m{month:02}");
                if matches!(month, 1 | 4 | 7 | 10 | 12) {
                    x_ticks.push(PeriodUsageTrendTick {
                        bucket_key: key.clone(),
                        label: if month == 12 {
                            "12月".to_string()
                        } else {
                            month.to_string()
                        },
                    });
                }
                buckets.push(PeriodUsageTrendBucket {
                    key,
                    label: month.to_string(),
                    started_at,
                    ended_at,
                    total_tokens: (!is_future).then_some(0),
                    is_future,
                });
            }
            Ok(PeriodUsageTrend {
                unit: PeriodTrendUnit::Month,
                buckets,
                x_ticks,
            })
        }
    }
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
