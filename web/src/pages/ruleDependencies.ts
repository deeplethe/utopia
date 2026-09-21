import type { BusinessRule, EntityTypeView, RelationTypeView } from "../api";

type Definition = Omit<BusinessRule, "conclusion"> & { conclusion: string; conclude_expr?: unknown };
export interface RuleLinks { producers: Set<string>; consumers: Set<string> }

/** Definition-level candidates only. Conditions, time and actual readings decide
 * which links are used in a proof. Disabled rules remain part of the definition. */
export function ruleDependencies(rules: Definition[], classes: Pick<EntityTypeView, "id" | "parents">[], attributes: Pick<RelationTypeView, "id" | "key">[] = []) {
  const links = new Map<string, RuleLinks>(rules.map((r) => [r.id, { producers: new Set(), consumers: new Set() }]));
  const readers = new Map<string, Set<string>>();
  const scopes = new Map<string, Set<string>>();
  const parents = new Map(classes.map((c) => [c.id, c.parents]));
  let incomplete = false;
  const isA = attributes.find((a) => a.key === "is_a")?.id;
  const index = (map: Map<string, Set<string>>, key: string, id: string) => {
    if (!map.has(key)) map.set(key, new Set());
    map.get(key)!.add(id);
  };
  const readExpr = (raw: unknown, id: string, depth = 0): void => {
    if (depth > 4 || !raw || typeof raw !== "object" || Array.isArray(raw)) { incomplete = true; return; }
    const node = raw as Record<string, unknown>;
    if (typeof node.attr === "string") { index(readers, node.attr, id); return; }
    if (typeof node.const === "number" || typeof node.const === "string") return;
    if (["add", "sub", "mul", "div"].includes(String(node.op))) {
      readExpr(node.l, id, depth + 1); readExpr(node.r, id, depth + 1);
    } else incomplete = true;
  };
  for (const r of rules) {
    index(scopes, r.subject_type_id, r.id);
    if (!parents.has(r.subject_type_id)) incomplete = true;
    for (const c of r.conditions) {
      index(readers, c.predicate_id, r.id);
      if (c.operand && typeof c.operand === "object" && !Array.isArray(c.operand)) readExpr(c.operand, r.id);
    }
    if (r.conclusion === "computed") readExpr(r.conclude_expr, r.id);
    else if (!["typing", "attribute"].includes(r.conclusion)) incomplete = true;
  }
  const connect = (producer: string, consumers?: Set<string>) => {
    for (const consumer of consumers ?? []) {
      links.get(producer)!.consumers.add(consumer);
      links.get(consumer)!.producers.add(producer);
    }
  };
  const ancestorCache = new Map<string, Set<string>>();
  const ancestors = (type: string) => {
    const cached = ancestorCache.get(type);
    if (cached) return cached;
    const seen = new Set<string>(); const pending = [type];
    while (pending.length) {
      const id = pending.pop()!;
      if (seen.has(id)) continue;
      seen.add(id);
      const ps = parents.get(id);
      if (!ps) incomplete = true;
      else pending.push(...ps);
    }
    ancestorCache.set(type, seen);
    return seen;
  };
  for (const r of rules) {
    if (r.conclusion === "typing" && r.conclude_type_id) {
      // reasoning::attribute_rules scopes a rule to its class and descendants;
      // therefore a concluded child can supply membership in an ancestor scope.
      for (const type of ancestors(r.conclude_type_id)) connect(r.id, scopes.get(type));
      // Typings also enter the literal fact pool on the built-in is_a predicate.
      if (isA) connect(r.id, readers.get(isA));
    } else if (["attribute", "computed"].includes(r.conclusion) && r.conclude_predicate_id) {
      connect(r.id, readers.get(r.conclude_predicate_id));
      // A literal is_a conclusion can also affect scope, but the ontology view
      // does not expose every IRI the runner resolves. Do not claim completeness.
      if (r.conclude_predicate_id === isA) incomplete = true;
    }
  }
  return { links, incomplete };
}
