import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { TopModels } from "./TopModels";

describe("TopModels", () => {
  it("renders top five models and collapses the rest", () => {
    const rows = Array.from({ length: 10 }, (_, index) => ({
      key: `model-${index}`,
      label: `Model ${index}`,
      estimated_cost: 100 - index,
      total_tokens: 1_000_000 + index * 100_000,
      share: 0.1,
    }));

    const html = renderToStaticMarkup(<TopModels rows={rows} />);

    expect(html).toContain("Model 0");
    expect(html).toContain("Model 1");
    expect(html).toContain("Model 2");
    expect(html).toContain("Model 3");
    expect(html).toContain("Model 4");
    expect(html).toContain("1.00M");
    expect(html).toContain("1.40M");
    expect(html).toContain("8.50M");
    expect(html).toContain("按 token 前 5");
    expect(html).toContain("其他模型");
    expect(html).not.toContain("¥100.00");
    expect(html).not.toContain("按成本前 5");
    expect(html).not.toContain("Model 5</span>");
  });

  it("folds a backend Other bucket into other models", () => {
    const rows = [
      { key: "model-a", label: "Model A", estimated_cost: 100, total_tokens: 1_000_000, share: 0.3 },
      { key: "model-b", label: "Model B", estimated_cost: 50, total_tokens: 500_000, share: 0.2 },
      { key: "model-c", label: "Model C", estimated_cost: 25, total_tokens: 250_000, share: 0.15 },
      { key: "model-d", label: "Model D", estimated_cost: 12, total_tokens: 120_000, share: 0.12 },
      { key: "other", label: "Other", estimated_cost: 25, total_tokens: 250_000, share: 0.13 },
    ];

    const html = renderToStaticMarkup(<TopModels rows={rows} />);

    expect(html).toContain("其他模型");
    expect(html).not.toContain(">Other</span>");
    expect(html).not.toContain("其他 1 个模型");
  });

  it("merges a backend Other bucket with collapsed ordinary model rows", () => {
    const rows = [
      { key: "model-a", label: "Model A", estimated_cost: 100, total_tokens: 1_000_000, share: 0.25 },
      { key: "model-b", label: "Model B", estimated_cost: 90, total_tokens: 900_000, share: 0.22 },
      { key: "other", label: "Other", estimated_cost: 25, total_tokens: 250_000, share: 0.1 },
      { key: "model-c", label: "Model C", estimated_cost: 80, total_tokens: 800_000, share: 0.2 },
      { key: "model-d", label: "Model D", estimated_cost: 70, total_tokens: 700_000, share: 0.17 },
      { key: "model-e", label: "Model E", estimated_cost: 60, total_tokens: 600_000, share: 0.15 },
      { key: "model-f", label: "Model F", estimated_cost: 40, total_tokens: 400_000, share: 0.1 },
    ];

    const html = renderToStaticMarkup(<TopModels rows={rows} />);

    expect(html.match(/其他模型/g)).toHaveLength(1);
    expect(html).toContain("Model E");
    expect(html).toContain("650.00K");
    expect(html).not.toContain(">Other</span>");
    expect(html).not.toContain("Model F</span>");
  });

  it("labels a mixed collapsed remainder as one aggregate other models row", () => {
    const rows = [
      ...Array.from({ length: 9 }, (_, index) => ({
        key: `model-${index}`,
        label: `Model ${index}`,
        estimated_cost: 100 - index,
        total_tokens: 1_000_000,
        share: 0.1,
      })),
      { key: "other", label: "Other", estimated_cost: 25, total_tokens: 250_000, share: 0.1 },
    ];

    const html = renderToStaticMarkup(<TopModels rows={rows} />);

    expect(html).toContain("其他模型");
    expect(html).toContain("4.25M");
    expect(html).not.toContain(">Other</span>");
    expect(html).not.toContain("Model 5</span>");
  });

  it("keeps compact token units at two decimals", () => {
    const rows = [
      { key: "billions", label: "Billions", estimated_cost: 100, total_tokens: 1_234_000_000, share: 0.7 },
      { key: "millions", label: "Millions", estimated_cost: 50, total_tokens: 13_100_000, share: 0.2 },
      { key: "thousands", label: "Thousands", estimated_cost: 10, total_tokens: 9_400, share: 0.1 },
    ];

    const html = renderToStaticMarkup(<TopModels rows={rows} />);

    expect(html).toContain("1.23B");
    expect(html).toContain("13.10M");
    expect(html).toContain("9.40K");
  });

  it("keeps sub-thousand token counts as integers", () => {
    const rows = [
      { key: "hundreds", label: "Hundreds", estimated_cost: 1, total_tokens: 999, share: 0.5 },
      { key: "zero", label: "Zero", estimated_cost: 0, total_tokens: 0, share: 0.5 },
    ];

    const html = renderToStaticMarkup(<TopModels rows={rows} />);

    expect(html).toContain(">999</span>");
    expect(html).toContain(">0</span>");
    expect(html).not.toContain("999.00");
    expect(html).not.toContain("0.00");
  });
});
