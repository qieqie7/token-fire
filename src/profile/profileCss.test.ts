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

  it("keeps the fixed menubar profile layout non-scrollable with aligned metric and attribution grids", () => {
    const css = readFileSync(profileCssPath, "utf8");
    const popover = cssBlock(css, ".profile-popover");
    const metricGrid = cssBlock(css, ".profile-metric-grid");
    const attributionGrid = cssBlock(css, ".profile-attribution-grid");
    const profileList = cssBlock(css, ".profile-list");
    const profileSources = cssBlock(css, ".profile-sources");
    const rankedTrack = cssBlock(css, ".profile-ranked-row__track");

    expect(popover).toContain("height: 100vh;");
    expect(popover).toContain("overflow: hidden;");
    expect(metricGrid).toContain("grid-template-columns: 1.3fr 0.9fr;");
    expect(attributionGrid).toContain("grid-template-columns: 0.9fr 1.3fr;");
    expect(profileList).toContain("height: 100%;");
    expect(profileSources).toContain("height: 100%;");
    expect(rankedTrack).toContain("height: 5px;");
    expect(css).not.toContain("height: 198px;");
    expect(css).not.toContain("overflow-y: auto");
  });
});
