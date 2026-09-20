import { describe, expect, it, vi } from "vitest";
import { api, type BusinessRule } from "../api";
import { expressionText, metadataOnly, metadataPatch, readExpression } from "./ruleExpressions";
const a = "00000000-0000-0000-0000-000000000001", b = "00000000-0000-0000-0000-000000000002";
const attrs = [{ id: a, key: "revenue", label: "收入" }, { id: b, key: "cost", label: "成本" }];
const expr = { op: "div", l: { op: "sub", l: { attr: a }, r: { attr: b } }, r: { attr: a } };
const rule: BusinessRule = {
  id: "rule", name: "Margin", subject_type_id: "class", subject_label: "Company",
  conclusion: "computed", conclude_expr: expr, conclude_type_label: null,
  conclude_predicate_label: "Margin", conclude_predicate_id: a,
  conditions: [{ predicate_id: a, op: "gt", operand: 0, group: 4 }], enabled: true, derived_count: 0, capped: 0,
};
describe("reading existing rule expressions", () => {
  it("keeps nested noncommutative structure and resolves IDs", () => {
    expect(expressionText(expr, attrs, "unknown")).toBe("((收入 − 成本) ÷ 收入)");
    const nested = { op: "sub", l: { attr: a }, r: { op: "sub", l: { attr: b }, r: { const: 3 } } };
    expect(expressionText(nested, attrs, "unknown")).toBe("(收入 − (成本 − 3))");
    expect(readExpression(nested)).toEqual(nested);
    expect(expressionText({ const: "2.50" }, attrs, "unknown")).toBe("2.50");
  });
  it("distinguishes repeated labels and keeps missing IDs visible", () => {
    expect(expressionText(expr, attrs.map((x) => ({ ...x, label: "值" })), "unknown")).toBe("((值 (revenue) − 值 (cost)) ÷ 值 (revenue))");
    expect(expressionText({ attr: a }, [], "unknown")).toBe(a);
  });
  it("does not coerce unsupported nodes to zero", () => {
    for (const raw of [null, {}, { const: "" }, { const: Infinity }, { const: "NaN" }, { const: "0x20" }, { op: "sum", l: { attr: a }, r: { const: 1 } }, { attr: a, extra: true }, { attr: "missing" }]) {
      expect(readExpression(raw)).toBeNull();
      expect(expressionText(raw, attrs, "unknown")).toBe("unknown");
    }
  });
  it("uses the store's root-at-zero depth limit", () => {
    let tree: unknown = { attr: a };
    for (let i = 0; i < 4; i++) tree = { op: "add", l: tree, r: { const: 1 } };
    expect(readExpression(tree)).not.toBeNull();
    expect(readExpression({ op: "add", l: tree, r: { const: 1 } })).toBeNull();
  });
  it("guards definitions the constant form cannot round-trip", () => {
    expect(metadataOnly(rule)).toBe(true);
    expect(metadataOnly({ ...rule, conclusion: "typing", conditions: [{ predicate_id: a, op: "gt", operand: expr }] })).toBe(true);
    expect(metadataOnly({ ...rule, conclusion: "attribute", conclude_value: 4 })).toBe(true);
    expect(metadataOnly({ ...rule, conclusion: "attribute", conclude_value: "A" })).toBe(false);
    expect(metadataOnly({ ...rule, conclusion: "typing" })).toBe(false);
  });
  it("sends only metadata via the API adapter, including after an error", async () => {
    const before = JSON.stringify(rule);
    const fetcher = vi.fn().mockResolvedValueOnce(new Response(JSON.stringify({ error: "denied" }), { status: 403 })).mockResolvedValue(new Response(JSON.stringify({ ok: true })));
    vi.stubGlobal("fetch", fetcher);
    try {
      const patch = metadataPatch("New name", "New description");
      await expect(api.updateRule("kb", rule.id, patch)).rejects.toThrow("denied");
      await api.updateRule("kb", rule.id, patch);
      for (const [url, init] of fetcher.mock.calls) {
        expect(url).toBe("/api/v1/kbs/kb/rules/rule");
        expect(init.method).toBe("PATCH");
        expect(JSON.parse(init.body)).toEqual({ name: "New name", description: "New description" });
      }
      expect(JSON.stringify(rule)).toBe(before);
    } finally { vi.unstubAllGlobals(); }
  });
});
