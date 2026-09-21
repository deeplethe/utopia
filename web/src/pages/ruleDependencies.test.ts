import { describe, expect, it, vi } from "vitest";
import { Children, isValidElement, type ReactElement, type ReactNode } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import type { BusinessRule } from "../api";
import { ruleDependencies } from "./ruleDependencies";
import { RuleDependencyList } from "./RulesPanel";
import { en } from "../i18n/en";
import { zh } from "../i18n/zh";
const r = (id: string, patch: Partial<BusinessRule> = {}): BusinessRule => ({
  id, name: id, subject_type_id: "root", subject_label: "Class", conclusion: "attribute",
  conclude_predicate_id: `out-${id}`, conclude_type_label: null, conclude_predicate_label: "Value",
  conditions: [{ predicate_id: "input", op: "gt", operand: 100 }], enabled: true, derived_count: 0, capped: 0, ...patch,
});
const classes = [{ id: "root", parents: [] }, { id: "child", parents: ["root"] }, { id: "leaf", parents: ["child"] }];
describe("potential definition dependencies", () => {
  it("finds every producer and consumer by ID, regardless of labels, thresholds or enabled state", () => {
    const rules = [r("a", { conclude_predicate_id: "p" }), r("b", { conclude_predicate_id: "p", enabled: false }), r("c", { conditions: [{ predicate_id: "p", op: "lt", operand: -999 }] }), r("d", { conclude_predicate_id: "q" })];
    const before = JSON.stringify(rules);
    const { links } = ruleDependencies(rules, classes);
    expect([...links.get("c")!.producers]).toEqual(["a", "b"]);
    expect([...links.get("a")!.consumers]).toEqual(["c"]);
    expect(links.get("d")!.consumers.size).toBe(0);
    expect(JSON.stringify(rules)).toBe(before);
  });
  it("reads nested computed expressions and condition operands", () => {
    const expr = { op: "div", l: { const: 2 }, r: { op: "sub", l: { attr: "p" }, r: { attr: "q" } } };
    const computed = { ...r("computed"), conclusion: "computed", conclude_expr: expr };
    const { links } = ruleDependencies([r("p", { conclude_predicate_id: "p" }), r("q", { conclude_predicate_id: "q" }), computed, r("operand", { conditions: [{ predicate_id: "other", op: "gt", operand: expr }] })], classes);
    expect([...links.get("computed")!.producers]).toEqual(["p", "q"]);
    expect([...links.get("operand")!.producers]).toEqual(["p", "q"]);
  });
  it("matches concluded subclasses to ancestor scopes, not the reverse", () => {
    const { links } = ruleDependencies([r("child-output", { conclusion: "typing", conclude_type_id: "child" }), r("root-output", { conclusion: "typing", conclude_type_id: "root" }), r("child-scope", { subject_type_id: "child" }), r("leaf-scope", { subject_type_id: "leaf" })], classes);
    expect(links.get("child-output")!.consumers.has("child-scope")).toBe(true);
    expect(links.get("child-output")!.consumers.has("root-output")).toBe(true);
    expect(links.get("child-output")!.consumers.has("leaf-scope")).toBe(false);
    expect(links.get("root-output")!.consumers.has("child-scope")).toBe(false);
  });
  it("includes explicit readers of the built-in typing predicate", () => {
    const { links } = ruleDependencies([r("typing", { conclusion: "typing", conclude_type_id: "child" }), r("literal", { subject_type_id: "leaf", conditions: [{ predicate_id: "is-a-id", op: "present" }] })], classes, [{ id: "is-a-id", key: "is_a" }]);
    expect(links.get("typing")!.consumers.has("literal")).toBe(true);
  });
  it("preserves self-dependencies and cycles without treating them as errors", () => {
    const { links } = ruleDependencies([r("a", { conclude_predicate_id: "input" }), r("b", { conclude_predicate_id: "input" })], classes);
    expect([...links.get("a")!.consumers]).toEqual(["a", "b"]);
    expect([...links.get("b")!.consumers]).toEqual(["a", "b"]);
  });
  it("does not reuse another snapshot and reports incomplete definitions", () => {
    const first = ruleDependencies([r("old", { conclude_predicate_id: "input" })], classes);
    expect(first.links.get("old")!.producers.size).toBe(1);
    expect(ruleDependencies([], classes).links.size).toBe(0);
    expect(ruleDependencies([{ ...r("new"), conclusion: "computed", conclude_expr: { op: "future" } }], classes).incomplete).toBe(true);
    expect(ruleDependencies([r("new")], []).incomplete).toBe(true);
  });
  it("handles hierarchy cycles and all parents", () => {
    const tree = [{ id: "x", parents: ["y", "z"] }, { id: "y", parents: ["x"] }, { id: "z", parents: [] }];
    const { links } = ruleDependencies([r("p", { conclusion: "typing", conclude_type_id: "x", subject_type_id: "x" }), r("c", { subject_type_id: "z" })], tree);
    expect(links.get("p")!.consumers.has("c")).toBe(true);
  });
  it("does not truncate a large sparse rule set", () => {
    const rules = Array.from({ length: 10000 }, (_, i) => r(String(i), { conclude_predicate_id: `p${i}`, conditions: [{ predicate_id: `p${i-1}`, op: "present" }] }));
    const start = performance.now();
    const { links } = ruleDependencies(rules, classes);
    expect(links.size).toBe(10000);
    expect([...links.get("9999")!.producers]).toEqual(["9998"]);
    expect([...links.values()].reduce((n, x) => n + x.consumers.size, 0)).toBe(9999);
    expect(performance.now() - start).toBeLessThan(3000);
  });
  it("renders disabled definitions and navigates to the selected ID", () => {
    const onSelect = vi.fn();
    const element = RuleDependencyList({ title: "May receive input from", rules: [r("chosen", { enabled: false })], onSelect });
    const markup = renderToStaticMarkup(element);
    expect(markup).toContain("chosen"); expect(markup).toContain(en.ontology.ruleDisabled);
    type El = ReactElement<{ children?: ReactNode; onClick?: () => void }>;
    const walk = (e: El): void => { if (e.props.onClick) e.props.onClick(); Children.forEach(e.props.children, (c) => { if (isValidElement(c)) walk(c as El); }); };
    walk(element as El); expect(onSelect).toHaveBeenCalledWith("chosen");
    for (const bundle of [en, zh]) expect(bundle.ontology.ruleDependenciesHint.length).toBeGreaterThan(30);
  });
});
