import type { ProfilePeriod } from "./types";

const periods: Array<{ value: ProfilePeriod; label: string }> = [
  { value: "today", label: "当日" },
  { value: "this_week", label: "当周" },
  { value: "this_month", label: "当月" },
  { value: "this_year", label: "当年" },
];

export interface PeriodFilterProps {
  period: ProfilePeriod;
  onChange: (period: ProfilePeriod) => void;
}

export function PeriodFilter({ period, onChange }: PeriodFilterProps) {
  return (
    <div className="profile-period-filter" role="tablist" aria-label="所选周期">
      {periods.map((item) => (
        <button
          key={item.value}
          type="button"
          role="tab"
          aria-selected={period === item.value}
          className="profile-period-filter__button"
          onClick={() => onChange(item.value)}
        >
          {item.label}
        </button>
      ))}
    </div>
  );
}
