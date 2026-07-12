// @ts-ignore - Vitest runs this test in Node and can read CSS from disk.
import { readFileSync } from "node:fs";
// @ts-ignore - Vitest runs this test in Node and can resolve file URLs.
import { fileURLToPath } from "node:url";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { SourceDiagnostics, nextSelectedSource, resolveSelectedSource } from "./SourceDiagnostics";
import type { SourceDiagnosticsSnapshot } from "./types";

const sourceDiagnosticsCssPath = fileURLToPath(new URL("./source-diagnostics.css", import.meta.url));

const claudeSource: SourceDiagnosticsSnapshot["sources"][number] = {
  source: "claude",
  displayName: "Claude",
  optional: true,
  headline: "capture_not_ready",
  displaySummary: {
    statusText: "捕获未就绪",
    detailText: "采集程序不可用",
    noteText: "展开查看问题证据",
  },
  trustSummary: "捕获未就绪 · 最近没有 Claude transcript 进入统计",
  primaryBreak: {
    stage: "capture",
    title: "捕获未就绪",
    evidence: "capture unavailable",
    impact: "最近没有 Claude transcript 进入统计",
  },
  chain: [
    { key: "participation", label: "参与采集", status: "ok", summary: "启用" },
    { key: "capture", label: "捕获就绪", status: "warning", summary: "未就绪" },
    { key: "signal", label: "看到信号", status: "unknown", summary: "无信号" },
    { key: "extraction", label: "提取 token", status: "unknown", summary: "未执行" },
    { key: "storage", label: "写入统计", status: "unknown", summary: "无 token observation" },
  ],
  evidence: [
    {
      title: "接入状态",
      items: [
        { label: "采集配置", value: "未安装", status: "warning" },
        { label: "采集程序", value: "缺失", status: "error" },
      ],
    },
  ],
  actions: [{ id: "refresh", label: "刷新", enabled: true }],
};

const rawPrimaryBreakSentinel =
  "RAW_PRIMARY_BREAK_SENTINEL=/Users/example/private/session.jsonl; empty_reason=no_complete_jsonl_rows";

const cursorSource: SourceDiagnosticsSnapshot["sources"][number] = {
  source: "cursor",
  displayName: "Cursor",
  optional: true,
  headline: "token_not_extracted",
  displaySummary: {
    statusText: "未提取到 token",
    detailText: "最近采集未产生 token usage",
    noteText: "展开查看采集结果",
  },
  trustSummary: "legacy cursor trust summary must not render",
  primaryBreak: {
    stage: "extraction",
    title: "未提取到 token",
    evidence: rawPrimaryBreakSentinel,
    impact: "最近没有 token observation 进入统计",
  },
  chain: [
    { key: "participation", label: "参与采集", status: "ok", summary: "启用" },
    { key: "capture", label: "捕获就绪", status: "ok", summary: "transcript 可读" },
    { key: "signal", label: "看到信号", status: "ok", summary: "4 分钟前" },
    { key: "extraction", label: "提取 token", status: "warning", summary: "未发现 token usage rows" },
    { key: "storage", label: "写入统计", status: "unknown", summary: "无 token observation" },
  ],
  evidence: [
    {
      title: "最近采集",
      items: [
        { label: "最近捕获", value: "14:33", status: "muted" },
        {
          label: "检查结果",
          value: "没有完整 JSONL 记录，需要等待下一次完整写入",
          status: "warning",
        },
      ],
    },
  ],
  actions: [
    { id: "refresh", label: "刷新", enabled: true },
    { id: "open_logs", label: "打开日志", enabled: true },
    { id: "copy_debug_bundle", label: "复制诊断包", enabled: true },
  ],
};

