// @ts-ignore - Vitest runs this test in Node and can read TS source from disk.
import { readFileSync } from "node:fs";
// @ts-ignore - Vitest runs this test in Node and can resolve file URLs.
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const appSourcePath = fileURLToPath(new URL("./App.tsx", import.meta.url));

describe("Profile popover window movement", () => {
  it("does not expose a frontend native window drag handler", () => {
    const source = readFileSync(appSourcePath, "utf8");

    expect(source).not.toContain("startDragging");
    expect(source).not.toContain("getCurrentWindow");
  });
});
