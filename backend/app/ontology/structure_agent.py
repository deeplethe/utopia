"""Agentic structure repair — attach isolated classes.

After extraction some classes end up unattached: no parent class, no subclasses, and not the
domain/range of any property (the LLM extracted a concept but never abstracted a parent to hang it
under). For each, an agent proposes the best broader parent — an existing class, or a new general
kind — and attaches it via ``subclass_of`` when confident. Runs at discovery (extraction / detect),
like the other agents; leaves the genuinely-rootless ones alone.
"""
from __future__ import annotations

import asyncio
import json
import logging
from concurrent.futures import ThreadPoolExecutor
from contextvars import copy_context

from sqlmodel import Session, select

from app import model_config, prompt_config
from app.config import settings
from app.db.database import engine
from app.db.models import Chunk, Document, KnowledgeSystem
from app.llm import openrouter
from app.ontology import (
    editor,
    retrieval,
    role_evidence,
    schema,
    statement_provenance,
    store,
    workbench,
)
from app.ontology.vocab import norm_label

logger = logging.getLogger(__name__)

_SYSTEM = """An ontology class is UNATTACHED: it has no parent class and no relationships. Use the
provided SOURCE EXCERPTS to suggest the single best BROADER parent class it should be a subclass of.

- Strongly prefer an EXISTING class from the provided list; reply with its exact label and new=false.
- Only propose a NEW general class when its exact reusable label occurs in the source and the source
  explicitly states the is-a relation (new=true).
- If the class genuinely has no source-supported broader kind, reply parent="" (skip).
- The parent must be a strictly MORE GENERAL kind, never a synonym or the class itself.
- Do not use outside knowledge or mere semantic plausibility. Copy the decisive source wording
  exactly into evidence. Named individuals must not be attached as subclasses.

Reply with EXACTLY ONE JSON object: {"parent":"<label or empty>","new":<bool>,
"confidence":<0..1>,"evidence":"<exact source span or empty>","reason":"<=200 chars>"}."""

prompt_config.register(
    key="tbox.structure_repair",
    category="governance",
    title="Isolated-class structure repair",
    description="Suggest a broader parent for classes that have no structural connections.",
    default=_SYSTEM,
    order=10,
)


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


def _verified_source_edge(
    source_text: str,
    child: str,
    parent: str,
    evidence: str,
    model: str | None,
) -> bool:
    if not role_evidence.evidence_is_grounded(source_text, evidence):
        return False
    from app.ontology import extract

    try:
        verified = asyncio.run(extract._verify_tbox_candidates(
            source_text,
            {
                "classes": [
                    {"label": child, "comment": "", "evidence": evidence},
                    {"label": parent, "comment": "", "evidence": evidence},
                ],
                "object_properties": [],
                "data_properties": [],
                "subclass_of": [{"sub": child, "super": parent, "evidence": evidence}],
                "disjoint_with": [],
                "equivalent_class": [],
            },
            model,
        ))
    except Exception as exc:  # noqa: BLE001
        logger.warning("structure evidence verification failed for %s: %s", child, exc)
        return False
    accepted = {
        norm_label(str(row.get("label") or row.get("name") or ""))
        for row in verified.get("classes", [])
        if isinstance(row, dict)
    }
    edge_verified = any(
        isinstance(row, dict)
        and norm_label(str(row.get("sub") or row.get("child") or row.get("subclass") or ""))
        == norm_label(child)
        and norm_label(str(row.get("super") or row.get("parent") or row.get("superclass") or ""))
        == norm_label(parent)
        for row in verified.get("subclass_of", [])
    )
    return {norm_label(child), norm_label(parent)} <= accepted and edge_verified


