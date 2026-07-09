import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEffect, useMemo, useState } from "react";
import type { ProfilePeriod, ProfileSummary } from "../profile/types";

export const USAGE_FACTS_INVALIDATED_EVENT = "usage_facts_invalidated";
export const PROFILE_WINDOW_FOCUSED_EVENT = "profile_window_focused";

export function loadProfileSummary(period: ProfilePeriod): Promise<ProfileSummary> {
  return invoke<ProfileSummary>("profile_summary", { period });
}

export async function subscribeProfileSummaryRefresh(handler: () => void): Promise<() => void> {
  const unlistens: Array<() => void> = [];
  try {
    unlistens.push(await listen(USAGE_FACTS_INVALIDATED_EVENT, handler));
    unlistens.push(await listen(PROFILE_WINDOW_FOCUSED_EVENT, handler));
  } catch (error) {
    unlistens.forEach((unlisten) => unlisten());
    throw error;
  }
  return () => {
    unlistens.forEach((unlisten) => unlisten());
  };
}

export interface ProfileSummaryControllerOptions {
  load: (period: ProfilePeriod) => Promise<ProfileSummary>;
  onSummary: (summary: ProfileSummary | null) => void;
  onLoading: (loading: boolean) => void;
  onError: (error: boolean) => void;
  subscribe: (handler: () => void) => (() => void) | Promise<() => void>;
  fallbackIntervalMs: number;
  setInterval: (callback: () => void, delay: number) => number;
  clearInterval: (id: number) => void;
}

export function createProfileSummaryController({
  load,
  onSummary,
  onLoading,
  onError,
  subscribe,
  fallbackIntervalMs,
  setInterval,
  clearInterval,
}: ProfileSummaryControllerOptions) {
  let latestRequestId = 0;
  let currentPeriod: ProfilePeriod = "this_month";
  let intervalId: number | undefined;
  let unlisten: (() => void) | undefined;
  let stopped = true;
  let subscriptionLifecycleId = 0;

  const refresh = (period = currentPeriod) => {
    if (stopped) return;
    currentPeriod = period;
    const requestId = ++latestRequestId;
    onLoading(true);
    void load(period).then(
      (summary) => {
        if (stopped || requestId !== latestRequestId) return;
        onSummary(summary);
        onError(false);
        onLoading(false);
      },
      () => {
        if (stopped || requestId !== latestRequestId) return;
        onSummary(null);
        onError(true);
        onLoading(false);
      },
    );
  };

  return {
    refresh,
    start(period: ProfilePeriod) {
      currentPeriod = period;
      stopped = false;
      const lifecycleId = ++subscriptionLifecycleId;
      refresh(period);
      if (intervalId === undefined) {
        intervalId = setInterval(() => refresh(currentPeriod), fallbackIntervalMs);
      }
      void Promise.resolve(subscribe(() => refresh(currentPeriod))).then(
        (nextUnlisten) => {
          if (stopped || lifecycleId !== subscriptionLifecycleId) {
            nextUnlisten();
            return;
          }
          unlisten = nextUnlisten;
        },
        () => {
          // Fallback polling remains active when event subscription is unavailable.
        },
      );
    },
    stop() {
      if (intervalId !== undefined) {
        clearInterval(intervalId);
        intervalId = undefined;
      }
      stopped = true;
      latestRequestId += 1;
      unlisten?.();
      unlisten = undefined;
    },
  };
}

export function useProfileSummary(period: ProfilePeriod) {
  const [summary, setSummary] = useState<ProfileSummary | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState(false);
  const controller = useMemo(
    () =>
      createProfileSummaryController({
        load: loadProfileSummary,
        onSummary: setSummary,
        onLoading: setLoading,
        onError: setError,
        subscribe: subscribeProfileSummaryRefresh,
        fallbackIntervalMs: 300_000,
        setInterval: (callback, delay) => globalThis.setInterval(callback, delay) as unknown as number,
        clearInterval: (id) => globalThis.clearInterval(id),
      }),
    [],
  );

  useEffect(() => {
    controller.start(period);
    return () => controller.stop();
  }, [controller, period]);

  return { summary, loading, error };
}
