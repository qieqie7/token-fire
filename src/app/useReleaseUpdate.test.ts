import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { describe, expect, it, vi } from "vitest";
import {
  loadReleaseUpdateStatus,
  openLatestRelease,
  RELEASE_UPDATE_CHANGED_EVENT,
  subscribeReleaseUpdateChanged,
} from "./useReleaseUpdate";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(),
}));

describe("release update hook helpers", () => {
  it("loads release update status from the backend command", async () => {
    const status = {
      state: "update_available",
      checked_at: "2026-07-09T10:00:00Z",
      current_version: "0.1.0",
      current_commit_short: "2b67267",
      latest_version: "0.1.1",
      latest_tag: "v0.1.1",
    };
    vi.mocked(invoke).mockResolvedValue(status);

    await expect(loadReleaseUpdateStatus()).resolves.toEqual(status);

    expect(invoke).toHaveBeenCalledWith("release_update_status");
  });

  it("opens the latest release through the backend command", async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);

    await openLatestRelease();

    expect(invoke).toHaveBeenCalledWith("open_latest_release");
  });

  it("subscribes to release_update_changed and returns the unlisten callback", async () => {
    const unlisten = vi.fn();
    vi.mocked(listen).mockResolvedValue(unlisten);
    const handler = vi.fn();

    await expect(subscribeReleaseUpdateChanged(handler)).resolves.toBe(unlisten);

    expect(listen).toHaveBeenCalledWith(RELEASE_UPDATE_CHANGED_EVENT, handler);
  });
});
