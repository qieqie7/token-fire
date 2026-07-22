import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { ProfilePopover } from "./ProfilePopover";
import type { BuildIdentity, ProfileSummary } from "./types";

const days = Array.from({ length: 365 }, (_, index) => ({
  local_date: `2026-01-${String((index % 28) + 1).padStart(2, "0")}`,
  estimated_cost: index === 10 ? 41.2 : 0,
  total_tokens: index === 10 ? 6_400_000 : 0,
  intensity: index === 10 ? 4 : 0,
})) as ProfileSummary["year_profile"]["days"];

const trend: ProfileSummary["selected_period"]["trend"] = {
  unit: "day",
  buckets: [
    {
      key: "d01",
      label: "1",
      started_at: "2026-07-01T00:00:00Z",
      ended_at: "2026-07-02T00:00:00Z",
      total_tokens: 4_000_000,
      is_future: false,
    },
    {
      key: "d02",
      label: "2",
      started_at: "2026-07-02T00:00:00Z",
      ended_at: "2026-07-03T00:00:00Z",
      total_tokens: 0,
      is_future: false,
    },
    {
      key: "d03",
      label: "3",
      started_at: "2026-07-03T00:00:00Z",
      ended_at: "2026-07-04T00:00:00Z",
      total_tokens: 15_800_000,
      is_future: false,
    },
    {
      key: "d04",
      label: "4",
      started_at: "2026-07-04T00:00:00Z",
      ended_at: "2026-07-05T00:00:00Z",
      total_tokens: null,
      is_future: true,
    },
  ],
  x_ticks: [
    { bucket_key: "d01", label: "1" },
    { bucket_key: "d03", label: "3" },
    { bucket_key: "d04", label: "月末" },
  ],
};

const summary: ProfileSummary = {
  generated_at: "2026-07-04T12:00:00Z",
  currency: "CNY",
  year_profile: {
    days,
    estimated_cost: 2914,
    total_tokens: 412_000_000,
    active_days: 182,
    average_active_day_cost: 16.01,
    peak_day: { local_date: "2026-01-18", estimated_cost: 41.2, total_tokens: 6_400_000 },
  },
  selected_period: {
    period: "this_month",
    started_at: "2026-06-27T12:00:00Z",
    ended_at: "2026-07-04T12:00:00Z",
    estimated_cost: 128.42,
    total_tokens: 19_800_000,
    trend,
    model_breakdown: [
      { key: "gpt-5.5", label: "GPT-5.5", estimated_cost: 62, total_tokens: 8_000_000, share: 0.4 },
      { key: "gpt-5.4", label: "GPT-5.4", estimated_cost: 39, total_tokens: 6_000_000, share: 0.3 },
      { key: "claude-opus-4", label: "Claude Opus 4", estimated_cost: 21, total_tokens: 4_000_000, share: 0.16 },
      { key: "gemini-flash", label: "Gemini Flash", estimated_cost: 6, total_tokens: 1_000_000, share: 0.05 },
      { key: "gpt-5-mini", label: "GPT-5 Mini", estimated_cost: 4, total_tokens: 500_000, share: 0.03 },
      { key: "other", label: "Other", estimated_cost: 2, total_tokens: 300_000, share: 0.02 },
    ],
    source_breakdown: [
      { key: "traex", label: "TraeX", estimated_cost: 82, total_tokens: 12_000_000, share: 0.64 },
      { key: "codex", label: "Codex", estimated_cost: 31, total_tokens: 5_000_000, share: 0.24 },
      { key: "claude", label: "Claude", estimated_cost: 10, total_tokens: 2_000_000, share: 0.08 },
      { key: "gpt", label: "GPT", estimated_cost: 5, total_tokens: 800_000, share: 0.04 },
    ],
    cost_drivers: {
      input_cost: 20,
      output_cost: 70,
      reasoning_output_cost: 28,
      cache_creation_input_cost: 10,
      cached_input_cost: 0.5,
      unattributed_cost: 0,
      cached_input_tokens: 1_000_000,
      cache_read_ratio: 0.2,
    },
  },
};

const buildIdentity: BuildIdentity = {
  version: "0.1.1",
  git_commit: "7e17eb0abcdef",
  git_commit_short: "7e17eb0",
  build_time: "unix:123",
  dirty: false,
};

