import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { describe, expect, it, vi } from "vitest";
import {
  USAGE_FACTS_INVALIDATED_EVENT,
  PROFILE_WINDOW_FOCUSED_EVENT,
  createProfileSummaryController,
  loadProfileSummary,
  subscribeProfileSummaryRefresh,
} from "./useProfileSummary";
import type { ProfileSummary } from "../profile/types";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(),
}));

const summary: ProfileSummary = {
  generated_at: "2026-07-04T12:00:00Z",
  currency: "CNY",
  year_profile: {
    days: Array.from({ length: 365 }, (_, index) => ({
      local_date: `2025-07-${String((index % 28) + 1).padStart(2, "0")}`,
      estimated_cost: 0,
      total_tokens: 0,
      intensity: 0,
    })),
    estimated_cost: 2914,
    total_tokens: 412_000_000,
    active_days: 182,
    average_active_day_cost: 16.01,
    peak_day: {
      local_date: "2026-01-18",
      estimated_cost: 41.2,
      total_tokens: 6_400_000,
    },
  },
  selected_period: {
    period: "this_week",
    started_at: "2026-06-27T12:00:00Z",
    ended_at: "2026-07-04T12:00:00Z",
    estimated_cost: 128.42,
    total_tokens: 19_800_000,
    model_breakdown: [
      { key: "gpt-5.5", label: "GPT-5.5", estimated_cost: 62, total_tokens: 8_000_000, share: 0.48 },
    ],
    source_breakdown: [
      { key: "traex", label: "TraeX", estimated_cost: 82, total_tokens: 12_000_000, share: 0.64 },
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

async function flushProfileRefresh() {
  await Promise.resolve();
  await Promise.resolve();
}

describe("profile summary loader", () => {
  it("invokes profile_summary with the selected calendar period", async () => {
    vi.mocked(invoke).mockResolvedValue(summary);

    await expect(loadProfileSummary("this_month")).resolves.toEqual(summary);

    expect(invoke).toHaveBeenCalledWith("profile_summary", { period: "this_month" });
  });

  it("subscribes to usage fact invalidation and profile open focus events", async () => {
    const handlers = new Map<string, () => void>();
    const unlistenUsage = vi.fn();
    const unlistenFocus = vi.fn();
    vi.mocked(listen).mockImplementation((event, handler) => {
      handlers.set(String(event), handler as () => void);
      return Promise.resolve(event === PROFILE_WINDOW_FOCUSED_EVENT ? unlistenFocus : unlistenUsage);
    });
    const refresh = vi.fn();

    const unlisten = await subscribeProfileSummaryRefresh(refresh);
    handlers.get(PROFILE_WINDOW_FOCUSED_EVENT)?.();
    handlers.get(USAGE_FACTS_INVALIDATED_EVENT)?.();
    unlisten();

    expect(listen).toHaveBeenCalledWith(USAGE_FACTS_INVALIDATED_EVENT, expect.any(Function));
    expect(listen).toHaveBeenCalledWith(PROFILE_WINDOW_FOCUSED_EVENT, expect.any(Function));
    expect(listen).not.toHaveBeenCalledWith("profile_summary_changed", expect.any(Function));
    expect(refresh).toHaveBeenCalledTimes(2);
    expect(unlistenFocus).toHaveBeenCalledTimes(1);
    expect(unlistenUsage).toHaveBeenCalledTimes(1);
  });
});

describe("profile summary controller", () => {
  it("loads immediately and reports loading state changes", async () => {
    const onSummary = vi.fn();
    const onLoading = vi.fn();
    const onError = vi.fn();
    const controller = createProfileSummaryController({
      load: vi.fn().mockResolvedValue(summary),
      onSummary,
      onLoading,
      onError,
      subscribe: vi.fn().mockResolvedValue(() => {}),
      fallbackIntervalMs: 300_000,
      setInterval: () => 0,
      clearInterval: () => {},
    });

    controller.start("this_week");
    await flushProfileRefresh();

    expect(onLoading).toHaveBeenNthCalledWith(1, true);
    expect(onSummary).toHaveBeenCalledWith(summary);
    expect(onError).toHaveBeenCalledWith(false);
    expect(onLoading).toHaveBeenLastCalledWith(false);
  });

  it("keeps the shell renderable when loading fails", async () => {
    const onSummary = vi.fn();
    const onLoading = vi.fn();
    const onError = vi.fn();
    const controller = createProfileSummaryController({
      load: vi.fn().mockRejectedValue(new Error("profile failed")),
      onSummary,
      onLoading,
      onError,
      subscribe: vi.fn().mockResolvedValue(() => {}),
      fallbackIntervalMs: 300_000,
      setInterval: () => 0,
      clearInterval: () => {},
    });

    controller.start("today");
    await flushProfileRefresh();

    expect(onSummary).toHaveBeenCalledWith(null);
    expect(onError).toHaveBeenCalledWith(true);
    expect(onLoading).toHaveBeenLastCalledWith(false);
  });

  it("ignores load resolution after stop", async () => {
    let resolveLoad: ((nextSummary: ProfileSummary) => void) | undefined;
    const onSummary = vi.fn();
    const onLoading = vi.fn();
    const onError = vi.fn();
    const controller = createProfileSummaryController({
      load: vi.fn(
        () =>
          new Promise<ProfileSummary>((resolve) => {
            resolveLoad = resolve;
          }),
      ),
      onSummary,
      onLoading,
      onError,
      subscribe: vi.fn().mockResolvedValue(() => {}),
      fallbackIntervalMs: 300_000,
      setInterval: () => 0,
      clearInterval: () => {},
    });

    controller.start("this_week");
    controller.stop();
    resolveLoad?.(summary);
    await flushProfileRefresh();

    expect(onSummary).not.toHaveBeenCalled();
    expect(onError).not.toHaveBeenCalled();
    expect(onLoading).toHaveBeenCalledTimes(1);
    expect(onLoading).toHaveBeenCalledWith(true);
  });

  it("cleans up late subscriptions without overwriting the active subscription", async () => {
    const unlistenFirst = vi.fn();
    const unlistenSecond = vi.fn();
    let resolveFirst: ((unlisten: () => void) => void) | undefined;
    let resolveSecond: ((unlisten: () => void) => void) | undefined;
    const subscribe = vi
      .fn()
      .mockImplementationOnce(
        () =>
          new Promise<() => void>((resolve) => {
            resolveFirst = resolve;
          }),
      )
      .mockImplementationOnce(
        () =>
          new Promise<() => void>((resolve) => {
            resolveSecond = resolve;
          }),
      );
    const controller = createProfileSummaryController({
      load: vi.fn().mockResolvedValue(summary),
      onSummary: vi.fn(),
      onLoading: vi.fn(),
      onError: vi.fn(),
      subscribe,
      fallbackIntervalMs: 300_000,
      setInterval: () => 0,
      clearInterval: () => {},
    });

    controller.start("this_week");
    controller.stop();
    controller.start("today");
    resolveSecond?.(unlistenSecond);
    await flushProfileRefresh();
    resolveFirst?.(unlistenFirst);
    await flushProfileRefresh();
    controller.stop();

    expect(unlistenFirst).toHaveBeenCalledTimes(1);
    expect(unlistenSecond).toHaveBeenCalledTimes(1);
  });

  it("refreshes on usage-change events", async () => {
    let handler: (() => void) | undefined;
    const load = vi
      .fn()
      .mockResolvedValueOnce(summary)
      .mockResolvedValueOnce({
        ...summary,
        selected_period: {
          ...summary.selected_period,
          estimated_cost: 130,
        },
      });
    const onSummary = vi.fn();
    const controller = createProfileSummaryController({
      load,
      onSummary,
      onLoading: vi.fn(),
      onError: vi.fn(),
      subscribe: vi.fn().mockImplementation((nextHandler: () => void) => {
        handler = nextHandler;
        return Promise.resolve(() => {});
      }),
      fallbackIntervalMs: 300_000,
      setInterval: () => 0,
      clearInterval: () => {},
    });

    controller.start("this_week");
    await flushProfileRefresh();
    handler?.();
    await flushProfileRefresh();

    expect(load).toHaveBeenCalledTimes(2);
    expect(onSummary.mock.calls.at(-1)?.[0].selected_period.estimated_cost).toBe(130);
  });

  it("uses the usage facts invalidated event name", async () => {
    vi.mocked(invoke).mockResolvedValue(summary);
    vi.mocked(listen).mockResolvedValue(() => {});

    expect(USAGE_FACTS_INVALIDATED_EVENT).toBe("usage_facts_invalidated");
    expect(PROFILE_WINDOW_FOCUSED_EVENT).toBe("profile_window_focused");
  });
});
