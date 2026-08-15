"""Agentic TBox domain/range reconciliation with learned memory (``TboxReconciliation``).

After concurrent per-chunk extraction, a property can accrue several rdfs:domain (or rdfs:range)
classes (each chunk picked one from its local context), which means "the value must be ALL of
them at once" — usually an over-narrow mistake. A multi-step agent decides the right single
domain/range (a common superclass, a union, or keeping one), consulting PAST decisions (its own
and humans') so it goes by experience. Applied via the ontology editor; on any error the
property is left as-is for the conflict queue to catch.
"""
from __future__ import annotations

import json
import logging

from pyoxigraph import NamedNode
from sqlmodel import Session, select

from app import prompt_config
from app.config import settings
from app.db.database import engine
from app.db.models import TboxReconciliation
from app.llm import openrouter
from app.ontology import editor, retrieval, schema, store, vocab

logger = logging.getLogger(__name__)

_SYSTEM = """You reconcile an ontology property that, after extraction, ended up with MULTIPLE \
{slot} classes — which means "the value must be ALL of them at once", usually a mistake. Choose \
the best single {slot}:
- COMMON_SUPER: they share a sensible common superclass covering all uses (e.g. Pump +
  PumpingUnit -> Equipment) — prefer this when it exists.
- UNION: the property genuinely applies to alternative, unrelated types — use their union.
- KEEP: one is correct and the others are extraction errors — keep that one.
Weigh PAST DECISIONS for similar properties (given below / via a tool) and follow that experience.

Respond with EXACTLY ONE JSON object per turn:
1) {"action":"get_neighborhood","class":"<class label>"}          -> its super/subclasses
2) {"action":"lookup_experience","property":"<a property label>"}  -> past reconciliation decisions
3) {"action":"finish","choice":"common_super|union|keep","class":"<class label for common_super/keep>","reason":"..."}
Output MUST be a single valid JSON object, no prose."""

prompt_config.register(
    key="tbox.domain_range_reconcile",
    category="governance",
    title="Domain and range reconciliation",
    description="Collapse conflicting property domains or ranges to a coherent schema decision.",
    default=_SYSTEM,
    variables=("slot",),
    order=20,
)


def _ancestors(supers: dict[str, set[str]], iri: str) -> set[str]:
    out, stack = set(), [iri]
    while stack:
        for s in supers.get(stack.pop(), ()):
            if s not in out:
                out.add(s)
                stack.append(s)
    return out


def _past(session: Session, ks_id: int, property_label: str, slot: str | None = None) -> list[dict]:
    conds = [
        TboxReconciliation.knowledge_system_id == ks_id,
        TboxReconciliation.property_label == property_label,
    ]
    if slot:  # a domain decision must not be biased by this property's past range decisions
        conds.append(TboxReconciliation.slot == slot)
    rows = session.exec(
        select(TboxReconciliation).where(*conds).order_by(TboxReconciliation.id.desc())
    ).all()[:5]
    return [{"slot": r.slot, "candidates": r.candidates, "choice": r.choice,
             "chosen": r.chosen_label, "reason": r.reason, "by": r.resolved_by} for r in rows]


def _decide(session, ks_id, graph_iri, model, prop_label, slot, cand_labels, common_super_label) -> dict | None:
    system = prompt_config.render("tbox.domain_range_reconcile", slot=slot)
    exp = _past(session, ks_id, prop_label, slot)
    user = (
        f'Property "{prop_label}" has multiple {slot}s: {", ".join(cand_labels)}.\n'
        f'{("A common superclass exists: " + common_super_label + ".") if common_super_label else "No obvious common superclass."}\n'
        f'Past decisions for this property: {json.dumps(exp, ensure_ascii=False) if exp else "none"}\n'
        "Decide and finish."
    )
    messages = [{"role": "system", "content": system}, {"role": "user", "content": user}]
    for _ in range(settings.resolution_max_steps):
        try:
            reply = openrouter.chat_sync(messages, model=model)
            data = openrouter.extract_json(reply)
        except Exception as e:  # noqa: BLE001
            logger.warning("tbox reconcile agent error (%s); leaving to conflict queue", e)
            return None
        if not isinstance(data, dict):
            messages.append({"role": "assistant", "content": reply})
            messages.append({"role": "user", "content": "Reply with a single JSON object."})
            continue
        action = data.get("action")
        if action == "finish":
            return {"choice": data.get("choice"), "class": str(data.get("class", "")).strip(),
                    "reason": str(data.get("reason", ""))[:200]}
        if action == "get_neighborhood":
            res = retrieval.get_neighborhood(graph_iri, str(data.get("class", "")))
            messages.append({"role": "assistant", "content": reply})
            messages.append({"role": "user", "content": f"get_neighborhood result:\n{json.dumps(res, ensure_ascii=False)}"})
        elif action == "lookup_experience":
            res = _past(session, ks_id, str(data.get("property", "")).strip())
            messages.append({"role": "assistant", "content": reply})
            messages.append({"role": "user", "content": f"lookup_experience result:\n{json.dumps(res, ensure_ascii=False)}"})
        else:
            messages.append({"role": "assistant", "content": reply})
            messages.append({"role": "user", "content": "Unknown action. Use get_neighborhood, lookup_experience, or finish."})
    return None


