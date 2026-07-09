// @ts-ignore - Vitest runs this test in Node and can read local config dirs.
import { existsSync } from "node:fs";
// @ts-ignore - Vitest runs this test in Node and can resolve file URLs.
import { fileURLToPath } from "node:url";
// @ts-ignore - Vite can load JSON in tests without changing the app tsconfig.
import tauriConfig from "../../src-tauri/tauri.conf.json";
import { describe, expect, it } from "vitest";

type TauriConfig = {
  app?: {
    windows?: Array<{
      label?: string;
      title?: string;
      width?: number;
      height?: number;
      decorations?: boolean;
      transparent?: boolean;
      visible?: boolean;
      alwaysOnTop?: boolean;
      resizable?: boolean;
    }>;
  };
  bundle?: {
    icon?: string[];
    resources?: string[];
  };
};

describe("Tauri window drag capability", () => {
  it("does not grant native window dragging to the fixed menubar Profile popover", () => {
    const windowDragCapabilityPath = fileURLToPath(
      new URL("../../src-tauri/capabilities/window-drag.json", import.meta.url),
    );

    expect(existsSync(windowDragCapabilityPath)).toBe(false);
  });
});

describe("Tauri profile window config", () => {
  it("uses a hidden menubar Profile popover window", () => {
    const config = tauriConfig as TauriConfig;
    const mainWindow = config.app?.windows?.find((window) => window.title === "TokenFire");

    expect(mainWindow?.width).toBe(428);
    expect(mainWindow?.height).toBe(572);
    expect(mainWindow?.label).toBe("main");
    expect(mainWindow?.decorations).toBe(false);
    expect(mainWindow?.transparent).toBe(true);
    expect(mainWindow?.visible).toBe(false);
    expect(mainWindow?.alwaysOnTop).toBe(true);
    expect(mainWindow?.resizable).toBe(false);
  });
});

describe("Tauri bundle icon config", () => {
  it("uses the generated TokenFire app icon", () => {
    const config = tauriConfig as TauriConfig;

    expect(config.bundle?.icon).toContain("icons/icon.png");
    expect(config.bundle?.icon).toContain("icons/icon.icns");
  });

  it("packages the menubar tray icon as a runtime resource", () => {
    const config = tauriConfig as TauriConfig;

    expect(config.bundle?.resources).toContain("icons/tray-icon.png");
  });
});
