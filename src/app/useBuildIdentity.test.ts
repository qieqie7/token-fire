import { invoke } from "@tauri-apps/api/core";
import { describe, expect, it, vi } from "vitest";
import { loadBuildIdentity } from "./useBuildIdentity";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

describe("build identity loader", () => {
  it("invokes the build_identity command", async () => {
    const identity = {
      version: "0.1.1",
      git_commit: "7e17eb0abcdef",
      git_commit_short: "7e17eb0",
      build_time: "unix:123",
      dirty: false,
    };
    vi.mocked(invoke).mockResolvedValue(identity);

    await expect(loadBuildIdentity()).resolves.toEqual(identity);

    expect(invoke).toHaveBeenCalledWith("build_identity");
  });
});
