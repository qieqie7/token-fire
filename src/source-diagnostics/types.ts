export type DiagnosticStageKey = "participation" | "capture" | "signal" | "extraction" | "storage";
export type DiagnosticStatus = "ok" | "warning" | "error" | "disabled" | "unknown" | "not_applicable";
export type DiagnosticHeadline =
  | "connected"
  | "disabled"
  | "pending_verification"
  | "capture_not_ready"
  | "signal_not_seen"
  | "token_not_extracted"
  | "not_stored"
  | "configuration_error"
  | "runtime_error";

export interface DiagnosticSummary {
  connected: number;
  attention: number;
  disabled: number;
}

export interface DiagnosticDisplaySummary {
  statusText: string;
  detailText: string;
  noteText?: string;
}

export interface DiagnosticStage {
  key: DiagnosticStageKey;
  label: string;
  status: DiagnosticStatus;
  summary: string;
  evidence?: string;
  checkedAt?: string;
}

export interface DiagnosticBreak {
  stage: DiagnosticStageKey;
  title: string;
  evidence: string;
  impact: string;
}

export interface DiagnosticEvidenceItem {
  label: string;
  value: string;
  status?: "ok" | "warning" | "error" | "muted";
}

export interface DiagnosticEvidenceGroup {
  title: string;
  items: DiagnosticEvidenceItem[];
}

export interface DiagnosticAction {
  id: "refresh" | "open_logs" | "copy_debug_bundle" | "reinstall_hook";
  label: string;
  enabled: boolean;
  reason?: string;
}

export interface SourceDiagnostic {
  source: "traex" | "codex" | "claude" | "cursor";
  displayName: string;
  optional: boolean;
  headline: DiagnosticHeadline;
  displaySummary: DiagnosticDisplaySummary;
  trustSummary: string;
  primaryBreak?: DiagnosticBreak;
  chain: DiagnosticStage[];
  evidence: DiagnosticEvidenceGroup[];
  actions: DiagnosticAction[];
}

export interface SourceDiagnosticsSnapshot {
  generatedAt: string;
  summary: DiagnosticSummary;
  sources: SourceDiagnostic[];
}
