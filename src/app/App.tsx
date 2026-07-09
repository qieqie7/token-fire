import { useState } from "react";
import { ProfilePopover } from "../profile/ProfilePopover";
import type { ProfilePeriod } from "../profile/types";
import { useBuildIdentity } from "./useBuildIdentity";
import { useProfileSummary } from "./useProfileSummary";
import { openLatestRelease, useReleaseUpdate } from "./useReleaseUpdate";

export function App() {
  const [period, setPeriod] = useState<ProfilePeriod>("this_month");
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
