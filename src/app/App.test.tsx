import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { App } from "./App";

vi.mock("./useProfileSummary", () => ({
  useProfileSummary: () => ({ summary: null, loading: false, error: false }),
}));

describe("App", () => {
  it("defaults the profile period to this_month", () => {
    const html = renderToStaticMarkup(<App />);

    expect(html).toMatch(
      /<button\b(?=[^>]*role="tab")(?=[^>]*aria-selected="true")[^>]*>当月<\/button>/,
    );
  });
});
