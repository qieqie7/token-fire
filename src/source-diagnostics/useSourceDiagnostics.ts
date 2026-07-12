import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect, useMemo, useState } from "react";
import type { SourceDiagnosticsSnapshot } from "./types";

export const USAGE_FACTS_INVALIDATED_EVENT = "usage_facts_invalidated";
export const SOURCE_DIAGNOSTICS_WINDOW_FOCUSED_EVENT = "source_diagnostics_window_focused";

export function loadSourceDiagnostics(): Promise<SourceDiagnosticsSnapshot> {
  return invoke<SourceDiagnosticsSnapshot>("source_diagnostics_snapshot");
}

export async function subscribeSourceDiagnosticsRefresh(refresh: () => void): Promise<() => void> {
  const unlistens: Array<() => void> = [];
  try {
    unlistens.push(await listen(USAGE_FACTS_INVALIDATED_EVENT, refresh));
    unlistens.push(await listen(SOURCE_DIAGNOSTICS_WINDOW_FOCUSED_EVENT, refresh));
    unlistens.push(
      await getCurrentWindow().onFocusChanged(({ payload: focused }) => {
        if (focused) refresh();
      }),
    );
  } catch (error) {
    unlistens.forEach((unlisten) => unlisten());
    throw error;
  }
  return () => {
    unlistens.forEach((unlisten) => unlisten());
  };
}

interface SourceDiagnosticsControllerOptions {
  load: () => Promise<SourceDiagnosticsSnapshot>;
  subscribe: (refresh: () => void) => Promise<() => void>;
  onSnapshot: (snapshot: SourceDiagnosticsSnapshot | null) => void;
  onLoading: (loading: boolean) => void;
  onError: (error: boolean) => void;
  minimumLoadingMs?: number;
}

export function createSourceDiagnosticsController(options: SourceDiagnosticsControllerOptions) {
  let stopped = true;
  let unlisten: (() => void) | undefined;
  let latestRequestId = 0;
  let subscriptionLifecycleId = 0;
  // Single-flight guard：同一时刻只跑一个刷新序列，进行中到来的触发只标记 pending，
  // 由当前序列排空时补跑，把 focus/事件的多次触发合并、避免并行重复读库。
  let inFlight = false;
  let pending = false;
  const minimumLoadingMs = options.minimumLoadingMs ?? 500;

  const wait = (durationMs: number) =>
    new Promise<void>((resolve) => {
      globalThis.setTimeout(resolve, durationMs);
    });

  // 单次读库：不负责 loading 与地板等待，仅做只读快照 + last-write-wins。
  // requestId 现仅用于让 stop()（其自增 latestRequestId）能让进行中的读库结果作废，
  // 并发去重职责已被单飞守卫吸收。
  const runOnce = async () => {
    const requestId = ++latestRequestId;
    try {
      const snapshot = await options.load();
      if (stopped || requestId !== latestRequestId) return;
      options.onSnapshot(snapshot);
      options.onError(false);
    } catch {
      if (stopped || requestId !== latestRequestId) return;
      options.onSnapshot(null);
      options.onError(true);
    }
  };

  const refresh = async () => {
    if (stopped) return;
    if (inFlight) {
      // 已有刷新在跑：合并为一次尾随补跑，避免并行重复读库。
      pending = true;
      return;
    }
    inFlight = true;
    // 地板按整段合流序列计一次，避免补跑把 spinner 拖成 N×minimumLoadingMs。
    const startedAt = Date.now();
    options.onLoading(true);
    try {
      do {
        pending = false;
        await runOnce();
        // 序列看似排空时才补足最小可见时长；若等待期间又来触发，while 会再兜住，不丢刷新。
        if (!pending && !stopped) {
          const remainingLoadingMs = minimumLoadingMs - (Date.now() - startedAt);
          if (remainingLoadingMs > 0) {
            await wait(remainingLoadingMs);
          }
        }
      } while (pending && !stopped);
    } finally {
      inFlight = false;
      if (!stopped) options.onLoading(false);
    }
  };

  return {
    start() {
      stopped = false;
      const lifecycleId = ++subscriptionLifecycleId;
      void options.subscribe(refresh).then(
        (nextUnlisten) => {
          if (stopped || lifecycleId !== subscriptionLifecycleId) {
            nextUnlisten();
            return;
          }
          unlisten = nextUnlisten;
        },
        () => {
          // Manual refresh still works if event subscription is unavailable.
        },
      );
      void refresh();
    },
    stop() {
      stopped = true;
      latestRequestId += 1;
      unlisten?.();
      unlisten = undefined;
    },
    refresh,
  };
}

export function useSourceDiagnostics() {
  const [snapshot, setSnapshot] = useState<SourceDiagnosticsSnapshot | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState(false);
  const controller = useMemo(
    () =>
      createSourceDiagnosticsController({
        load: loadSourceDiagnostics,
        subscribe: subscribeSourceDiagnosticsRefresh,
        onSnapshot: setSnapshot,
        onLoading: setLoading,
        onError: setError,
      }),
    [],
  );

  useEffect(() => {
    controller.start();
    return () => controller.stop();
  }, [controller]);

  return { snapshot, loading, error, refresh: controller.refresh };
}
