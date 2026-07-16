// @ts-ignore - Vitest runs this test in Node and can read CSS from disk.
import { readFileSync } from "node:fs";
// @ts-ignore - Vitest runs this test in Node and can resolve file URLs.
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const profileCssPath = fileURLToPath(new URL("./profile.css", import.meta.url));

describe("profile css", () => {
  it("keeps the daily heatmap low-density inside the menubar popover", () => {
    const css = readFileSync(profileCssPath, "utf8");

    expect(css).toContain(".profile-heatmap__days");
    expect(css).toContain(".profile-heatmap__placeholder");
    expect(css).toContain("grid-template-columns: repeat(53, minmax(0, 1fr));");
    expect(css).toContain("grid-template-rows: repeat(7, minmax(0, 1fr));");
    expect(css).toContain("gap: 2px;");
    expect(css).not.toContain(".profile-heatmap__axis");
    expect(css).not.toContain(".profile-heatmap__weeks::before");
    expect(css).not.toContain(".profile-heatmap__weeks::after");
  });

  function cssBlock(css: string, selector: string): string {
    const start = css.indexOf(`${selector} {`);
    expect(start).toBeGreaterThanOrEqual(0);
    const end = css.indexOf("\n}", start);
    expect(end).toBeGreaterThan(start);
    return css.slice(start, end);
  }

  it("keeps heatmap active readouts minimal", () => {
    const css = readFileSync(profileCssPath, "utf8");
    const activeDay = cssBlock(css, '.profile-heatmap__day[data-active="true"]');

    expect(activeDay).toContain("outline:");
    expect(activeDay).toContain("outline-offset: 1px;");
    expect(css).not.toContain("box-shadow: 0 0 0 8px");
  });

  it("keeps the fixed menubar profile layout non-scrollable with aligned metric and attribution grids", () => {
    const css = readFileSync(profileCssPath, "utf8");
    const popover = cssBlock(css, ".profile-popover");
    const metricGrid = cssBlock(css, ".profile-metric-grid");
    const attributionGrid = cssBlock(css, ".profile-attribution-grid");
    const profileList = cssBlock(css, ".profile-list");
    const profileSources = cssBlock(css, ".profile-sources");
    const rankedTrack = cssBlock(css, ".profile-ranked-row__track");
    const trend = cssBlock(css, ".profile-trend");
    const trendChart = cssBlock(css, ".profile-trend__chart");
    const trendLine = cssBlock(css, ".profile-trend__line");
    const trendEndpoint = cssBlock(css, ".profile-trend__endpoint");

    expect(popover).toContain("height: 100vh;");
    expect(popover).toContain("overflow: hidden;");
    expect(popover).toContain("grid-template-rows: auto auto auto auto auto auto 1fr;");
    expect(metricGrid).toContain("grid-template-columns: 1.3fr 0.9fr;");
    expect(attributionGrid).toContain("grid-template-columns: 0.9fr 1.3fr;");
    expect(profileList).toContain("height: 100%;");
    expect(profileSources).toContain("height: 100%;");
    expect(rankedTrack).toContain("height: 5px;");
    expect(trend).toContain("padding: 9px 10px 8px;");
    expect(trendChart).toContain("aspect-ratio: 320 / 74;");
    expect(trendLine).toContain("stroke: #ff8a34;");
    expect(trendEndpoint).toContain("fill: #ff8a34;");
    expect(css).not.toContain("height: 198px;");
    expect(css).not.toContain("overflow-y: auto");
    expect(css).not.toContain(".profile-trend__ticks");
    expect(css).not.toContain(".profile-trend__ticks span:first-child");
    expect(css).not.toContain(".profile-trend__ticks span:last-child");
  });

  it("keeps the Profile version mark compact in the header", () => {
    const css = readFileSync(profileCssPath, "utf8");
    const header = cssBlock(css, ".profile-header");
    const version = cssBlock(css, ".profile-version");

    expect(header).toContain("min-width: 0;");
    expect(version).toContain("max-width: 190px;");
    expect(version).toContain("text-overflow: ellipsis;");
    expect(version).toContain("white-space: nowrap;");
    expect(version).toContain("font-family: var(--font-family-mono);");
  });

  it("keeps the release update badge compact in the Profile header", () => {
    const css = readFileSync(profileCssPath, "utf8");
    const base = cssBlock(css, ".profile-version");
    const badge = cssBlock(css, ".profile-version--update");

    expect(base).toContain("text-overflow: ellipsis;");
    expect(base).toContain("white-space: nowrap;");
    expect(badge).toContain("border-radius: 999px;");
    expect(badge).toContain("max-width: 190px;");
  });
});