const codexConnectedWithOutsideWindow: SourceDiagnosticsSnapshot["sources"][number] = {
  source: "codex",
  displayName: "Codex",
  optional: false,
  headline: "connected",
  trustSummary: "legacy trust summary must not render",
  displaySummary: {
    statusText: "已接入",
    detailText: "最近成功写入 1 条 · 12:40",
    noteText: "扫描到 895 条窗口外历史记录，未计入当前统计周期",
  },
  chain: [
    { key: "participation", label: "参与采集", status: "ok", summary: "有证据" },
    { key: "capture", label: "捕获就绪", status: "ok", summary: "有证据" },
    { key: "signal", label: "看到信号", status: "ok", summary: "有证据" },
    { key: "extraction", label: "提取 token", status: "ok", summary: "有证据" },
    { key: "storage", label: "写入统计", status: "ok", summary: "有证据" },
  ],
  evidence: [
    {
      title: "当前判断",
      items: [
        { label: "状态", value: "可信", status: "ok" },
        { label: "依据", value: "最近成功写入 1 条", status: "muted" },
      ],
    },
    {
      title: "接入状态",
      items: [
        { label: "采集配置", value: "已安装", status: "ok" },
        { label: "采集程序", value: "可用", status: "ok" },
      ],
    },
    {
      title: "最近采集",
      items: [
        { label: "最近捕获", value: "12:43", status: "muted" },
        { label: "本次写入", value: "1 条", status: "ok" },
        { label: "重复记录", value: "0 条", status: "muted" },
        { label: "窗口外记录", value: "895 条", status: "muted" },
      ],
    },
    {
      title: "数据库证据",
      items: [
        { label: "数据库最近写入", value: "12:40", status: "muted" },
        { label: "可信状态", value: "可信", status: "ok" },
      ],
    },
    {
      title: "最新问题",
      items: [{ label: "最新问题", value: "无", status: "muted" }],
    },
  ],
  actions: [
    { id: "refresh", label: "刷新", enabled: true },
    { id: "open_logs", label: "打开日志", enabled: true },
    { id: "copy_debug_bundle", label: "复制诊断包", enabled: true },
  ],
};

const snapshot: SourceDiagnosticsSnapshot = {
  generatedAt: "2026-07-10T10:00:00Z",
  summary: { connected: 2, attention: 1, disabled: 1 },
  sources: [cursorSource],
};

const multiSourceSnapshot: SourceDiagnosticsSnapshot = {
  ...snapshot,
  sources: [claudeSource, cursorSource],
};

