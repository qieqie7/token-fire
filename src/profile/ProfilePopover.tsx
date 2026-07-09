import type { BuildIdentity, ProfilePeriod, ProfileSummary } from "./types";
import { AppSources } from "./AppSources";
import { MetricPair } from "./MetricPair";
import { PeriodFilter } from "./PeriodFilter";
import { TopModels } from "./TopModels";
import { YearHeatmap } from "./YearHeatmap";
import "./profile.css";

export interface ProfilePopoverProps {
  period: ProfilePeriod;
  summary: ProfileSummary | null;
  loading: boolean;
  error: boolean;
  buildIdentity: BuildIdentity | null;
  onPeriodChange: (period: ProfilePeriod) => void;
}

export function formatBuildIdentityLabel(identity: BuildIdentity): string {
  const parts = [`v${identity.version}`];
  if (identity.git_commit_short) parts.push(identity.git_commit_short);
  if (identity.dirty) parts.push("dirty");
  return parts.join(" · ");
}

export function ProfilePopover({
  period,
  summary,
  loading,
  error,
  buildIdentity,
  onPeriodChange,
}: ProfilePopoverProps) {
  const year = summary?.year_profile ?? null;
  const selectedPeriod = summary?.selected_period ?? null;
  const buildIdentityLabel = buildIdentity ? formatBuildIdentityLabel(buildIdentity) : null;

  return (
    <main className="profile-popover">
      <header className="profile-header">
        <div className="profile-brand">
          <span className="profile-brand__flame" />
          <span>TokenFire</span>
        </div>
        {buildIdentityLabel ? (
          <span className="profile-version" title={`TokenFire ${buildIdentityLabel}`}>
            {buildIdentityLabel}
          </span>
        ) : null}
      </header>

      {year ? (
        <YearHeatmap
          days={year.days}
          activeDays={year.active_days}
          estimatedCost={year.estimated_cost}
          totalTokens={year.total_tokens}
        />
      ) : (
        <section className="profile-panel profile-empty-heatmap">
          <div className="profile-section-head">
            <span>过去 365 天</span>
            <span>活跃 0 天</span>
          </div>
          <div className="profile-empty-heatmap__grid" />
        </section>
      )}

      <div className="profile-section-head">
        <span>所选周期</span>
        <span>{loading ? "加载中" : error ? "不可用" : "筛选下方数据"}</span>
      </div>
      <PeriodFilter period={period} onChange={onPeriodChange} />
      <MetricPair period={selectedPeriod} />
      <div className="profile-attribution-grid">
        <AppSources rows={selectedPeriod?.source_breakdown ?? []} />
        <TopModels rows={selectedPeriod?.model_breakdown ?? []} />
      </div>
    </main>
  );
}
