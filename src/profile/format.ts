export function formatCny(value: number): string {
  const safeValue = Number.isFinite(value) ? Math.max(0, value) : 0;
  if (safeValue > 0 && safeValue < 0.01) return "<¥0.01";
  return `¥${safeValue.toLocaleString("en-US", {
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
  })}`;
}

export function formatCompactTokens(tokens: number): string {
  const safeTokens = Number.isFinite(tokens) ? Math.max(0, tokens) : 0;
  if (safeTokens >= 1_000_000_000) return `${(safeTokens / 1_000_000_000).toFixed(2)}B`;
  if (safeTokens >= 1_000_000) return `${(safeTokens / 1_000_000).toFixed(2)}M`;
  if (safeTokens >= 1_000) return `${(safeTokens / 1_000).toFixed(2)}K`;
  return `${Math.round(safeTokens)}`;
}

export function formatPercent(value: number): string {
  const safeValue = Number.isFinite(value) ? Math.max(0, value) : 0;
  return `${Math.round(safeValue * 100)}%`;
}

export function formatLocalDate(value: string): string {
  return value.slice(5);
}
