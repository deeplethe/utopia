import { describe, expect, it } from "vitest";
import { draftFromExpression, previewExpression, type ExpressionDraft } from "./expressionDraft";
import { readExpression } from "./ruleExpressions";

const a = "00000000-0000-0000-0000-000000000001";
const b = "00000000-0000-0000-0000-000000000002";
const c = "00000000-0000-0000-0000-000000000003";
const ids = new Set([a, b, c]);
describe("expression draft fidelity", () => {
  it("preserves right-nested subtraction and division on reopen and inner edits", () => {
    for (const op of ["sub", "div"] as const) {
      const expr = { op, l: { attr: a }, r: { op, l: { attr: b }, r: { attr: c } } };
      const draft = draftFromExpression(expr);
      expect(previewExpression(draft, ids)).toEqual(expr);
      if (!("op" in draft) || !("op" in draft.r)) throw new Error("lost tree");
      const changed = { ...draft, r: { ...draft.r, r: { const: "2.5" } } };
      expect(previewExpression(changed, ids)).toEqual({ ...expr, r: { ...expr.r, r: { const: "2.5" } } });
      expect(draft).toEqual(expr);
    }
  });
  it("never substitutes zero or a first attribute for incomplete and missing inputs", () => {
    for (const text of ["", "-", "+", "1e", "Infinity", "NaN", "1.2.3"]) {
      expect(previewExpression({ const: text }, ids)).toBeNull();
    }
    expect(previewExpression({ attr: "" }, ids)).toBeNull();
    expect(previewExpression({ attr: a }, new Set())).toBeNull();
    expect(previewExpression({ const: "0" }, ids)).toEqual({ const: "0" });
  });
  it("uses the existing structural parser depth and unknown-shape boundary", () => {
    let draft: ExpressionDraft = { attr: a };
    for (let i = 0; i < 4; i++) draft = { op: "add", l: draft, r: { const: "1" } };
    expect(previewExpression(draft, ids)).not.toBeNull();
    expect(previewExpression({ op: "add", l: draft, r: { attr: a } }, ids)).toBeNull();
    expect(readExpression({ attr: a, future: true })).toBeNull();
  });
});