describe("SourceDiagnostics", () => {
  it("renders connected source with latest no-op and recent success evidence", () => {
    const html = renderToStaticMarkup(
      <SourceDiagnostics
        snapshot={{
          generatedAt: "2026-07-10T04:45:00Z",
          summary: { connected: 1, attention: 0, disabled: 0 },
          sources: [codexConnectedWithOutsideWindow],
        }}
        loading={false}
        error={false}
        onRefresh={() => {}}
        onAction={vi.fn()}
        initialSelectedSource="codex"
      />,
    );

    expect(html).toContain("Codex");
    expect(html).toContain("已接入");
    expect(html).toContain("最近成功写入 1 条 · 12:40");
    expect(html).toContain("扫描到 895 条窗口外历史记录，未计入当前统计周期");
    expect(html).toContain("最近采集");
    expect(html).toContain("本次写入");
    expect(html).toContain("窗口外记录");
    expect(html).toContain("数据库最近写入");
    expect(html).toContain('data-tone="muted"');
    expect(html).not.toContain("legacy trust summary must not render");
    for (const rawKey of [
      "latest_signal_at",
      "last_signal_at",
      "latest_signal_result",
      "latest_success_trust",
      "latest_storage_by_source",
      "latest_hard_failure_reason",
    ]) {
      expect(html).not.toContain(rawKey);
    }
  });

  it("renders pending verification copy for expired success", () => {
    const pendingSource: SourceDiagnosticsSnapshot["sources"][number] = {
      source: "codex",
      displayName: "Codex",
      optional: false,
      headline: "pending_verification",
      displaySummary: {
        statusText: "待验证",
        detailText: "最近成功已过期",
        noteText: "需要新的成功写入验证",
      },
      trustSummary: "最近成功已过期 · 需要新的成功写入验证",
      primaryBreak: {
        stage: "storage",
        title: "待验证",
        evidence: "最近成功已过期",
        impact: "近期未验证成功写入",
      },
      chain: [
        { key: "participation", label: "参与采集", status: "ok", summary: "有证据" },
        { key: "capture", label: "捕获就绪", status: "ok", summary: "有证据" },
        { key: "signal", label: "看到信号", status: "unknown", summary: "未知" },
        { key: "extraction", label: "提取 token", status: "unknown", summary: "未知" },
        { key: "storage", label: "写入统计", status: "warning", summary: "需要关注" },
      ],
      evidence: [
        {
          title: "当前判断",
          items: [{ label: "状态", value: "待验证", status: "warning" }],
        },
        {
          title: "数据库证据",
          items: [
            { label: "数据库最近写入", value: "10:40", status: "muted" },
            { label: "可信状态", value: "已过期", status: "warning" },
          ],
        },
      ],
      actions: [
        { id: "refresh", label: "刷新", enabled: true },
        { id: "open_logs", label: "打开日志", enabled: true },
        { id: "copy_debug_bundle", label: "复制诊断包", enabled: true },
      ],
    };
    const html = renderToStaticMarkup(
      <SourceDiagnostics
        snapshot={{
          generatedAt: "2026-07-10T11:00:00Z",
          summary: { connected: 0, attention: 1, disabled: 0 },
          sources: [pendingSource],
        }}
        loading={false}
        error={false}
        onRefresh={() => {}}
        onAction={vi.fn()}
      />,
    );

    expect(html).toContain("待验证");
    expect(html).toContain("最近成功已过期");
    expect(html).toContain("需要新的成功写入验证");
    expect(html).not.toContain("latest_success_trust");
  });

  it("renders the five-stage evidence chain and Cursor extraction break", () => {
    const html = renderToStaticMarkup(
      <SourceDiagnostics snapshot={snapshot} loading={false} error={false} onRefresh={() => {}} onAction={vi.fn()} />,
    );

    expect(html).toContain("接入诊断");
    expect(html).toContain("检查使用数据从来源到统计的证据链");
    expect(html).toContain("参与采集");
    expect(html).toContain("捕获就绪");
    expect(html).toContain("看到信号");
    expect(html).toContain("提取 token");
    expect(html).toContain("写入统计");
    expect(html).toContain("未提取到 token");
    expect(html).toContain("最近采集未产生 token usage");
    expect(html).toContain("展开查看采集结果");
    expect(html).not.toContain("legacy cursor trust summary must not render");
    expect(html).toContain('type="button"');
    expect(html).toContain('aria-pressed="false"');
    expect(html).toContain('aria-expanded="false"');
    expect(html).not.toContain("断点：extraction");
    expect(html).not.toContain("Health Center");
    expect(html).not.toContain("Settings");
  });

  it("does not render raw primary-break evidence", () => {
    const html = renderToStaticMarkup(
      <SourceDiagnostics
        snapshot={snapshot}
        loading={false}
        error={false}
        onRefresh={() => {}}
        onAction={vi.fn()}
        initialSelectedSource="cursor"
      />,
    );

    expect(html).toContain("未提取到 token");
    expect(html).toContain("最近没有 token observation 进入统计");
    expect(html).not.toContain(rawPrimaryBreakSentinel);
  });

  it("uses the backend display summary for the expanded title", () => {
    // statusText 与旧前端 headlineLabels 文案有意不同，锁定展开标题走后端语义、不再本地映射。
    const signalNotSeenSource: SourceDiagnosticsSnapshot["sources"][number] = {
      source: "codex",
      displayName: "Codex",
      optional: false,
      headline: "signal_not_seen",
      displaySummary: {
        statusText: "未看到信号",
        detailText: "近期尚未捕获来源事件",
        noteText: "展开查看接入状态",
      },
      trustSummary: "legacy trust summary must not render",
      chain: [
        { key: "participation", label: "参与采集", status: "ok", summary: "启用" },
        { key: "capture", label: "捕获就绪", status: "ok", summary: "transcript 可读" },
        { key: "signal", label: "看到信号", status: "warning", summary: "无信号" },
        { key: "extraction", label: "提取 token", status: "unknown", summary: "未执行" },
        { key: "storage", label: "写入统计", status: "unknown", summary: "无 token observation" },
      ],
      evidence: [
        {
          title: "接入状态",
          items: [
            { label: "采集配置", value: "已安装", status: "ok" },
            { label: "采集程序", value: "可用", status: "ok" },
          ],
        },
      ],
      actions: [{ id: "refresh", label: "刷新", enabled: true }],
    };
    const html = renderToStaticMarkup(
      <SourceDiagnostics
        snapshot={{
          generatedAt: "2026-07-10T10:00:00Z",
          summary: { connected: 0, attention: 1, disabled: 0 },
          sources: [signalNotSeenSource],
        }}
        loading={false}
        error={false}
        onRefresh={() => {}}
        onAction={vi.fn()}
        initialSelectedSource="codex"
      />,
    );

    expect(html).toContain("Codex · 未看到信号");
    expect(html).not.toContain("最近未看到信号");
  });

  it("starts with every source collapsed", () => {
    const html = renderToStaticMarkup(
      <SourceDiagnostics
        snapshot={multiSourceSnapshot}
        loading={false}
        error={false}
        onRefresh={() => {}}
        onAction={vi.fn()}
      />,
    );

    expect(html).not.toContain('aria-expanded="true"');
    expect(html).not.toContain('data-source-diagnostics-expanded="claude"');
    expect(html).not.toContain('data-source-diagnostics-expanded="cursor"');
  });

  it("allows zero selected sources and at most one expanded source", () => {
    const sourceIds = multiSourceSnapshot.sources.map((source) => source.source);

    expect(resolveSelectedSource(null, sourceIds)).toBeNull();
    expect(nextSelectedSource(null, "claude")).toBe("claude");
    expect(nextSelectedSource("claude", "claude")).toBeNull();
    expect(resolveSelectedSource(nextSelectedSource("claude", "cursor"), sourceIds)).toBe("cursor");
    expect(resolveSelectedSource("missing-source", sourceIds)).toBeNull();
  });

  it("renders expanded source details directly after its card", () => {
    const selected = resolveSelectedSource(nextSelectedSource(null, "claude"), ["claude", "cursor"]);
    const html = renderToStaticMarkup(
      <SourceDiagnostics
        snapshot={multiSourceSnapshot}
        loading={false}
        error={false}
        onRefresh={() => {}}
        onAction={vi.fn()}
        initialSelectedSource={selected}
      />,
    );

    expect(html).toContain('data-source-diagnostics-card="claude"');
    expect(html).toContain('data-source-diagnostics-expanded="claude"');
    expect(html.indexOf('data-source-diagnostics-card="claude"')).toBeLessThan(
      html.indexOf('data-source-diagnostics-expanded="claude"'),
    );
    expect(html.indexOf('data-source-diagnostics-expanded="claude"')).toBeLessThan(
      html.indexOf('data-source-diagnostics-card="cursor"'),
    );
  });

  it("renders loading and error states without settings controls", () => {
    const html = renderToStaticMarkup(
      <SourceDiagnostics snapshot={null} loading={false} error={true} onRefresh={() => {}} onAction={vi.fn()} />,
    );

    expect(html).toContain("接入诊断");
    expect(html).toContain("诊断不可用");
    expect(html).not.toContain("保留天数");
    expect(html).not.toContain("主题");
  });

  it("marks refresh as busy while diagnostics are refreshing", () => {
    const html = renderToStaticMarkup(
      <SourceDiagnostics snapshot={snapshot} loading={true} error={false} onRefresh={() => {}} onAction={vi.fn()} />,
    );

    expect(html).toContain('aria-label="刷新"');
    expect(html).toContain('aria-busy="true"');
    expect(html).toContain('class="source-diagnostics-icon-button"');
    expect(html).toContain('class="source-diagnostics-icon source-diagnostics-icon--spinning"');
    expect(html).not.toContain(
      'class="source-diagnostics-icon-button source-diagnostics-icon-button--spinning"',
    );
  });

  it("keeps long runtime evidence values visible and wrapping", () => {
    const selected = resolveSelectedSource(nextSelectedSource(null, "cursor"), ["cursor"]);
    const html = renderToStaticMarkup(
      <SourceDiagnostics
        snapshot={snapshot}
        loading={false}
        error={false}
        onRefresh={() => {}}
        onAction={vi.fn()}
        initialSelectedSource={selected}
      />,
    );
    const css = readFileSync(sourceDiagnosticsCssPath, "utf8");
    const factValue = cssBlock(css, ".source-diagnostics-fact strong");
    const factLabel = cssBlock(css, ".source-diagnostics-fact > span");

    expect(html).toContain("没有完整 JSONL 记录，需要等待下一次完整写入");
    expect(factValue).toContain("overflow-wrap: anywhere;");
    expect(factValue).toContain("word-break: break-word;");
    expect(factValue).not.toContain("white-space: nowrap;");
    expect(factLabel).toContain("overflow-wrap: anywhere;");
    expect(factLabel).toContain("word-break: break-word;");
  });

  it("keeps the refresh indicator active while loading", () => {
    const css = readFileSync(sourceDiagnosticsCssPath, "utf8");
    const spinner = cssBlock(css, ".source-diagnostics-icon--spinning");

    expect(spinner).toContain("linear infinite");
    expect(css).not.toContain(".source-diagnostics-icon-button--spinning");
    expect(css).toContain("@media (prefers-reduced-motion: reduce)");
  });
});

function cssBlock(css: string, selector: string): string {
  const start = css.indexOf(`${selector} {`);
  expect(start).toBeGreaterThanOrEqual(0);
  const end = css.indexOf("\n}", start);
  expect(end).toBeGreaterThan(start);
  return css.slice(start, end);
}
