import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { PeriodTrendChart, formatTrendBucketReadout } from "./PeriodTrendChart";
import type { PeriodUsageTrend } from "./types";

const trend: PeriodUsageTrend = {
  unit: "hour",
  buckets: [
    {
      key: "h00",
      label: "0",
      started_at: "2026-07-04T00:00:00",
      ended_at: "2026-07-04T01:00:00",
      total_tokens: 0,
      is_future: false,
    },
    {
      key: "h01",
      label: "1",
      started_at: "2026-07-04T01:00:00",
      ended_at: "2026-07-04T02:00:00",
      total_tokens: 2_000_000,
      is_future: false,
    },
    {
      key: "h02",
      label: "2",
      started_at: "2026-07-04T02:00:00",
      ended_at: "2026-07-04T03:00:00",
      total_tokens: 3_000_000,
      is_future: false,
    },
    {
      key: "h03",
      label: "3",
      started_at: "2026-07-04T03:00:00",
      ended_at: "2026-07-04T04:00:00",
      total_tokens: null,
      is_future: true,
    },
  ],
  x_ticks: [
    { bucket_key: "h00", label: "0" },
    { bucket_key: "h02", label: "2" },
    { bucket_key: "h03", label: "3" },
  ],
};

describe("PeriodTrendChart", () => {
  it("formats trend readouts without cost", () => {
    expect(formatTrendBucketReadout(trend.unit, trend.buckets[2])).toEqual({
      title: "2",
      value: "3.00M token",
      meta: "02:00-03:00",
      ariaLabel: "2 02:00-03:00 3.00M token",
      empty: false,
    });

    expect(formatTrendBucketReadout(trend.unit, trend.buckets[0])).toEqual({
      title: "0",
      value: "0 token",
      meta: "00:00-01:00",
      ariaLabel: "0 00:00-01:00 0 token，无用量",
      empty: true,
    });
  });

  it("uses calendar labels for day and month buckets instead of midnight hour ranges", () => {
    expect(
      formatTrendBucketReadout("day", {
        key: "d11",
        label: "11",
        started_at: "2026-07-10T16:00:00Z",
        ended_at: "2026-07-11T16:00:00Z",
        total_tokens: 255_460_000,
        is_future: false,
      }),
    ).toEqual({
      title: "11",
      value: "255.46M token",
      meta: "07-11",
      ariaLabel: "11 07-11 255.46M token",
      empty: false,
    });

    expect(
      formatTrendBucketReadout("month", {
        key: "m07",
        label: "7",
        started_at: "2026-06-30T16:00:00Z",
        ended_at: "2026-07-31T16:00:00Z",
        total_tokens: 255_460_000,
        is_future: false,
      }),
    ).toEqual({
      title: "7",
      value: "255.46M token",
      meta: "2026-07",
      ariaLabel: "7 2026-07 255.46M token",
      empty: false,
    });
  });

  it("renders hover hit circles for elapsed buckets only", () => {
    const html = renderToStaticMarkup(<PeriodTrendChart trend={trend} />);

    expect(html).toContain('data-point-hit-bucket="h00"');
    expect(html).toContain('data-point-hit-bucket="h01"');
    expect(html).toContain('data-point-hit-bucket="h02"');
    expect(html).not.toContain('data-point-hit-bucket="h03"');
    expect(html).toContain('r="10"');
    expect(html).toContain('aria-label="2 02:00-03:00 3.00M token"');
  });

  it("keeps existing line, endpoint, and tick behavior", () => {
    const html = renderToStaticMarkup(<PeriodTrendChart trend={trend} />);

    expect(html).toContain("Token 趋势");
    expect(html).toContain('viewBox="0 0 320 74"');
    expect(html).toContain('x1="10"');
    expect(html).toContain('x2="310"');
    expect(html).toContain("峰值 3.00M token");
    expect(html).toContain("未来时间桶不计入曲线");
    expect(html).toContain(">0<");
    expect(html).toContain(">2<");
    expect(html).toContain(">3<");
    expect(html).toContain('data-current-bucket="h02"');
    expect(html).toContain("profile-trend__endpoint");
    expect(html).toContain('class="profile-trend__tick" x="10" y="68" text-anchor="start"');
    expect(html).toContain('class="profile-trend__tick" x="210" y="68" text-anchor="middle"');
    expect(html).toContain('class="profile-trend__tick" x="310" y="68" text-anchor="end"');
    expect(html).not.toContain("profile-trend__future");
    expect(html).not.toContain("profile-trend-future-dots");
    expect(html).not.toContain("profile-trend__area");
    expect(html).not.toContain('data-current-bucket="h03"');
    expect(html).not.toContain('data-point-bucket="h03"');
  });

  it("keeps elapsed zero buckets distinct from future null buckets", () => {
    const html = renderToStaticMarkup(<PeriodTrendChart trend={trend} />);

    expect(html).toContain('data-point-bucket="h00"');
    expect(html).not.toContain('data-future-start="h03"');
  });

  it("renders a quiet empty state when elapsed buckets have no usage", () => {
    const emptyTrend: PeriodUsageTrend = {
      ...trend,
      buckets: trend.buckets.map((bucket) => ({
        ...bucket,
        total_tokens: bucket.is_future ? null : 0,
      })),
    };

    const html = renderToStaticMarkup(<PeriodTrendChart trend={emptyTrend} />);

    expect(html).toContain("暂无用量");
    expect(html).toContain("峰值 0 token");
  });

  it("renders only the endpoint when there is a single elapsed bucket", () => {
    const singlePointTrend: PeriodUsageTrend = {
      ...trend,
      buckets: [
        {
          ...trend.buckets[0],
          total_tokens: 1_000_000,
        },
        {
          ...trend.buckets[1],
          total_tokens: null,
          is_future: true,
        },
      ],
      x_ticks: [
        { bucket_key: "h00", label: "0" },
        { bucket_key: "h01", label: "1" },
      ],
    };

    const html = renderToStaticMarkup(<PeriodTrendChart trend={singlePointTrend} />);

    expect(html).toContain("峰值 1.00M token");
    expect(html).toContain('data-current-bucket="h00"');
    expect(html).toContain("profile-trend__endpoint");
  });

  it("keeps the smoothed line inside the visible token range during steep climbs", () => {
    const html = renderToStaticMarkup(<PeriodTrendChart trend={trend} />);
    const linePath = html.match(/class="profile-trend__line" d="([^"]+)"/)?.[1] ?? "";
    const yValues = linePath
      .match(/(?:^|[ ,])([0-9.]+)(?=[, ]|$)/g)
      ?.map((value) => Number(value.trim().replace(",", "")))
      .filter((_, index) => index % 2 === 1) ?? [];

    expect(yValues.length).toBeGreaterThan(0);
    expect(Math.max(...yValues)).toBeLessThanOrEqual(50);
    expect(Math.min(...yValues)).toBeGreaterThanOrEqual(10);
  });
});
