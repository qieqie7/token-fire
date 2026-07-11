export type ProfilePeriod = "today" | "this_week" | "this_month" | "this_year";

export interface BuildIdentity {
  version: string;
  git_commit: string | null;
  git_commit_short: string | null;
  build_time: string | null;
  dirty: boolean;
}

export interface ProfileSummary {
  generated_at: string;
  currency: "CNY";
  year_profile: YearProfileSummary;
  selected_period: PeriodProfileSummary;
}

export interface YearProfileSummary {
  days: ProfileDayBucket[];
  estimated_cost: number;
  total_tokens: number;
  active_days: number;
  average_active_day_cost: number;
  peak_day: ProfilePeakDay | null;
}

export interface ProfileDayBucket {
  local_date: string;
  estimated_cost: number;
  total_tokens: number;
  intensity: 0 | 1 | 2 | 3 | 4;
}

export interface ProfilePeakDay {
  local_date: string;
  estimated_cost: number;
  total_tokens: number;
}

export interface PeriodProfileSummary {
  period: ProfilePeriod;
  started_at: string;
  ended_at: string;
  estimated_cost: number;
  total_tokens: number;
  trend: PeriodUsageTrend;
  model_breakdown: RankedProfileBreakdown[];
  source_breakdown: RankedProfileBreakdown[];
  cost_drivers: ProfileCostDrivers;
}

export interface PeriodUsageTrend {
  unit: "hour" | "day" | "month";
  buckets: PeriodUsageTrendBucket[];
  x_ticks: PeriodUsageTrendTick[];
}

export interface PeriodUsageTrendBucket {
  key: string;
  label: string;
  started_at: string;
  ended_at: string;
  total_tokens: number | null;
  is_future: boolean;
}

export interface PeriodUsageTrendTick {
  bucket_key: string;
  label: string;
}

export interface RankedProfileBreakdown {
  key: string;
  label: string;
  estimated_cost: number;
  total_tokens: number;
  share: number;
}

export interface ProfileCostDrivers {
  input_cost: number;
  output_cost: number;
  reasoning_output_cost: number;
  cache_creation_input_cost: number;
  cached_input_cost: number;
  unattributed_cost: number;
  cached_input_tokens: number;
  cache_read_ratio: number;
}

export type ReleaseUpdateStatus =
  | { state: "unknown"; checked_at: null }
  | { state: "checking"; checked_at: string | null }
  | { state: "up_to_date"; checked_at: string; current_version: string; latest_version: string }
  | {
      state: "update_available";
      checked_at: string;
      current_version: string;
      current_commit_short: string | null;
      latest_version: string;
      latest_tag: string;
    }
  | {
      state: "failed";
      checked_at: string | null;
      reason: "network" | "rate_limited" | "http_status" | "invalid_response" | "invalid_version" | "non_stable_release";
    };
