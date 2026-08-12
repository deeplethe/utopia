import type { Conflict } from "./types"
import type { MessageKey, Translate } from "./i18n"

const TYPE_LABELS: Record<string, string> = {
  cycle: "Subclass cycle",
  disjoint_subclass: "Disjoint subclass",
  disjoint_common: "Disjoint common subclass",
  domain_multi: "Multiple domains",
  range_multi: "Multiple ranges",
  equiv_disjoint: "Equivalent and disjoint",
  duplicate: "Possible duplicate",
  predicate_specialization: "Over-specialized relations",
}

const TYPE_KEYS: Record<string, MessageKey> = {
  cycle: "conflict.type.cycle",
  disjoint_subclass: "conflict.type.disjointSubclass",
  disjoint_common: "conflict.type.disjointCommon",
  domain_multi: "conflict.type.domainMulti",
  range_multi: "conflict.type.rangeMulti",
  equiv_disjoint: "conflict.type.equivDisjoint",
  duplicate: "conflict.type.duplicate",
  predicate_specialization: "conflict.type.predicateSpecialization",
}

export function conflictTypeLabel(type: string, t?: Translate) {
  if (t && TYPE_KEYS[type]) return t(TYPE_KEYS[type])
  return TYPE_LABELS[type] ?? type.replaceAll("_", " ")
}

export function conflictSubject(conflict: Conflict) {
  const labels = conflict.payload.entities.map((entity) => entity.label).filter(Boolean)
  if (conflict.ctype === "domain_multi" || conflict.ctype === "range_multi") {
    return labels[0] ?? conflict.title
  }
  if (conflict.ctype === "duplicate" && labels.length >= 2) {
    return `${labels[0]} ↔ ${labels[1]}`
  }
  return labels.length ? labels.join(" · ") : conflict.title
}
