import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { ReviewTypeFilter } from "../api";
import { S } from "../i18n";
import { DuplicateReviewPager } from "./Review";

describe("duplicate review pagination", () => {
  // Only one pair matches Same type; one untyped pair belongs only to All.
  const totals = { any: 9, same: 1, conflict: 7 };
  const render = (types: ReviewTypeFilter, page = 0) =>
    renderToStaticMarkup(createElement(DuplicateReviewPager, { totals, types, page, onPage: () => {} }));

  it("does not offer another page for the only matching pair", () => {
    expect(render("same")).toBe("");
  });

  it("counts all pairs only in All and reaches the final filtered pair", () => {
    expect(render("any", 1)).toContain(S.library.pageOf(7, 9, 9));
    expect(render("conflict")).toContain(S.library.pageOf(1, 6, 7));
    expect(render("conflict", 1)).toContain(S.library.pageOf(7, 7, 7));
  });
});
