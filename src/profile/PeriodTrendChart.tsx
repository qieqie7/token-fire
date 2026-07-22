import { useState, type PointerEvent } from "react";
import { Popover } from "../design-system/Popover";
import type { PeriodUsageTrend, PeriodUsageTrendBucket } from "./types";
import { formatCompactTokens } from "./format";

const WIDTH = 320;
const CHART_HEIGHT = 64;
const TICK_Y = 68;
const HEIGHT = 74;
const PAD_X = 10;
const PAD_TOP = 10;
const PAD_BOTTOM = 14;
const PLOT_X_START = PAD_X;
const PLOT_X_END = WIDTH - PAD_X;
const PLOT_Y_START = PAD_TOP;
const PLOT_Y_END = CHART_HEIGHT - PAD_BOTTOM;
const BUCKET_HIT_WIDTH = 14;

interface Point {
  x: number;
  y: number;
  bucket: PeriodUsageTrendBucket;
}

export interface TrendBucketReadout {
  value: string;
  meta: string;
  ariaLabel: string;
  empty: boolean;
}

function parseDate(value: string): Date | null {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? null : date;
}

function padTwo(value: number): string {
  return String(value).padStart(2, "0");
}

function formatHourMinute(value: string): string {
  const date = parseDate(value);
  if (!date) return "";
  return `${padTwo(date.getHours())}:${padTwo(date.getMinutes())}`;
}

function formatLocalDate(value: string): string {
  const date = parseDate(value);
  if (!date) return "";
  return `${padTwo(date.getMonth() + 1)}-${padTwo(date.getDate())}`;
}

function formatLocalYearMonth(value: string): string {
  const date = parseDate(value);
  if (!date) return "";
  return `${date.getFullYear()}-${padTwo(date.getMonth() + 1)}`;
}

function formatBucketRange(unit: PeriodUsageTrend["unit"], bucket: PeriodUsageTrendBucket): string {
  if (unit === "day") return formatLocalDate(bucket.started_at);
  if (unit === "month") return formatLocalYearMonth(bucket.started_at);

  const start = formatHourMinute(bucket.started_at);
  const end = formatHourMinute(bucket.ended_at);
  if (!start || !end) return "";
  return `${start}-${end}`;
}

export function formatTrendBucketReadout(unit: PeriodUsageTrend["unit"], bucket: PeriodUsageTrendBucket): TrendBucketReadout {
  const value = `${formatCompactTokens(bucket.total_tokens ?? 0)} token`;
  const meta = formatBucketRange(unit, bucket);
  const empty = (bucket.total_tokens ?? 0) === 0;
  const ariaParts = [bucket.label, meta, value].filter(Boolean);

  return {
    value,
    meta,
    ariaLabel: empty ? `${ariaParts.join(" ")}，无用量` : ariaParts.join(" "),
    empty,
  };
}

function observedBuckets(trend: PeriodUsageTrend): PeriodUsageTrendBucket[] {
  return trend.buckets.filter((bucket) => bucket.total_tokens !== null);
}

function maxTokens(buckets: PeriodUsageTrendBucket[]): number {
  return buckets.reduce((max, bucket) => Math.max(max, bucket.total_tokens ?? 0), 0);
}

function xForIndex(index: number, count: number): number {
  if (count <= 1) return WIDTH / 2;
  return PLOT_X_START + (index / (count - 1)) * (PLOT_X_END - PLOT_X_START);
}

function yForTokens(tokens: number, max: number): number {
  if (max <= 0) return CHART_HEIGHT - PAD_BOTTOM;
  const usableHeight = CHART_HEIGHT - PAD_TOP - PAD_BOTTOM;
  return PAD_TOP + usableHeight - (tokens / max) * usableHeight;
}

function clampY(value: number): number {
  return Math.min(CHART_HEIGHT - PAD_BOTTOM, Math.max(PAD_TOP, value));
}

function pointsFor(trend: PeriodUsageTrend): Point[] {
  const max = maxTokens(observedBuckets(trend));
  return trend.buckets
    .map((bucket, index) => {
      if (bucket.total_tokens === null) return null;
      return {
        x: xForIndex(index, trend.buckets.length),
        y: yForTokens(bucket.total_tokens, max),
        bucket,
      };
    })
    .filter((point): point is Point => point !== null);
}

function bucketHitWidth(points: Point[]): number {
  if (points.length <= 1) return BUCKET_HIT_WIDTH;
  const minSpacing = points.slice(1).reduce((min, point, index) => {
    const previous = points[index];
    return Math.min(min, Math.abs(point.x - previous.x));
  }, Number.POSITIVE_INFINITY);
  return Math.min(BUCKET_HIT_WIDTH, Math.max(8, minSpacing * 0.45));
}

function smoothLinePath(points: Point[]): string {
  if (points.length === 0) return "";
  if (points.length === 1) return `M ${points[0].x} ${points[0].y}`;

  let path = `M ${points[0].x} ${points[0].y}`;
  for (let index = 0; index < points.length - 1; index += 1) {
    const previous = points[Math.max(0, index - 1)];
    const current = points[index];
    const next = points[index + 1];
    const following = points[Math.min(points.length - 1, index + 2)];
    const cp1x = current.x + (next.x - previous.x) / 6;
    const cp1y = clampY(current.y + (next.y - previous.y) / 6);
    const cp2x = next.x - (following.x - current.x) / 6;
    const cp2y = clampY(next.y - (following.y - current.y) / 6);
    path += ` C ${cp1x} ${cp1y}, ${cp2x} ${cp2y}, ${next.x} ${next.y}`;
  }
  return path;
}

