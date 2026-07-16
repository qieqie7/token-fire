import { useState, type PointerEvent } from "react";
import { Popover } from "../design-system/Popover";
import type { ProfileDayBucket } from "./types";
import { formatCny, formatCompactTokens, formatLocalDate } from "./format";

export interface YearHeatmapProps {
  days: ProfileDayBucket[];
  activeDays: number;
  estimatedCost: number;
  totalTokens: number;
}

interface MonthTick {
  key: string;
  label: string;
  column: number;
}

export interface HeatmapDayReadout {
  title: string;
  value: string;
  meta: string;
  ariaLabel: string;
  empty: boolean;
}

export function formatHeatmapDayReadout(day: ProfileDayBucket): HeatmapDayReadout {
  const title = formatLocalDate(day.local_date);
  const value = `${formatCompactTokens(day.total_tokens)} token`;
  const meta = `估算 ${formatCny(day.estimated_cost)}`;
  const empty = day.total_tokens === 0;

  return {
    title,
    value,
    meta,
    ariaLabel: empty ? `${title} ${value}，${meta}，无用量` : `${title} ${value}，${meta}`,
    empty,
  };
}

function monthLabel(date: string): string {
  const month = Number(date.slice(5, 7));
  return month > 0 ? `${month} 月` : "";
}

function visibleDays(days: ProfileDayBucket[]): ProfileDayBucket[] {
  return days.slice(-365);
}

function weekdayOffsetFromMonday(date: string): number {
  const weekday = new Date(`${date}T00:00:00.000Z`).getUTCDay();
  return (weekday + 6) % 7;
}

function monthTicksFor(days: ProfileDayBucket[], leadingPlaceholderCount: number): MonthTick[] {
  const ticks: MonthTick[] = [];
  for (let index = 0; index < days.length; index += 1) {
    const day = days[index];
    if (!day.local_date.endsWith("-01")) continue;
    const column = Math.floor((leadingPlaceholderCount + index) / 7) + 1;
    const label = monthLabel(day.local_date);
    if (label) ticks.push({ key: day.local_date, label, column });
  }

  return ticks.filter((_, index) => index % 3 === 0).slice(0, 4);
}

export function YearHeatmap({ days, activeDays, estimatedCost, totalTokens }: YearHeatmapProps) {
  const displayDays = visibleDays(days);
  const leadingPlaceholderCount = displayDays[0]
    ? weekdayOffsetFromMonday(displayDays[0].local_date)
    : 0;
  const monthTicks = monthTicksFor(displayDays, leadingPlaceholderCount);
  const [activeDay, setActiveDay] = useState<ProfileDayBucket | null>(null);
  const [referenceElement, setReferenceElement] = useState<HTMLElement | null>(null);
  const activeReadout = activeDay ? formatHeatmapDayReadout(activeDay) : null;

  const activateDay = (day: ProfileDayBucket, element: HTMLElement) => {
    setActiveDay(day);
    setReferenceElement(element);
  };

  const clearActiveDay = () => {
    setActiveDay(null);
    setReferenceElement(null);
  };

  return (
    <section className="profile-panel profile-heatmap" aria-label="过去 365 天">
      <div className="profile-section-head">
        <span>过去 365 天</span>
        <span>活跃 {activeDays} 天</span>
      </div>
      <div className="profile-heatmap__days" role="img" aria-label="过去 365 天用量与估算成本日历热力图">
        {Array.from({ length: leadingPlaceholderCount }, (_, index) => (
          <span key={`placeholder-${index}`} className="profile-heatmap__placeholder" />
        ))}
        {displayDays.map((day) => {
          const readout = formatHeatmapDayReadout(day);
          return (
            <span
              key={day.local_date}
              className="profile-heatmap__day"
              data-intensity={day.intensity}
              data-active={activeDay?.local_date === day.local_date ? "true" : undefined}
              aria-label={readout.ariaLabel}
              onPointerEnter={(event: PointerEvent<HTMLSpanElement>) => activateDay(day, event.currentTarget)}
              onPointerMove={(event: PointerEvent<HTMLSpanElement>) => activateDay(day, event.currentTarget)}
              onPointerLeave={clearActiveDay}
            />
          );
        })}
      </div>
      <Popover
        open={Boolean(activeReadout)}
        reference={referenceElement}
        title={activeReadout?.title}
        content={
          activeReadout ? (
            <>
              {activeReadout.value}
              {activeReadout.empty ? <span className="tf-popover__meta">无用量</span> : null}
              <span className="tf-popover__meta">{activeReadout.meta}</span>
            </>
          ) : null
        }
      />
      <div className="profile-heatmap__months" aria-hidden="true">
        {monthTicks.map((tick) => (
          <span key={tick.key} style={{ gridColumn: tick.column }}>{tick.label}</span>
        ))}
      </div>
      <div className="profile-heatmap__summary">
        <span>{formatCompactTokens(totalTokens)} token</span>
        <span>估算 {formatCny(estimatedCost)}</span>
      </div>
    </section>
  );
}