def _to_op(d: dict, prop_iri: str, slot: str, vals: list[str], common_super: str | None,
           common: set[str], l2i: dict[str, str]):
    choice = d.get("choice")
    if choice == "union":
        return {"op": "set_property_union", "iri": prop_iri, "slot": slot, "members": vals}, None
    if choice == "common_super":
        # Only accept the agent's class if it's a REAL common ancestor of every current value;
        # otherwise fall back to the computed common super. Never overwrite the slot with an
        # unrelated class (that silently corrupts the TBox and spawns spurious validation warnings).
        chosen = l2i.get(d.get("class", "").lower())
        iri = chosen if chosen in common else common_super
        return ({"op": "update_property", "iri": prop_iri, slot: iri}, iri) if iri else (None, None)
    if choice == "keep":
        iri = l2i.get(d.get("class", "").lower())
        return ({"op": "update_property", "iri": prop_iri, slot: iri}, iri) if iri in vals else (None, None)
    return None, None


def reconcile(
    ks_id: int,
    graph_iri: str,
    base_iri: str,
    model: str | None = None,
) -> tuple[list[str], list[TboxReconciliation]]:
    """Reconcile every property with multiple domain/range classes; record each decision.
    Returns human-readable strings plus unpersisted learned-decision rows. The caller
    owns the SQL transaction, graph capture, validation, and commit so RDF edits and
    decisions are atomic. This function opens a read-only session only for experience
    lookup; it never captures or commits independently."""
    if not settings.agentic_tbox_reconcile:
        return [], []
    view = schema.build_view(graph_iri)
    labels = view["labels"]
    lbl = lambda i: labels.get(i, i.rsplit("#", 1)[-1].rsplit("/", 1)[-1])  # noqa: E731
    l2i = {c["label"].strip().lower(): c["iri"] for c in view["classes"]}
    supers = {c["iri"]: set(c["superclasses"]) for c in view["classes"]}
    applied: list[str] = []
    decisions: list[TboxReconciliation] = []

    with Session(engine) as lookup_session:
        for p in view["object_properties"] + view["data_properties"]:
            for slot, pred in (("domain", vocab.RDFS_DOMAIN), ("range", vocab.RDFS_RANGE)):
                vals = sorted({o.value for o in store.object_terms(graph_iri, NamedNode(p["iri"]), pred)
                               if isinstance(o, NamedNode)})
                if len(vals) < 2 or any("XMLSchema#" in v for v in vals):
                    continue
                # Collapse values subsumed by a more-general one (union semantics): {抽油机, 设备}
                # with 抽油机⊑设备 is just 设备 — deterministic, no agent, no conflict.
                reduced = [v for v in vals if not any(w != v and w in _ancestors(supers, v) for w in vals)]
                if not reduced:
                    reduced = vals
                if len(reduced) == 1:
                    try:
                        editor.apply_edit(graph_iri, base_iri, {"op": "update_property", "iri": p["iri"], slot: reduced[0]})
                    except Exception as e:  # noqa: BLE001
                        logger.warning("tbox reconcile subsume failed for %s: %s", p["iri"], e)
                        continue
                    decisions.append(TboxReconciliation(
                        knowledge_system_id=ks_id, slot=slot, property_label=lbl(p["iri"]),
                        property_iri=p["iri"], candidates=[lbl(v) for v in vals],
                        choice="subsume", chosen_label=lbl(reduced[0]),
                        reason="redundant — subsumed by the more general class", resolved_by="agent",
                    ))
                    applied.append(f'{lbl(p["iri"])}.{slot} → {lbl(reduced[0])} (subsumed)')
                    continue
                vals = reduced  # only genuinely unrelated classes remain — decide among these
                common = set.intersection(*[({v} | _ancestors(supers, v)) for v in vals]) - set(vals)
                common_super = max(common, key=lambda s: len(_ancestors(supers, s))) if common else None
                d = _decide(lookup_session, ks_id, graph_iri, model, lbl(p["iri"]), slot,
                            [lbl(v) for v in vals], lbl(common_super) if common_super else None)
                if not d:
                    continue
                op, chosen = _to_op(d, p["iri"], slot, vals, common_super, common, l2i)
                if not op:
                    continue
                try:
                    editor.apply_edit(graph_iri, base_iri, op)
                except Exception as e:  # noqa: BLE001
                    logger.warning("tbox reconcile apply failed for %s: %s", p["iri"], e)
                    continue
                decisions.append(TboxReconciliation(
                    knowledge_system_id=ks_id, slot=slot, property_label=lbl(p["iri"]),
                    property_iri=p["iri"], candidates=[lbl(v) for v in vals],
                    choice=d["choice"], chosen_label=(lbl(chosen) if chosen else "union"),
                    reason=d.get("reason"), resolved_by="agent",
                ))
                applied.append(f'{lbl(p["iri"])}.{slot} → {d["choice"]} ({lbl(chosen) if chosen else "union"})')
    return applied, decisions


def record_manual(ks_id: int, slot: str, property_label: str, property_iri: str | None,
                  candidates: list[str], choice: str, chosen_label: str | None, resolved_by: str,
                  reason: str | None = None) -> None:
    """Record a human's domain/range reconciliation (from resolving a domain_multi/range_multi
    conflict) so the agent can learn from it. Best-effort; never raises to the caller."""
    try:
        with Session(engine) as session:
            session.add(TboxReconciliation(
                knowledge_system_id=ks_id, slot=slot, property_label=property_label,
                property_iri=property_iri, candidates=candidates, choice=choice,
                chosen_label=chosen_label, reason=reason, resolved_by=resolved_by,
            ))
            session.commit()
    except Exception as e:  # noqa: BLE001
        logger.warning("record_manual reconciliation failed: %s", e)
