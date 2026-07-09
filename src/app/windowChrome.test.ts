// @ts-ignore - Vitest runs this test in Node and can read CSS from disk.
import { readFileSync } from "node:fs";
// @ts-ignore - Vitest runs this test in Node and can resolve file URLs.
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const stylePath = fileURLToPath(new URL("../style.css", import.meta.url));

describe("Profile window chrome", () => {
  it("does not keep widget shell drag styling", () => {
    const styles = readFileSync(stylePath, "utf8");

    expect(styles).not.toContain(".widget-shell");
    expect(styles).not.toContain(".live-counter");
    expect(styles).not.toContain("cursor: grab");
  });
});
