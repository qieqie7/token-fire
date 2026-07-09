import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";
import type { ReleaseUpdateStatus } from "../profile/types";

export const RELEASE_UPDATE_CHANGED_EVENT = "release_update_changed";

export function loadReleaseUpdateStatus(): Promise<ReleaseUpdateStatus> {
  return invoke<ReleaseUpdateStatus>("release_update_status");
}

export function openLatestRelease(): Promise<void> {
  return invoke<void>("open_latest_release");
}

export async function subscribeReleaseUpdateChanged(handler: () => void): Promise<() => void> {
  return listen(RELEASE_UPDATE_CHANGED_EVENT, handler);
}

export function useReleaseUpdate(): ReleaseUpdateStatus | null {
  const [status, setStatus] = useState<ReleaseUpdateStatus | null>(null);

  useEffect(() => {
    let active = true;

    const refresh = () => {
      void loadReleaseUpdateStatus().then(
        (nextStatus) => {
          if (active) setStatus(nextStatus);
        },
        () => {
          if (active) setStatus(null);
        },
      );
    };

    refresh();
    let unlisten: (() => void) | undefined;
    void subscribeReleaseUpdateChanged(refresh).then(
      (nextUnlisten) => {
        if (!active) {
          nextUnlisten();
          return;
        }
        unlisten = nextUnlisten;
      },
      () => {},
    );

    return () => {
      active = false;
      unlisten?.();
    };
  }, []);

  return status;
}
