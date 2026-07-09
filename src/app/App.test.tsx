import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { ReleaseUpdateStatus } from "../profile/types";
import { App } from "./App";

let releaseUpdateForApp: ReleaseUpdateStatus | null = null;

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

vi.mock("./useReleaseUpdate", () => ({
  useReleaseUpdate: () => releaseUpdateForApp,
  openLatestRelease: vi.fn(),
}));

describe("App", () => {
  it("defaults the profile period to this_month", () => {
    const html = renderToStaticMarkup(<App />);

    expect(html).toMatch(
      /<button\b(?=[^>]*role="tab")(?=[^>]*aria-selected="true")[^>]*>当月<\/button>/,
    );
    expect(html).toContain("v0.1.1 · 7e17eb0");
  });

  it("passes update available state into the Profile header", () => {
    releaseUpdateForApp = {
      state: "update_available",
      checked_at: "2026-07-09T10:00:00Z",
      current_version: "0.1.1",
      current_commit_short: "7e17eb0",
      latest_version: "0.1.2",
      latest_tag: "v0.1.2",
    };

    const html = renderToStaticMarkup(<App />);

    expect(html).toContain("v0.1.1 · 7e17eb0 可更新");
    releaseUpdateForApp = null;
  });
});
