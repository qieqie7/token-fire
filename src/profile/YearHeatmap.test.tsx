import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { YearHeatmap } from "./YearHeatmap";
import type { ProfileDayBucket } from "./types";

function makeYearDays(
  startDate: string,
  costs: Record<number, number> = {},
  tokens: Record<number, number> = {},
): ProfileDayBucket[] {
  const start = new Date(`${startDate}T00:00:00.000Z`);

  return Array.from({ length: 365 }, (_, index) => {
    const date = new Date(start);
    date.setUTCDate(start.getUTCDate() + index);
    const estimatedCost = costs[index] ?? 0;
    const totalTokens = tokens[index] ?? estimatedCost * 1000;

    return {
      local_date: date.toISOString().slice(0, 10),
      estimated_cost: estimatedCost,
      total_tokens: totalTokens,
      intensity: estimatedCost > 0 ? 4 : 0,
    };
  });
}

describe("YearHeatmap", () => {
  it("renders one daily cell per provided day with low-density header copy", () => {
    const days = makeYearDays("2025-07-06", { 0: 9 }, { 0: 9_400_000 });
    const html = renderToStaticMarkup(
      <YearHeatmap days={days} activeDays={182} estimatedCost={2914} totalTokens={412_000_000} />,
    );

    expect((html.match(/class="profile-heatmap__day"/g) ?? []).length).toBe(365);
    expect(html).toContain("过去 365 天");
    expect(html).toContain("活跃 182 天");
    expect(html).toContain("412.00M token");
    expect(html).toContain("估算 ¥2,914.00");
    expect(html).toContain('data-intensity="4"');
    expect((html.match(/class="profile-heatmap__placeholder"/g) ?? []).length).toBe(6);
  });

  it("uses day-level tooltips instead of weekly ranges", () => {
    const days = makeYearDays("2025-07-06", { 0: 9 }, { 0: 9_400_000 });
    const html = renderToStaticMarkup(
      <YearHeatmap days={days} activeDays={1} estimatedCost={9} totalTokens={9_400_000} />,
    );

    expect(html).toContain("07-06 9.40M token · 估算 ¥9.00");
    expect(html).not.toContain("07-06-07-13");
    expect(html).not.toContain("峰值周");
  });

  it("aligns sparse month labels to real month boundaries", () => {
    const days = makeYearDays("2025-08-15");
    const html = renderToStaticMarkup(
      <YearHeatmap days={days} activeDays={0} estimatedCost={0} totalTokens={0} />,
    );

    expect(html).toContain("9 月");
    expect(html).toContain("12 月");
    expect(html).toContain("3 月");
    expect(html).toContain("6 月");
    expect(html).not.toContain("现在");
  });

  it("keeps the 7 by 53 grid capacity at the Sunday-start maximum", () => {
    const days = makeYearDays("2025-07-06");
    const html = renderToStaticMarkup(
      <YearHeatmap days={days} activeDays={0} estimatedCost={0} totalTokens={0} />,
    );

    expect((html.match(/class="profile-heatmap__day"/g) ?? []).length).toBe(365);
    expect((html.match(/class="profile-heatmap__placeholder"/g) ?? []).length).toBe(6);
  });

  it("keeps billion-scale annual totals readable", () => {
    const days = makeYearDays("2025-07-06");
    const html = renderToStaticMarkup(
      <YearHeatmap days={days} activeDays={1} estimatedCost={90} totalTokens={2_279_000_000} />,
    );

    expect(html).toContain("2.28B token");
  });

  it("renders empty input without crashing", () => {
    const html = renderToStaticMarkup(
      <YearHeatmap days={[]} activeDays={0} estimatedCost={0} totalTokens={0} />,
    );

    expect(html).toContain("过去 365 天");
    expect(html).toContain("活跃 0 天");
    expect(html).toContain("0 token");
    expect((html.match(/class="profile-heatmap__day"/g) ?? []).length).toBe(0);
  });
});
