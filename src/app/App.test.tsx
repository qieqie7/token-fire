import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { App } from "./App";

vi.mock("./useProfileSummary", () => ({
  useProfileSummary: () => ({ summary: null, loading: false, error: false }),
}));

vi.mock("./useBuildIdentity", () => ({
  useBuildIdentity: () => ({
    version: "0.1.1",
    git_commit: "7e17eb0abcdef",
    git_commit_short: "7e17eb0",
    build_time: "unix:123",
    dirty: false,
  }),
}));

describe("App", () => {
  it("defaults the profile period to this_month", () => {
    const html = renderToStaticMarkup(<App />);

    expect(html).toMatch(
      /<button\b(?=[^>]*role="tab")(?=[^>]*aria-selected="true")[^>]*>当月<\/button>/,
    );
    expect(html).toContain("v0.1.1 · 7e17eb0");
  });
});
