"""Agentic structure repair — attach isolated classes.

After extraction some classes end up unattached: no parent class, no subclasses, and not the
domain/range of any property (the LLM extracted a concept but never abstracted a parent to hang it
under). For each, an agent proposes the best broader parent — an existing class, or a new general
kind — and attaches it via ``subclass_of`` when confident. Runs at discovery (extraction / detect),
like the other agents; leaves the genuinely-rootless ones alone.
"""
from __future__ import annotations

import json
import logging

from sqlmodel import Session

from app.config import settings
from app.db.database import engine
from app.db.models import KnowledgeSystem
from app.llm import openrouter
from app.ontology import editor, schema, store
from app.ontology.vocab import norm_label

logger = logging.getLogger(__name__)

_SYSTEM = """An ontology class is UNATTACHED — it has no parent class and no relationships. Suggest
the single best BROADER parent class it should be a subclass of.

- Strongly prefer an EXISTING class from the provided list — reply with its exact label and new=false.
- Only propose a NEW general class if none of the existing ones is a sensible parent (new=true).
- If the class genuinely has no sensible broader kind, reply parent="" (skip).
- The parent must be a strictly MORE GENERAL kind, never a synonym or the class itself.

Reply with EXACTLY ONE JSON object: {"parent":"<label or empty>","new":<bool>,"confidence":<0..1>,"reason":"<=200 chars>"}."""


def _isolated(view: dict) -> list[dict]:
    supers = {c["iri"]: set(c["superclasses"]) for c in view["classes"]}
    has_child: set[str] = set()
    for c in view["classes"]:
        has_child |= set(c["superclasses"])
    used: set[str] = set()
    for p in view["object_properties"] + view["data_properties"]:
        used |= set(p.get("domain_members") or [])
        used |= set(p.get("range_members") or [])
    return [c for c in view["classes"]
            if not supers[c["iri"]] and c["iri"] not in has_child and c["iri"] not in used]


def _decide(label: str, existing: list[str], model: str | None) -> dict | None:
    user = f'Unattached class: "{label}"\nExisting classes: {existing}\n\nSuggest its parent.'
    try:
        reply = openrouter.chat_sync([{"role": "system", "content": _SYSTEM}, {"role": "user", "content": user}], model=model)
        data = openrouter.extract_json(reply)
    except Exception as e:  # noqa: BLE001
        logger.warning("structure agent error (%s) on %s", e, label)
        return None
    if not isinstance(data, dict):
        return None
    return {"parent": str(data.get("parent", "")).strip(), "new": bool(data.get("new")),
            "confidence": data.get("confidence"), "reason": str(data.get("reason", ""))[:200]}


def attach_isolated_bg(ks_id: int, model: str | None = None) -> list[str]:
    """Attach the KS's isolated classes under a broader parent (own session; call via
    ``asyncio.to_thread``). Applies the confident suggestions; leaves the rest. Returns a log."""
    if not settings.agentic_isolated_classes:
        return []
    from app import audit

    with Session(engine) as session:
        ks = session.get(KnowledgeSystem, ks_id)
        if not ks:
            return []
        view = schema.build_view(ks.graph_iri)
        isolated = _isolated(view)
        if not isolated:
            return []
        all_labels = sorted(c["label"] for c in view["classes"])
        # Pass 1 — propose a parent for each isolated class (no writes yet).
        proposals: list[tuple[dict, dict]] = []
        for c in isolated:
            d = _decide(c["label"], [x for x in all_labels if x != c["label"]], model)
            if d:
                proposals.append((c, d))
        # A parent proposed for MANY isolated classes is almost certainly an over-general catch-all
        # (a systematic mis-guess — e.g. dozens of "…function" classes all under "process profile"),
        # so don't auto-attach those; leave them for a human to place.
        from collections import Counter

        parent_votes = Counter(norm_label(d["parent"]) for _, d in proposals if d["parent"])
        max_same_parent = settings.structure_max_same_parent
        # Read the TBox index ONCE (a full-graph scan); keep it in sync in-memory as new parent
        # classes get created below, instead of rescanning the whole graph on every proposal.
        idx = schema.read_index(ks.graph_iri)

        log: list[str] = []
        # Pass 2 — apply the confident, non-suspicious suggestions.
        for c, d in proposals:
            parent = d["parent"]
            try:
                conf = float(d.get("confidence") or 0.0)
            except (TypeError, ValueError):
                conf = 0.0
            if not parent or conf < settings.conflict_auto_apply_floor or norm_label(parent) == norm_label(c["label"]):
                log.append(f'{c["label"]}: agent suggested "{parent or "skip"}" ({conf:.2f}) — left')
                continue
            if parent_votes[norm_label(parent)] > max_same_parent:  # over-general dumping ground → leave for a human
                log.append(f'{c["label"]}: "{parent}" proposed for {parent_votes[norm_label(parent)]} classes — likely over-generalization, left')
                continue
            p_iri = idx.class_by_norm.get(norm_label(parent))
            if not p_iri and not d["new"]:
                continue  # agent named a non-existent "existing" class → don't invent it
            created_new = not p_iri
            try:
                with store.capture(ks.graph_iri, revert_on_error=True) as cap:
                    if created_new:
                        p_iri = editor.apply_edit(ks.graph_iri, ks.base_iri, {"op": "add_class", "label": parent})
                    editor.apply_edit(ks.graph_iri, ks.base_iri,
                                      {"op": "add_axiom", "type": "subclass", "sub": c["iri"], "super": p_iri})
            except Exception as e:  # noqa: BLE001
                logger.warning("structure agent attach failed for %s: %s", c["label"], e)
                continue
            if created_new:  # keep the in-memory index in sync so a later proposal reuses this parent
                idx.class_by_norm[norm_label(parent)] = p_iri
            added, removed = cap.diff()
            audit.record(
                session, ks_id=ks.id, action="tbox.attach_isolated",
                summary=f'Agent attached "{c["label"]}" ⊑ "{parent}"{" (new class)" if d["new"] else ""}',
                actor_id=None, actor_name="structure-agent",
                detail={"class": c["iri"], "parent": parent, "new": d["new"], "reason": d["reason"], "confidence": conf, "agent": True},
                added=added, removed=removed,
            )
            log.append(f'{c["label"]} ⊑ {parent}{" (new)" if d["new"] else ""} (auto {conf:.2f})')
        session.commit()
        return log
