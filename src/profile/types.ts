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
  model_breakdown: RankedProfileBreakdown[];
  source_breakdown: RankedProfileBreakdown[];
  cost_drivers: ProfileCostDrivers;
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
