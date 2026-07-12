import type { RankedProfileBreakdown } from "./types";

const TOP_SOURCE_LIMIT = 5;
const SOURCE_COLORS = ["#ff8a34", "#f6c85f", "#6ee7b7", "#60a5fa", "#c084fc", "#3f4652"] as const;

function displayLabel(label: string) {
  if (label === "Unknown") return "未知";
  if (label === "Other") return "其他";
  return label;
}

function compactRows(rows: RankedProfileBreakdown[]) {
  const visible = rows.slice(0, TOP_SOURCE_LIMIT);
  const remainder = rows.slice(TOP_SOURCE_LIMIT);
  if (remainder.length === 0) return visible;

  const estimatedCost = remainder.reduce((total, row) => total + row.estimated_cost, 0);
  const totalTokens = remainder.reduce((total, row) => total + row.total_tokens, 0);
  const share = remainder.reduce((total, row) => total + row.share, 0);

  return [
    ...visible,
    {
      key: "other",
      label: "其他",
      estimated_cost: estimatedCost,
      total_tokens: totalTokens,
      share,
    },
  ];
}

function pieGradient(rows: RankedProfileBreakdown[]) {
  if (rows.length === 0) {
    return "conic-gradient(#3f4652 0 100%)";
  }

  let cursor = 0;
  const segments = rows.map((row, index) => {
    const start = cursor;
    const end = index === rows.length - 1 ? 100 : Math.min(100, cursor + row.share * 100);
    cursor = end;
    return `${SOURCE_COLORS[index]} ${start.toFixed(2)}% ${end.toFixed(2)}%`;
  });

  return `conic-gradient(${segments.join(", ")})`;
}

export function AppSources({ rows }: { rows: RankedProfileBreakdown[] }) {
  const visibleRows = compactRows(rows);

  return (
    <section className="profile-panel profile-sources" aria-label="应用来源">
      <div className="profile-section-head">
        <span>应用来源</span>
        <span>按 token 前 {TOP_SOURCE_LIMIT}</span>
      </div>
      <div className="profile-source-body">
        <div
          className="profile-source-pie"
          role="img"
          aria-label="应用来源占比"
          style={{ background: pieGradient(visibleRows) }}
        />
        <div className="profile-source-legend">
          {(visibleRows.length > 0 ? visibleRows : [{ key: "unknown", label: "未知" }]).map((row, index) => (
            <span className="profile-source-legend__row" key={row.key}>
              <i style={{ background: SOURCE_COLORS[index] }} />
              <span>{displayLabel(row.label)}</span>
            </span>
          ))}
        </div>
      </div>
    </section>
  );
}
