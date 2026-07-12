import { useState } from "react";
import type {
  DiagnosticAction,
  DiagnosticStatus,
  SourceDiagnostic,
  SourceDiagnosticsSnapshot,
} from "./types";
import "./source-diagnostics.css";

function statusClass(status: DiagnosticStatus): string {
  return `source-diagnostics-step source-diagnostics-step--${status}`;
}

function refreshIconClass(loading: boolean): string {
  return loading ? "source-diagnostics-icon source-diagnostics-icon--spinning" : "source-diagnostics-icon";
}

export function resolveSelectedSource(selectedSource: string | null, sourceIds: readonly string[]): string | null {
  if (selectedSource && sourceIds.includes(selectedSource)) {
    return selectedSource;
  }
  return null;
}

export function nextSelectedSource(selectedSource: string | null, source: string): string | null {
  return selectedSource === source ? null : source;
}

interface SourceDiagnosticsProps {
  snapshot: SourceDiagnosticsSnapshot | null;
  loading: boolean;
  error: boolean;
  onRefresh: () => void;
  onAction: (action: DiagnosticAction) => void;
  initialSelectedSource?: string | null;
}

export function SourceDiagnostics({
  snapshot,
  loading,
  error,
  onRefresh,
  onAction,
  initialSelectedSource = null,
}: SourceDiagnosticsProps) {
  const [selectedSource, setSelectedSource] = useState<string | null>(initialSelectedSource);
  const selectedSourceId = resolveSelectedSource(
    selectedSource,
    snapshot?.sources.map((source) => source.source) ?? [],
  );
  const selected = snapshot?.sources.find((source) => source.source === selectedSourceId) ?? null;

  return (
    <main className="source-diagnostics">
      <header className="source-diagnostics-header">
        <div className="source-diagnostics-brand">
          <div className="source-diagnostics-title">
            <span className="source-diagnostics-flame" />
            接入诊断
          </div>
          <div className="source-diagnostics-subtitle">检查使用数据从来源到统计的证据链</div>
        </div>
        <button
          className="source-diagnostics-icon-button"
          type="button"
          onClick={onRefresh}
          aria-label="刷新"
          aria-busy={loading}
        >
          <span className={refreshIconClass(loading)} aria-hidden="true">
            ↻
          </span>
        </button>
      </header>

      {snapshot ? (
        <section className="source-diagnostics-summary" aria-label="诊断摘要">
          <span>
            已接入 <strong>{snapshot.summary.connected}</strong>
          </span>
          <span>
            需要关注 <strong>{snapshot.summary.attention}</strong>
          </span>
          <span>
            未启用 <strong>{snapshot.summary.disabled}</strong>
          </span>
        </section>
      ) : null}

      {error ? <section className="source-diagnostics-panel">诊断不可用</section> : null}
      {loading && !snapshot ? <section className="source-diagnostics-panel">正在检查</section> : null}

      {snapshot ? (
        <section className="source-diagnostics-body">
          <div className="source-diagnostics-list">
            {snapshot.sources.map((source) => {
              const selectedCard = selected?.source === source.source;
              return (
                <div className="source-diagnostics-source" key={source.source}>
                  <button
                    aria-pressed={selectedCard}
                    aria-expanded={selectedCard}
                    className="source-diagnostics-card"
                    data-source-diagnostics-card={source.source}
                    type="button"
                    onClick={() => setSelectedSource(nextSelectedSource(selectedSourceId, source.source))}
                  >
                    <div className="source-diagnostics-card-head">
                      <strong>{source.displayName}</strong>
                      <span>{source.displaySummary.statusText}</span>
                    </div>
                    <div className="source-diagnostics-chain">
                      {source.chain.map((stage) => (
                        <span className={statusClass(stage.status)} key={stage.key}>
                          {stage.label}
                        </span>
                      ))}
                    </div>
                    <div className="source-diagnostics-card-copy">
                      <strong>{source.displaySummary.detailText}</strong>
                      {source.displaySummary.noteText ? (
                        <small className="source-diagnostics-card-note">{source.displaySummary.noteText}</small>
                      ) : null}
                    </div>
                  </button>
                  {selectedCard ? <SourceDiagnosticsExpanded source={source} onAction={onAction} /> : null}
                </div>
              );
            })}
          </div>
        </section>
      ) : null}
    </main>
  );
}

interface SourceDiagnosticsExpandedProps {
  source: SourceDiagnostic;
  onAction: (action: DiagnosticAction) => void;
}

function SourceDiagnosticsExpanded({ source, onAction }: SourceDiagnosticsExpandedProps) {
  return (
    <section
      className="source-diagnostics-expanded"
      data-source-diagnostics-expanded={source.source}
      data-expanded="true"
    >
      <div className="source-diagnostics-expanded-inner">
        <h2>
          {source.displayName} · {source.displaySummary.statusText}
        </h2>
        {source.primaryBreak ? (
          <section className="source-diagnostics-panel">
            <h3>{source.primaryBreak.title}</h3>
            <div className="source-diagnostics-fact">
              <span>影响</span>
              <strong>{source.primaryBreak.impact}</strong>
            </div>
          </section>
        ) : null}
        {source.evidence.map((group) => (
          <section className="source-diagnostics-panel" key={group.title}>
            <h3>{group.title}</h3>
            {group.items.map((item) => (
              <div
                className="source-diagnostics-fact"
                data-tone={item.status ?? "muted"}
                key={`${group.title}-${item.label}`}
              >
                <span>{item.label}</span>
                <strong>{item.value}</strong>
              </div>
            ))}
          </section>
        ))}
        <section className="source-diagnostics-actions">
          {source.actions.map((action) => (
            <button
              key={action.id}
              type="button"
              disabled={!action.enabled}
              onClick={() => onAction(action)}
            >
              {action.label}
            </button>
          ))}
        </section>
      </div>
    </section>
  );
}
