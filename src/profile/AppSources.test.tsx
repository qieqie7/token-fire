import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { AppSources } from "./AppSources";
import type { RankedProfileBreakdown } from "./types";

const rows: RankedProfileBreakdown[] = [
  { key: "traex", label: "TraeX", estimated_cost: 48, total_tokens: 4800, share: 0.48 },
  { key: "claude", label: "Claude", estimated_cost: 24, total_tokens: 2400, share: 0.24 },
  { key: "codex", label: "Codex", estimated_cost: 16, total_tokens: 1600, share: 0.16 },
  { key: "gpt", label: "GPT", estimated_cost: 8, total_tokens: 800, share: 0.08 },
  { key: "other-app", label: "Other App", estimated_cost: 4, total_tokens: 400, share: 0.04 },
];

describe("AppSources", () => {
  it("renders a compact pie chart with top three sources and one aggregate other bucket", () => {
    const html = renderToStaticMarkup(<AppSources rows={rows} />);

    expect(html).toContain("应用来源");
    expect(html).toContain("按 token 前 3");
    expect(html).toContain("profile-source-pie");
    expect(html).toContain("conic-gradient");
    expect(html).toContain("TraeX");
    expect(html).toContain("Claude");
    expect(html).toContain("Codex");
    expect(html).toContain("其他");
    expect(html).not.toContain("GPT");
    expect(html).not.toContain("Other App");
    expect(html).not.toContain("48%");
    expect(html).not.toContain("24%");
  });

  it("does not render static supported-source hints when there is no source data", () => {
    const html = renderToStaticMarkup(<AppSources rows={[]} />);

    expect(html).toContain("应用来源");
    expect(html).toContain("未知");
    expect(html).not.toContain("TraeX");
    expect(html).not.toContain("Codex");
    expect(html).not.toContain("Claude");
    expect(html).not.toContain("Cursor");
    expect(html).not.toContain("安装 Claude Hook");
    expect(html).not.toContain("安装 Cursor Hook");
  });
});
