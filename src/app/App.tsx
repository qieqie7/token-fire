import { useState } from "react";
import { ProfilePopover } from "../profile/ProfilePopover";
import type { ProfilePeriod } from "../profile/types";
import { useProfileSummary } from "./useProfileSummary";

export function App() {
  const [period, setPeriod] = useState<ProfilePeriod>("this_month");
  const { summary, loading, error } = useProfileSummary(period);

  return (
    <ProfilePopover
      period={period}
      summary={summary}
      loading={loading}
      error={error}
      onPeriodChange={setPeriod}
    />
  );
}
