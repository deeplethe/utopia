import { readExpression, type RuleExpression } from "./ruleExpressions";

/** Unfinished text is draft state; it must never become the numeric zero. */
export type ExpressionDraft =
  | { attr: string }
  | { const: string }
  | { op: "add" | "sub" | "mul" | "div"; l: ExpressionDraft; r: ExpressionDraft };

export function draftFromExpression(expr: RuleExpression): ExpressionDraft {
  if ("const" in expr) return { const: String(expr.const) };
  if ("attr" in expr) return { attr: expr.attr };
  return { op: expr.op, l: draftFromExpression(expr.l), r: draftFromExpression(expr.r) };
}

/** Structural preview only. Declaration policy and server write validation are separate. */
export function previewExpression(draft: ExpressionDraft, attributeIds: ReadonlySet<string>): RuleExpression | null {
  const parsed = readExpression(draft);
  if (!parsed) return null;
  const referencesExist = (node: RuleExpression): boolean =>
    "attr" in node ? attributeIds.has(node.attr) : "const" in node || (referencesExist(node.l) && referencesExist(node.r));
  return referencesExist(parsed) ? parsed : null;
}