function tickLeft(tickKey: string, trend: PeriodUsageTrend): string {
  const index = trend.buckets.findIndex((bucket) => bucket.key === tickKey);
  if (index < 0) return "";
  return xForIndex(index, trend.buckets.length).toString();
}

function tickAnchor(tickIndex: number, tickCount: number): "start" | "middle" | "end" {
  if (tickIndex === 0) return "start";
  if (tickIndex === tickCount - 1) return "end";
  return "middle";
}

export function PeriodTrendChart({ trend }: { trend: PeriodUsageTrend | null | undefined }) {
  const points = trend ? pointsFor(trend) : [];
  const linePath = smoothLinePath(points);
  const endpoint = points[points.length - 1] ?? null;
  const peak = trend ? maxTokens(observedBuckets(trend)) : 0;
  const hasUsage = peak > 0;
  const trendUnit = trend?.unit ?? "hour";
  const [activePoint, setActivePoint] = useState<Point | null>(null);
  const [referenceElement, setReferenceElement] = useState<SVGRectElement | null>(null);
  const activeReadout = activePoint ? formatTrendBucketReadout(trendUnit, activePoint.bucket) : null;
  const bucketHitWidthValue = bucketHitWidth(points);
  const chartDescription = hasUsage
    ? `峰值 ${formatCompactTokens(peak)} token，未来时间桶不计入曲线`
    : "暂无用量，未来时间桶不计入曲线";

  const activatePoint = (point: Point, element: SVGRectElement) => {
    setActivePoint(point);
    setReferenceElement(element);
  };

  const clearActivePoint = () => {
    setActivePoint(null);
    setReferenceElement(null);
  };

  return (
    <section className="profile-panel profile-trend" aria-label="Token 趋势">
      <div className="profile-section-head">
        <span>Token 趋势</span>
        <span>峰值 {formatCompactTokens(peak)} token</span>
      </div>
      <div className="profile-trend__chart">
        <svg className="profile-trend__svg" viewBox={`0 0 ${WIDTH} ${HEIGHT}`} role="img" aria-label="所选周期 token 趋势">
          <desc>{chartDescription}</desc>
          <line className="profile-trend__baseline" x1={PLOT_X_START} y1={CHART_HEIGHT - PAD_BOTTOM} x2={PLOT_X_END} y2={CHART_HEIGHT - PAD_BOTTOM} />
          {linePath ? <path className="profile-trend__line" d={linePath} /> : null}
          {activePoint ? (
            <line
              className="profile-trend__active-line"
              data-active-bucket={activePoint.bucket.key}
              x1={activePoint.x}
              y1={activePoint.y}
              x2={activePoint.x}
              y2={CHART_HEIGHT - PAD_BOTTOM}
            />
          ) : null}
          {points.map((point) => (
            <circle
              key={point.bucket.key}
              className="profile-trend__point"
              data-point-bucket={point.bucket.key}
              cx={point.x}
              cy={point.y}
              r="0"
            />
          ))}
          {endpoint ? (
            <circle
              className="profile-trend__endpoint"
              data-current-bucket={endpoint.bucket.key}
              cx={endpoint.x}
              cy={endpoint.y}
              r="3.8"
            />
          ) : null}
          {activePoint ? (
            <circle
              className="profile-trend__point-active"
              data-active-bucket={activePoint.bucket.key}
              cx={activePoint.x}
              cy={activePoint.y}
              r="5"
            />
          ) : null}
          {points.map((point) => {
            const readout = formatTrendBucketReadout(trendUnit, point.bucket);
            return (
              <rect
                key={`${point.bucket.key}-hit`}
                className="profile-trend__bucket-hit"
                data-bucket-hit={point.bucket.key}
                x={point.x - bucketHitWidthValue / 2}
                y={PLOT_Y_START}
                width={bucketHitWidthValue}
                height={PLOT_Y_END - PLOT_Y_START}
                aria-label={readout.ariaLabel}
                onPointerEnter={(event: PointerEvent<SVGRectElement>) => activatePoint(point, event.currentTarget)}
                onPointerMove={(event: PointerEvent<SVGRectElement>) => activatePoint(point, event.currentTarget)}
                onPointerLeave={clearActivePoint}
              />
            );
          })}
          {trend
            ? trend.x_ticks.map((tick, index) => (
                <text
                  key={tick.bucket_key}
                  className="profile-trend__tick"
                  x={tickLeft(tick.bucket_key, trend)}
                  y={TICK_Y}
                  textAnchor={tickAnchor(index, trend.x_ticks.length)}
                >
                  {tick.label}
                </text>
              ))
            : null}
        </svg>
        {!hasUsage ? <span className="profile-trend__empty">暂无用量</span> : null}
      </div>
      <Popover
        open={Boolean(activeReadout)}
        reference={referenceElement}
        content={
          activeReadout ? (
            <>
              {activeReadout.value}
              {activeReadout.meta ? <span className="tf-popover__meta">{activeReadout.meta}</span> : null}
              {activeReadout.empty ? <span className="tf-popover__meta">无用量</span> : null}
            </>
          ) : null
        }
      />
    </section>
  );
}