describe("ProfilePopover", () => {
  it("renders fixed yearly profile above selected-period analytics", () => {
    const html = renderToStaticMarkup(
      <ProfilePopover
        period="this_month"
        summary={summary}
        loading={false}
        error={false}
        buildIdentity={buildIdentity}
        onPeriodChange={() => {}}
      />,
    );

    expect(html.indexOf("过去 365 天")).toBeLessThan(html.indexOf("所选周期"));
    expect(html).toContain("活跃 182 天");
    expect(html).toContain("412.00M token");
    expect(html).toContain("估算 ¥2,914.00");
    expect(html).toContain("¥128.42");
    expect(html).toContain("19.80M");
    expect(html.indexOf("估算成本")).toBeLessThan(html.indexOf("Token 数"));
    expect(html).toContain("GPT-5.5");
    expect(html).toContain("TraeX");
    expect(html).toContain("应用来源");
    expect(html).toContain("按 token 前 5");
    expect(html).toContain("当日");
    expect(html).toContain("当周");
    expect(html).toContain("当月");
    expect(html).toContain("当年");
    const selectedPeriodHtml = html.slice(html.indexOf("当日"));
    expect(selectedPeriodHtml.indexOf("当年")).toBeLessThan(selectedPeriodHtml.indexOf("Token 趋势"));
    expect(selectedPeriodHtml.indexOf("Token 趋势")).toBeLessThan(selectedPeriodHtml.indexOf("估算成本"));
    expect(selectedPeriodHtml).toContain("峰值 15.80M token");
    expect(selectedPeriodHtml).toContain('data-current-bucket="d03"');
    expect(html).not.toContain(">1D<");
    expect(html).not.toContain(">1W<");
    expect(html).not.toContain(">1M<");
    expect(html).not.toContain(">1Y<");
    expect(html).not.toContain("App sources");
    expect(html).not.toContain("Selected period");
    expect(html).not.toContain("Estimated cost");
    expect(html).not.toContain("年度估算");
    expect(html).not.toContain("日均活跃");
    expect(html).not.toContain("账单");
    expect(html).not.toContain("已扣费");
    expect(html).not.toContain("真实支出");
    expect(html).not.toContain("年度 Token");
    expect(html).not.toContain("日均 Token");
    expect(html).not.toContain("最近一年");
    expect(html).not.toContain("峰值周");
  });

  it("keeps chart readout infrastructure scoped to chart elements", () => {
    const html = renderToStaticMarkup(
      <ProfilePopover
        period="this_month"
        summary={summary}
        loading={false}
        error={false}
        buildIdentity={buildIdentity}
        onPeriodChange={() => {}}
      />,
    );

    const heatmapStart = html.indexOf("profile-heatmap__day");
    const heatmapEnd = html.indexOf("profile-heatmap__months");
    const heatmapHtml = html.slice(heatmapStart, heatmapEnd);

    expect(heatmapHtml).not.toContain("title=");
    expect(html).toContain("profile-trend__bucket-hit");
    expect(html).not.toContain("profile-trend__point-hit");
    expect(html).not.toContain("profile-trend__future");
    expect(html).not.toContain('data-point-hit-bucket="d04"');
    expect(html).toContain("TokenFire v0.1.1");
  });

  it("keeps the TokenFire identity visible during errors without maintenance controls", () => {
    const html = renderToStaticMarkup(
      <ProfilePopover
        period="today"
        summary={null}
        loading={false}
        error={true}
        buildIdentity={null}
        onPeriodChange={() => {}}
      />,
    );

    expect(html).toContain("TokenFire");
    expect(html).toContain("估算不可用");
    expect(html).not.toContain("Profile menu");
    expect(html).not.toContain("安装 TraeX Hook");
    expect(html).not.toContain("卸载 Codex Hook");
    expect(html).not.toContain("开启调试日志");
  });

  it("does not render peak-week copy in the empty/error heatmap fallback", () => {
    const html = renderToStaticMarkup(
      <ProfilePopover
        period="today"
        summary={null}
        loading={false}
        error={true}
        buildIdentity={null}
        onPeriodChange={() => {}}
      />,
    );

    expect(html).toContain("过去 365 天");
    expect(html).toContain("活跃 0 天");
    expect(html).not.toContain("峰值周");
  });

  it("renders compact build identity in the header", () => {
    const html = renderToStaticMarkup(
      <ProfilePopover
        period="this_month"
        summary={summary}
        loading={false}
        error={false}
        buildIdentity={buildIdentity}
        onPeriodChange={() => {}}
      />,
    );

    expect(html).toContain("v0.1.1 · 7e17eb0");
    expect(html.indexOf("TokenFire")).toBeLessThan(html.indexOf("v0.1.1"));
    expect(html).not.toContain("unix:123");
    expect(html).not.toContain("7e17eb0abcdef");
  });

  it("marks dirty builds without showing build time", () => {
    const html = renderToStaticMarkup(
      <ProfilePopover
        period="this_month"
        summary={summary}
        loading={false}
        error={false}
        buildIdentity={{ ...buildIdentity, dirty: true }}
        onPeriodChange={() => {}}
      />,
    );

    expect(html).toContain("v0.1.1 · 7e17eb0 · dirty");
    expect(html).not.toContain("unix:123");
  });

  it("shows current build identity with update marker when a newer release is available", () => {
    const html = renderToStaticMarkup(
      <ProfilePopover
        period="this_month"
        summary={summary}
        loading={false}
        error={false}
        buildIdentity={{ ...buildIdentity, version: "0.1.0", git_commit_short: "2b67267" }}
        releaseUpdate={{
          state: "update_available",
          checked_at: "2026-07-09T10:00:00Z",
          current_version: "0.1.0",
          current_commit_short: "2b67267",
          latest_version: "0.1.1",
          latest_tag: "v0.1.1",
        }}
        onOpenRelease={() => {}}
        onPeriodChange={() => {}}
      />,
    );

    expect(html).toContain("v0.1.0 · 2b67267 可更新");
    expect(html).toContain("TokenFire v0.1.1 可用，当前 v0.1.0 · 2b67267，点击打开 GitHub Release");
    expect(html).not.toContain(">v0.1.1 可用<");
  });

  it("keeps the normal build identity label when release status is not update_available", () => {
    const html = renderToStaticMarkup(
      <ProfilePopover
        period="this_month"
        summary={summary}
        loading={false}
        error={false}
        buildIdentity={buildIdentity}
        releaseUpdate={{
          state: "up_to_date",
          checked_at: "2026-07-09T10:00:00Z",
          current_version: "0.1.1",
          latest_version: "0.1.1",
        }}
        onOpenRelease={() => {}}
        onPeriodChange={() => {}}
      />,
    );

    expect(html).toContain("v0.1.1 · 7e17eb0");
    expect(html).not.toContain("可更新");
  });
});
