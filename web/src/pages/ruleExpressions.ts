import type { BusinessRule, RelationTypeView } from "../api";

export type RuleExpression =
  | { attr: string }
  | { const: number | string }
  | { op: "add" | "sub" | "mul" | "div"; l: RuleExpression; r: RuleExpression };

// Match the store's root-at-zero validation bound. Unknown nodes stay unknown;
// a reader must never replace them with a constant or simplify their structure.
export function readExpression(raw: unknown, depth = 0): RuleExpression | null {
  if (depth > 4 || !raw || typeof raw !== "object" || Array.isArray(raw)) return null;
  const node = raw as Record<string, unknown>;
  if (Object.keys(node).length === 1 && typeof node.attr === "string" &&
      /^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$/i.test(node.attr)) {
    return { attr: node.attr };
  }
  if (Object.keys(node).length === 1 &&
      (typeof node.const === "number" || typeof node.const === "string") &&
      /^[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?$/.test(String(node.const).trim()) && Number.isFinite(Number(node.const))) {
    return { const: node.const };
  }
  if (Object.keys(node).length !== 3 || !["add", "sub", "mul", "div"].includes(String(node.op))) return null;
  const l = readExpression(node.l, depth + 1), r = readExpression(node.r, depth + 1);
  return l && r ? { op: node.op as "add" | "sub" | "mul" | "div", l, r } : null;
}

export function expressionText(raw: unknown, attributes: Pick<RelationTypeView, "id" | "label" | "key">[], unknownText: string): string {
  const expr = readExpression(raw);
  if (!expr) return unknownText;
  const labels = new Map(attributes.map((a) => [a.id, a]));
  const counts = new Map<string, number>();
  for (const a of attributes) counts.set(a.label, (counts.get(a.label) ?? 0) + 1);
  const render = (node: RuleExpression): string => {
    if ("attr" in node) {
      const attr = labels.get(node.attr);
      return attr ? (counts.get(attr.label)! > 1 ? `${attr.label} (${attr.key})` : attr.label) : node.attr;
    }
    if ("const" in node) return String(node.const);
    return `(${render(node.l)} ${{ add: "+", sub: "−", mul: "×", div: "÷" }[node.op]} ${render(node.r)})`;
  };
  return render(expr);
}

/** The constant form cannot round-trip these definitions. It may edit their
 * name and description through PATCH, leaving every semantic field on the server. */
export function metadataOnly(rule: BusinessRule): boolean {
  if (!["typing", "attribute"].includes(rule.conclusion)) return true;
  if (rule.conclusion === "attribute" && typeof rule.conclude_value !== "string") return true;
  return rule.conditions.some((c) => {
    if (["gt", "gte", "lt", "lte"].includes(c.op)) {
      return !["number", "string"].includes(typeof c.operand);
    }
    if (["in", "not_in"].includes(c.op)) {
      return !Array.isArray(c.operand) || c.operand.some((v) => typeof v !== "string" || v.trim() !== v || !v || v.includes(","));
    }
    if (c.op === "between") return !Array.isArray(c.operand) || c.operand.length !== 2 || c.operand.some((v) => typeof v !== "number");
    return c.op !== "present" || c.operand != null;
  });
}

export function metadataPatch(name: string, description: string) {
  return { name, description };
}
