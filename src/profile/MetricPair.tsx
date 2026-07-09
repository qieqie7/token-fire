import type { PeriodProfileSummary } from "./types";
import { formatCny, formatCompactTokens } from "./format";

export function MetricPair({ period }: { period: PeriodProfileSummary | null }) {
  return (
    <div className="profile-metric-grid">
      <section className="profile-metric">
        <div className="profile-label">估算成本</div>
        <strong>{period ? formatCny(period.estimated_cost) : "估算不可用"}</strong>
        <span>仅供参考</span>
      </section>
      <section className="profile-metric">
        <div className="profile-label">Token 数</div>
        <strong>{period ? formatCompactTokens(period.total_tokens) : "--"}</strong>
        <span>所选周期</span>
      </section>
    </div>
  );
}