def _decide(label: str, existing: list[str], source_text: str, model: str | None) -> dict | None:
    if not source_text:
        return None
    user = (
        f'Unattached class: "{label}"\nExisting classes: {existing}\n\n'
        f'SOURCE EXCERPTS:\n"""\n{source_text}\n"""\n\nSuggest its parent.'
    )
    try:
        reply = openrouter.chat_sync(
            [
                {"role": "system", "content": prompt_config.get("tbox.structure_repair")},
                {"role": "user", "content": user},
            ],
            model=model,
        )
        data = openrouter.extract_json(reply)
    except Exception as e:  # noqa: BLE001
        logger.warning("structure agent error (%s) on %s", e, label)
        return None
    if not isinstance(data, dict):
        return None
    result = {
        "parent": str(data.get("parent", "")).strip(),
        "new": bool(data.get("new")),
        "confidence": data.get("confidence"),
        "evidence": str(data.get("evidence", "")).strip(),
        "reason": str(data.get("reason", ""))[:200],
        "verified": False,
    }
    try:
        confidence = float(result["confidence"] or 0.0)
    except (TypeError, ValueError):
        confidence = 0.0
    if result["parent"] and confidence >= settings.conflict_auto_apply_floor:
        result["verified"] = _verified_source_edge(
            source_text, label, result["parent"], result["evidence"], model,
        )
    return result


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
        document_ids = [
            document.id
            for document in session.exec(
                select(Document).where(Document.knowledge_system_id == ks.id)
            ).all()
            if document.id is not None
        ]
        source_chunks = session.exec(
            select(Chunk).where(Chunk.document_id.in_(document_ids)).order_by(Chunk.id)
        ).all() if document_ids else []

        def source_for(label: str) -> str:
            matched = [
                chunk.text for chunk in source_chunks
                if role_evidence.surface_is_grounded(chunk.text, label)
            ][:4]
            return "\n\n".join(text[:8000] for text in matched)

        # Pass 1 — propose a parent for each isolated class (no writes yet).
        proposals: list[tuple[dict, dict]] = []
        workers = min(len(isolated), model_config.llm_concurrency())
        source_inputs = [(c, source_for(c["label"])) for c in isolated]
        with ThreadPoolExecutor(max_workers=max(1, workers)) as pool:
            futures = [
                (
                    c,
                    pool.submit(
                        copy_context().run,
                        _decide,
                        c["label"],
                        [label for label in all_labels if label != c["label"]],
                        source_text,
                        model,
                    ),
                )
                for c, source_text in source_inputs
            ]
            for c, future in futures:
                d = future.result()
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
        graph_changed = False
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
            if not d.get("verified"):
                log.append(f'{c["label"]}: "{parent}" was not verified by source evidence — left')
                continue
            p_iri = idx.class_by_norm.get(norm_label(parent))
            if not p_iri and not d["new"]:
                continue  # agent named a non-existent "existing" class → don't invent it
            created_new = not p_iri
            try:
                abox_iri = workbench.abox_iri_for(ks.graph_iri)
                # Validation reads both graphs. Lock them in the fixed TBox -> ABox
                # order and keep the compensating captures live through SQL commit.
                with store.capture(ks.graph_iri, revert_on_error=True) as cap, \
                        store.capture(abox_iri, revert_on_error=True) as acap:
                    baseline_errors = workbench.structural_error_signatures(ks.graph_iri)
                    if created_new:
                        p_iri = editor.apply_edit(ks.graph_iri, ks.base_iri, {"op": "add_class", "label": parent})
                    editor.apply_edit(ks.graph_iri, ks.base_iri,
                                      {"op": "add_axiom", "type": "subclass", "sub": c["iri"], "super": p_iri})
                    new_errors = workbench.new_structural_errors(ks.graph_iri, baseline_errors)
                    if new_errors:
                        raise RuntimeError(
                            "structure repair introduced structural errors: "
                            + ", ".join(new_errors)
                        )
                    added, removed = cap.diff()
                    a_added, a_removed = acap.diff()
                    if not (added or removed or a_added or a_removed):
                        continue
                    detail = {
                        "class": c["iri"], "parent": parent, "new": created_new,
                        "reason": d["reason"], "evidence": d["evidence"],
                        "confidence": conf, "agent": True,
                    }
                    event = audit.record(
                        session, ks_id=ks.id, action="tbox.attach_isolated",
                        summary=f'Agent attached "{c["label"]}" ⊑ "{parent}"'
                                f'{" (new class)" if created_new else ""}',
                        actor_id=None, actor_name="structure-agent", detail=detail,
                        added=added, removed=removed, commit=False,
                    )
                    statement_provenance.record_tbox_diff(
                        session, ks.id, added, removed, event, commit=False,
                    )
                    session.commit()
            except Exception as e:  # noqa: BLE001
                session.rollback()
                logger.warning("structure agent attach failed for %s: %s", c["label"], e)
                continue
            if created_new:  # keep the in-memory index in sync so a later proposal reuses this parent
                idx.class_by_norm[norm_label(parent)] = p_iri
            graph_changed = True
            log.append(f'{c["label"]} ⊑ {parent}{" (new)" if created_new else ""} (auto {conf:.2f})')
        if graph_changed:
            try:
                retrieval.invalidate(ks.graph_iri)
            except Exception:  # noqa: BLE001
                logger.exception("structure agent cache invalidation failed for %s", ks.graph_iri)
        return log
