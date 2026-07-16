import type { PointerEvent } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { Popover, composeEventHandlers, isRenderablePopoverContent } from "./Popover";

const virtualReference = {
  getBoundingClientRect: () => ({
    x: 0,
    y: 0,
    top: 0,
    left: 0,
    right: 10,
    bottom: 10,
    width: 10,
    height: 10,
  }),
};

describe("Popover", () => {
  it("does not render overlay content while closed", () => {
    const html = renderToStaticMarkup(
      <Popover title="说明" content="隐藏内容" open={false} reference={null}>
        <button type="button">查看</button>
      </Popover>,
    );

    expect(html).toContain("查看");
    expect(html).not.toContain("隐藏内容");
    expect(html).not.toContain("tf-popover");
  });

  it("renders controlled reference content with TokenFire-owned classes", () => {
    const html = renderToStaticMarkup(
      <Popover open reference={virtualReference} title="07-16" content={<span>9.40M token</span>} />,
    );

    expect(html).toContain("tf-popover");
    expect(html).toContain("tf-popover__body");
    expect(html).toContain("tf-popover__title");
    expect(html).toContain("tf-popover__content");
    expect(html).toContain("07-16");
    expect(html).toContain("9.40M token");
  });

  it("supports overlayClassName without replacing base classes", () => {
    const html = renderToStaticMarkup(
      <Popover open reference={virtualReference} overlayClassName="profile-readout" content="内容" />,
    );

    expect(html).toContain("tf-popover");
    expect(html).toContain("profile-readout");
  });

  it("calls getPopupContainer with the current trigger node", () => {
    const container = {} as HTMLElement;
    const getPopupContainer = vi.fn(() => container);

    renderToStaticMarkup(
      <Popover open reference={virtualReference} getPopupContainer={getPopupContainer} content="内容" />,
    );

    expect(getPopupContainer).toHaveBeenCalledWith(null);
  });

  it("treats null false and empty string as non-renderable content", () => {
    expect(isRenderablePopoverContent(null)).toBe(false);
    expect(isRenderablePopoverContent(false)).toBe(false);
    expect(isRenderablePopoverContent("")).toBe(false);
    expect(isRenderablePopoverContent(0)).toBe(true);
    expect(isRenderablePopoverContent("内容")).toBe(true);
  });

  it("composes injected trigger handlers after caller handlers", () => {
    const calls: string[] = [];
    const userHandler = vi.fn(() => calls.push("user"));
    const injectedHandler = vi.fn(() => calls.push("injected"));
    const handler = composeEventHandlers(userHandler, injectedHandler);

    handler({} as PointerEvent<HTMLElement>);

    expect(calls).toEqual(["user", "injected"]);
  });
});
