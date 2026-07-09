import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import type { BuildIdentity } from "../profile/types";

export function loadBuildIdentity(): Promise<BuildIdentity> {
  return invoke<BuildIdentity>("build_identity");
}

export function useBuildIdentity(): BuildIdentity | null {
  const [identity, setIdentity] = useState<BuildIdentity | null>(null);

  useEffect(() => {
    let active = true;

    void loadBuildIdentity().then(
      (nextIdentity) => {
        if (active) setIdentity(nextIdentity);
      },
      () => {
        if (active) setIdentity(null);
      },
    );

    return () => {
      active = false;
    };
  }, []);

  return identity;
}
