import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  SOURCE_DIAGNOSTICS_WINDOW_FOCUSED_EVENT,
  USAGE_FACTS_INVALIDATED_EVENT,
  createSourceDiagnosticsController,
  loadSourceDiagnostics,
  subscribeSourceDiagnosticsRefresh,
} from "./useSourceDiagnostics";
import type { SourceDiagnosticsSnapshot } from "./types";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: vi.fn(),
}));

const snapshot: SourceDiagnosticsSnapshot = {
  generatedAt: "2026-07-10T10:00:00Z",
  summary: { connected: 2, attention: 1, disabled: 1 },
  sources: [],
};

async function flush() {
  await Promise.resolve();
  await Promise.resolve();
}

describe("source diagnostics loader", () => {
  beforeEach(() => {
    vi.mocked(getCurrentWindow).mockReturnValue({
      onFocusChanged: vi.fn().mockResolvedValue(() => {}),
    } as unknown as ReturnType<typeof getCurrentWindow>);
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("invokes source_diagnostics_snapshot", async () => {
    vi.mocked(invoke).mockResolvedValue(snapshot);

    await expect(loadSourceDiagnostics()).resolves.toEqual(snapshot);

    expect(invoke).toHaveBeenCalledWith("source_diagnostics_snapshot");
  });

  it("refreshes on usage invalidation and diagnostics window focus", async () => {
    const handlers = new Map<string, () => void>();
    const unlistenUsage = vi.fn();
    const unlistenFocus = vi.fn();
    vi.mocked(listen).mockImplementation((event, handler) => {
      handlers.set(String(event), handler as () => void);
      return Promise.resolve(String(event) === "source_diagnostics_window_focused" ? unlistenFocus : unlistenUsage);
    });
    const refresh = vi.fn();

    const unlisten = await subscribeSourceDiagnosticsRefresh(refresh);
    handlers.get("usage_facts_invalidated")?.();
    handlers.get("source_diagnostics_window_focused")?.();
    unlisten();

    expect(refresh).toHaveBeenCalledTimes(2);
    expect(unlistenUsage).toHaveBeenCalledTimes(1);
    expect(unlistenFocus).toHaveBeenCalledTimes(1);
  });

  it("refreshes when the diagnostics window enters focus", async () => {
    const handlers = new Map<string, () => void>();
    vi.mocked(listen).mockImplementation((event, handler) => {
      handlers.set(String(event), handler as () => void);
      return Promise.resolve(() => {});
    });
    const unlistenWindowFocus = vi.fn();
    let focusHandler: ((event: { payload: boolean }) => void) | undefined;
    vi.mocked(getCurrentWindow).mockReturnValue({
      onFocusChanged: vi.fn((handler) => {
        focusHandler = handler as (event: { payload: boolean }) => void;
        return Promise.resolve(unlistenWindowFocus);
      }),
    } as unknown as ReturnType<typeof getCurrentWindow>);
    const refresh = vi.fn();

    const unlisten = await subscribeSourceDiagnosticsRefresh(refresh);
    focusHandler?.({ payload: true });
    focusHandler?.({ payload: false });
    handlers.get(SOURCE_DIAGNOSTICS_WINDOW_FOCUSED_EVENT)?.();
    unlisten();

    expect(refresh).toHaveBeenCalledTimes(2);
    expect(unlistenWindowFocus).toHaveBeenCalledTimes(1);
  });

  it("cleans up the usage listener when diagnostics focus subscription fails", async () => {
    vi.mocked(listen).mockReset();
    const unlistenUsage = vi.fn();
    const refresh = vi.fn();
    vi.mocked(listen)
      .mockResolvedValueOnce(unlistenUsage)
      .mockRejectedValueOnce(new Error("focus listen failed"));

    await expect(subscribeSourceDiagnosticsRefresh(refresh)).rejects.toThrow("focus listen failed");

    expect(listen).toHaveBeenCalledWith(USAGE_FACTS_INVALIDATED_EVENT, refresh);
    expect(listen).toHaveBeenCalledWith(SOURCE_DIAGNOSTICS_WINDOW_FOCUSED_EVENT, refresh);
    expect(unlistenUsage).toHaveBeenCalledTimes(1);
  });

  it("loads immediately and reports errors", async () => {
    const onSnapshot = vi.fn();
    const onLoading = vi.fn();
    const onError = vi.fn();
    const controller = createSourceDiagnosticsController({
      load: vi.fn().mockRejectedValue(new Error("diagnostics failed")),
      subscribe: vi.fn().mockResolvedValue(() => {}),
      onSnapshot,
      onLoading,
      onError,
      minimumLoadingMs: 0,
    });

    controller.start();
    await flush();

    expect(onSnapshot).toHaveBeenCalledWith(null);
    expect(onError).toHaveBeenCalledWith(true);
    expect(onLoading).toHaveBeenLastCalledWith(false);
  });

  it("keeps loading visible for at least 500ms when refresh resolves quickly", async () => {
    vi.useFakeTimers();
    const onSnapshot = vi.fn();
    const onLoading = vi.fn();
    const controller = createSourceDiagnosticsController({
      load: vi.fn().mockResolvedValue(snapshot),
      subscribe: vi.fn().mockResolvedValue(() => {}),
      onSnapshot,
      onLoading,
      onError: vi.fn(),
    });

    controller.start();
    await flush();

    expect(onSnapshot).toHaveBeenCalledWith(snapshot);
    expect(onLoading).toHaveBeenLastCalledWith(true);

    await vi.advanceTimersByTimeAsync(499);
    expect(onLoading).toHaveBeenLastCalledWith(true);

    await vi.advanceTimersByTimeAsync(1);
    expect(onLoading).toHaveBeenLastCalledWith(false);
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
    const controller = createSourceDiagnosticsController({
      load: vi.fn().mockResolvedValue(snapshot),
      subscribe,
      onSnapshot: vi.fn(),
      onLoading: vi.fn(),
      onError: vi.fn(),
      minimumLoadingMs: 0,
    });

    controller.start();
    controller.stop();
    controller.start();
    resolveSecond?.(unlistenSecond);
    await flush();
    resolveFirst?.(unlistenFirst);
    await flush();
    controller.stop();

    expect(unlistenFirst).toHaveBeenCalledTimes(1);
    expect(unlistenSecond).toHaveBeenCalledTimes(1);
  });

  it("does not refresh after stop", async () => {
    const load = vi.fn().mockResolvedValue(snapshot);
    const onLoading = vi.fn();
    const controller = createSourceDiagnosticsController({
      load,
      subscribe: vi.fn().mockResolvedValue(() => {}),
      onSnapshot: vi.fn(),
      onLoading,
      onError: vi.fn(),
      minimumLoadingMs: 0,
    });

    controller.start();
    controller.stop();
    onLoading.mockClear();
    controller.refresh();
    await flush();

    expect(load).toHaveBeenCalledTimes(1);
    expect(onLoading).not.toHaveBeenCalled();
  });

  it("collapses triggers that arrive during an in-flight refresh into one trailing load", async () => {
    const resolvers: Array<() => void> = [];
    const load = vi.fn().mockImplementation(
      () =>
        new Promise<SourceDiagnosticsSnapshot>((resolve) => {
          resolvers.push(() => resolve(snapshot));
        }),
    );
    const onLoading = vi.fn();
    const controller = createSourceDiagnosticsController({
      load,
      subscribe: vi.fn().mockResolvedValue(() => {}),
      onSnapshot: vi.fn(),
      onLoading,
      onError: vi.fn(),
      minimumLoadingMs: 0,
    });

    controller.start();
    await flush();
    // start() 触发首刷，此时 in-flight；再叠三次触发（模拟 focus 事件 + onFocusChanged 等）。
    expect(load).toHaveBeenCalledTimes(1);
    controller.refresh();
    controller.refresh();
    controller.refresh();
    await flush();
    expect(load).toHaveBeenCalledTimes(1);

    // 首刷结束后只补跑一次，三次触发被合并。
    resolvers.shift()?.();
    await flush();
    expect(load).toHaveBeenCalledTimes(2);
    resolvers.shift()?.();
    await flush();
    expect(load).toHaveBeenCalledTimes(2);

    // loading 全程只开一次、合并序列结束后才关一次，中间不闪。
    expect(onLoading.mock.calls).toEqual([[true], [false]]);
  });

  it("still runs the trailing load after the in-flight refresh errors", async () => {
    const resolvers: Array<(value: SourceDiagnosticsSnapshot | Error) => void> = [];
    const load = vi.fn().mockImplementation(
      () =>
        new Promise<SourceDiagnosticsSnapshot>((resolve, reject) => {
          resolvers.push((value) => (value instanceof Error ? reject(value) : resolve(value)));
        }),
    );
    const onSnapshot = vi.fn();
    const onError = vi.fn();
    const controller = createSourceDiagnosticsController({
      load,
      subscribe: vi.fn().mockResolvedValue(() => {}),
      onSnapshot,
      onLoading: vi.fn(),
      onError,
      minimumLoadingMs: 0,
    });

    controller.start();
    await flush();
    // 首刷 in-flight 期间到来一次触发，随后首刷失败。
    controller.refresh();
    resolvers.shift()?.(new Error("diagnostics failed"));
    await flush();

    // 尽管首刷报错，尾随补跑仍执行，并能恢复成功状态。
    expect(load).toHaveBeenCalledTimes(2);
    expect(onError).toHaveBeenCalledWith(true);
    resolvers.shift()?.(snapshot);
    await flush();
    expect(onSnapshot).toHaveBeenLastCalledWith(snapshot);
    expect(onError).toHaveBeenLastCalledWith(false);
  });

  it("cancels the pending trailing run when stopped mid-flight", async () => {
    const resolvers: Array<() => void> = [];
    const load = vi.fn().mockImplementation(
      () =>
        new Promise<SourceDiagnosticsSnapshot>((resolve) => {
          resolvers.push(() => resolve(snapshot));
        }),
    );
    const onLoading = vi.fn();
    const controller = createSourceDiagnosticsController({
      load,
      subscribe: vi.fn().mockResolvedValue(() => {}),
      onSnapshot: vi.fn(),
      onLoading,
      onError: vi.fn(),
      minimumLoadingMs: 0,
    });

    controller.start();
    await flush();
    controller.refresh(); // pending = true
    controller.stop(); // 进行中 + pending 时停止
    resolvers.shift()?.(); // 首刷 resolve
    await flush();

    // stop() 后不得补跑，且停止后不再切 loading。
    expect(load).toHaveBeenCalledTimes(1);
    expect(onLoading).not.toHaveBeenCalledWith(false);
  });

  it("holds the loading floor once for the whole merged sequence", async () => {
    vi.useFakeTimers();
    const resolvers: Array<() => void> = [];
    const load = vi.fn().mockImplementation(
      () =>
        new Promise<SourceDiagnosticsSnapshot>((resolve) => {
          resolvers.push(() => resolve(snapshot));
        }),
    );
    const onLoading = vi.fn();
    const controller = createSourceDiagnosticsController({
      load,
      subscribe: vi.fn().mockResolvedValue(() => {}),
      onSnapshot: vi.fn(),
      onLoading,
      onError: vi.fn(),
    });

    controller.start();
    await flush();
    controller.refresh(); // in-flight 期间叠一次触发 → 合并补跑
    await flush();

    // 首刷立即 resolve，补跑也立即 resolve；地板从序列起点只计一次 500ms。
    resolvers.shift()?.();
    await flush();
    resolvers.shift()?.();
    await flush();

    // 合并了 2 次 runOnce，但整段仅需一次 500ms 地板，而非 1000ms。
    await vi.advanceTimersByTimeAsync(499);
    expect(onLoading).not.toHaveBeenCalledWith(false);
    await vi.advanceTimersByTimeAsync(1);
    expect(onLoading).toHaveBeenLastCalledWith(false);
    expect(load).toHaveBeenCalledTimes(2);
  });
});
