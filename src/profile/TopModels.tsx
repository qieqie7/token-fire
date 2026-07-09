import type { RankedProfileBreakdown } from "./types";
import { formatCompactTokens } from "./format";

function isOtherModel(row: RankedProfileBreakdown) {
  return row.key === "other" || row.label === "Other";
}

function displayModelLabel(row: RankedProfileBreakdown) {
  return isOtherModel(row) ? "其他模型" : row.label;
}

export function TopModels({ rows }: { rows: RankedProfileBreakdown[] }) {
  const visible = rows.filter((row) => !isOtherModel(row)).slice(0, 5);
  const collapsed = rows.filter((row) => isOtherModel(row) || !visible.includes(row));
  const collapsedTotals = collapsed.reduce(
    (total, row) => ({
      estimated_cost: total.estimated_cost + row.estimated_cost,
      total_tokens: total.total_tokens + row.total_tokens,
      share: total.share + row.share,
    }),
    { estimated_cost: 0, total_tokens: 0, share: 0 },
  );

  return (
    <section className="profile-panel profile-list">
      <div className="profile-section-head">
        <span>模型来源</span>
        <span>按 token 前 5</span>
      </div>
      {visible.map((row) => (
        <div className="profile-ranked-row" key={row.key}>
          <span className="profile-ranked-row__name">{displayModelLabel(row)}</span>
          <span className="profile-ranked-row__value">{formatCompactTokens(row.total_tokens)}</span>
          <span className="profile-ranked-row__track">
            <span style={{ width: `${Math.max(2, row.share * 100)}%` }} />
          </span>
        </div>
      ))}
      {collapsed.length > 0 ? (
        <div className="profile-ranked-row profile-ranked-row--muted">
          <span className="profile-ranked-row__name">其他模型</span>
          <span className="profile-ranked-row__value">{formatCompactTokens(collapsedTotals.total_tokens)}</span>
        </div>
      ) : null}
    </section>
  );
}
