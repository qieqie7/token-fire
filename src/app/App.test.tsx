import { renderToStaticMarkup } from "react-dom/server";
import { invoke } from "@tauri-apps/api/core";
import { describe, expect, it, vi } from "vitest";
import type { ReleaseUpdateStatus } from "../profile/types";
import type { DiagnosticAction } from "../source-diagnostics/types";
import { App, handleSourceDiagnosticsAction } from "./App";

let releaseUpdateForApp: ReleaseUpdateStatus | null = null;
let currentWindowLabel = "main";

const useProfileSummaryMock = vi.hoisted(() =>
  vi.fn(() => ({ summary: null, loading: false, error: false })),
);

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ label: currentWindowLabel }),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

vi.mock("./useProfileSummary", () => ({
  useProfileSummary: useProfileSummaryMock,
}));

vi.mock("../source-diagnostics/useSourceDiagnostics", () => ({
  useSourceDiagnostics: () => ({
    snapshot: { generatedAt: "2026-07-10T10:00:00Z", summary: { connected: 0, attention: 0, disabled: 0 }, sources: [] },
    loading: false,
    error: false,
    refresh: vi.fn(),
  }),
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
  it("defaults the profile period to today", () => {
    useProfileSummaryMock.mockClear();
    const html = renderToStaticMarkup(<App />);

    expect(html).toMatch(
      /<button\b(?=[^>]*role="tab")(?=[^>]*aria-selected="true")[^>]*>当日<\/button>/,
    );
    expect(useProfileSummaryMock).toHaveBeenCalledWith("today");
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

  it("renders source diagnostics in the source-diagnostics window", () => {
    currentWindowLabel = "source-diagnostics";

    const html = renderToStaticMarkup(<App />);

    expect(html).toContain("接入诊断");
    expect(html).not.toContain("过去 365 天");
    currentWindowLabel = "main";
  });

  it("routes source diagnostics actions through refresh or native command", async () => {
    const refresh = vi.fn();
    vi.mocked(invoke).mockResolvedValue(undefined);

    handleSourceDiagnosticsAction({ id: "refresh", label: "刷新", enabled: true }, refresh);
    handleSourceDiagnosticsAction({ id: "open_logs", label: "打开日志", enabled: true }, refresh);
    await Promise.resolve();

    expect(refresh).toHaveBeenCalledTimes(2);
    expect(invoke).toHaveBeenCalledWith("source_diagnostics_action", { actionId: "open_logs" });
  });
});
