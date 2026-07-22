import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { ProfilePopover } from "../profile/ProfilePopover";
import { DEFAULT_PROFILE_PERIOD } from "../profile/defaultPeriod";
import type { ProfilePeriod } from "../profile/types";
import { SourceDiagnostics } from "../source-diagnostics/SourceDiagnostics";
import type { DiagnosticAction } from "../source-diagnostics/types";
import { useSourceDiagnostics } from "../source-diagnostics/useSourceDiagnostics";
import { useBuildIdentity } from "./useBuildIdentity";
import { useProfileSummary } from "./useProfileSummary";
import { openLatestRelease, useReleaseUpdate } from "./useReleaseUpdate";

export function App() {
  const windowLabel = currentWindowLabel();
  if (windowLabel === "source-diagnostics") {
    return <SourceDiagnosticsApp />;
  }
  return <ProfileApp />;
}

function currentWindowLabel(): string {
  try {
    return getCurrentWindow().label;
  } catch {
    return "main";
  }
}

function ProfileApp() {
  const [period, setPeriod] = useState<ProfilePeriod>(DEFAULT_PROFILE_PERIOD);
  const { summary, loading, error } = useProfileSummary(period);
  const buildIdentity = useBuildIdentity();
  const releaseUpdate = useReleaseUpdate();

  return (
    <ProfilePopover
      period={period}
      summary={summary}
      loading={loading}
      error={error}
      buildIdentity={buildIdentity}
      releaseUpdate={releaseUpdate}
      onOpenRelease={() => {
        void openLatestRelease();
      }}
      onPeriodChange={setPeriod}
    />
  );
}

function SourceDiagnosticsApp() {
  const { snapshot, loading, error, refresh } = useSourceDiagnostics();

  return (
    <SourceDiagnostics
      snapshot={snapshot}
      loading={loading}
      error={error}
      onRefresh={refresh}
      onAction={(action) => handleSourceDiagnosticsAction(action, refresh)}
    />
  );
}

export function handleSourceDiagnosticsAction(action: DiagnosticAction, refresh: () => void) {
  if (action.id === "refresh") {
    refresh();
    return;
  }
  void invoke("source_diagnostics_action", { actionId: action.id }).then(refresh);
}
