"""Agentic conflict triage.

After conflict detection, an agent looks at the *auto-resolvable* TBox conflicts — duplicate
classes and over-specialized predicates — and picks the best available resolution. When it is very
confident it applies the resolution directly (recorded as a rollbackable ``conflict.resolve`` event
by ``conflict-agent``); otherwise it attaches a recommendation to the conflict so a human can
confirm with one click. Structural conflicts (cycles, disjoint contradictions) are never touched by
the agent — those need human judgement.

This is the conflict-queue analog of the resolution agent (which auto-resolves confident mentions
and queues the ambiguous ones) and the reconcile agent (domain/range). Runs synchronously off the
event loop, like reconcile.
"""
from __future__ import annotations

import json
import logging

from sqlmodel import Session, select

from app.config import settings
from app.db.database import engine
from app.db.models import Conflict, KnowledgeSystem, utcnow
from app.llm import openrouter
from app.ontology import editor, retrieval, store

logger = logging.getLogger(__name__)

# Conflict types the agent may triage. Structural conflicts are intentionally excluded.
AUTO_TYPES = {"duplicate", "predicate_specialization"}

_SYSTEM = """You resolve ONE ontology TBox conflict by choosing the best available resolution.

Respond with EXACTLY ONE JSON object per turn — one of:
1) {"action":"get_neighborhood","name":"<a class label>"}
   → returns that class's superclasses/subclasses/related properties, to judge.
2) {"action":"finish","resolution":"<a resolution id, or 'skip'>","confidence":<0..1>,"reason":"<short>"}

Guidance:
- Choose a resolution ONLY if you are confident it is correct. If genuinely unsure, finish with
  resolution "skip".
- Duplicate classes: merge the two only if they are truly the SAME concept (not a subtype); pick the
  direction that KEEPS the more standard/general label as the target.
- Over-specialized predicates (e.g. 拥有井/拥有计量站): prefer merging into the general relation; only
  make them sub-properties if the specializations carry genuinely distinct meaning; if unsure, skip.
- Keep "reason" concise (<= 200 chars)."""


def _decide(session: Session, graph_iri: str, model: str | None, c: Conflict) -> dict | None:
    resolutions = c.payload.get("resolutions", [])
    if not resolutions:
        return None
    ents = ", ".join(e.get("label", "?") for e in c.payload.get("entities", []))
    opts = "\n".join(f'- id="{r["id"]}": {r["label"]}' for r in resolutions)
    user = (
        f'Conflict type: {c.ctype}\n{c.title}: {c.detail}\n'
        f'Entities: {ents}\n\nResolutions:\n{opts}\n\nInspect if needed, then finish.'
    )
    messages = [{"role": "system", "content": _SYSTEM}, {"role": "user", "content": user}]
    for _ in range(settings.conflict_agent_max_steps):
        try:
            reply = openrouter.chat_sync(messages, model=model)
            data = openrouter.extract_json(reply)
        except Exception as e:  # noqa: BLE001  (LLM hiccup → leave the conflict for a human)
            logger.warning("conflict agent error (%s); leaving conflict %s to a human", e, c.id)
            return None
        if not isinstance(data, dict):
            messages.append({"role": "assistant", "content": reply})
            messages.append({"role": "user", "content": "Reply with a single JSON object."})
            continue
        action = data.get("action")
        if action == "finish":
            return {"resolution": str(data.get("resolution", "")).strip(),
                    "confidence": data.get("confidence"), "reason": str(data.get("reason", ""))[:200]}
        if action == "get_neighborhood":
            res = retrieval.get_neighborhood(graph_iri, str(data.get("name", "")))
            messages.append({"role": "assistant", "content": reply})
            messages.append({"role": "user", "content": f"get_neighborhood result:\n{json.dumps(res, ensure_ascii=False)}"})
        else:
            messages.append({"role": "assistant", "content": reply})
            messages.append({"role": "user", "content": "Unknown action. Use get_neighborhood or finish."})
    return None


def _resolve(session: Session, ks: KnowledgeSystem, model: str | None) -> list[str]:
    from app import audit  # local import: audit imports models; avoid a cycle at module load

    conflicts = session.exec(
        select(Conflict).where(
            Conflict.knowledge_system_id == ks.id,
            Conflict.status == "open",
            Conflict.ctype.in_(AUTO_TYPES),
        )
    ).all()
    log: list[str] = []
    for c in conflicts:
        d = _decide(session, ks.graph_iri, model, c)
        if not d:
            continue
        rid = d["resolution"]
        try:
            conf = float(d.get("confidence") or 0.0)
        except (TypeError, ValueError):
            conf = 0.0
        reason = d.get("reason") or ""
        chosen = next((r for r in c.payload.get("resolutions", []) if r["id"] == rid), None)
        if not chosen:  # "skip" or an invalid id → leave for a human, untouched
            continue

        if conf >= settings.conflict_auto_apply_floor:
            abox_iri = f"{ks.graph_iri.rstrip('/')}/abox"
            try:
                # capture the ABox too: a class/property merge retypes/repoints instance data.
                with store.capture(ks.graph_iri, revert_on_error=True) as cap, \
                        store.capture(abox_iri, revert_on_error=True) as acap:
                    editor.apply_edit(ks.graph_iri, ks.base_iri, chosen["op"])
            except Exception as e:  # noqa: BLE001  (stale/failed edit → leave the conflict open)
                logger.warning("conflict agent apply failed for %s: %s", c.id, e)
                continue
            added, removed = cap.diff()
            a_added, a_removed = acap.diff()
            import secrets
            gid = secrets.token_hex(8) if (a_added or a_removed) else None  # link TBox + ABox events
            c.status = "resolved"
            c.resolution = rid
            c.resolved_at = utcnow()
            session.add(c)
            audit.record(
                session, ks_id=ks.id, action="conflict.resolve",
                summary=f'Agent resolved "{c.title}": {chosen["label"]}',
                actor_id=None, actor_name="conflict-agent",
                detail={"conflict_id": c.id, "resolution": rid, "reason": reason, "confidence": conf, "agent": True},
                added=added, removed=removed, group_id=gid,
            )
            if a_added or a_removed:
                audit.record(
                    session, ks_id=ks.id, action="conflict.resolve",
                    summary=f'Agent resolved "{c.title}" — cascaded to instances',
                    actor_id=None, actor_name="conflict-agent",
                    detail={"conflict_id": c.id, "resolution": rid, "agent": True},
                    added=a_added, removed=a_removed, graph=abox_iri, group_id=gid,
                )
            log.append(f'{c.title} → {chosen["label"]} (auto {conf:.2f})')
        else:
            # Not confident enough to apply — attach the recommendation for a human to confirm.
            c.payload = {**c.payload, "recommendation": {"resolution_id": rid, "reason": reason, "confidence": conf}}
            session.add(c)
            log.append(f'{c.title} → recommend "{chosen["label"]}" ({conf:.2f})')
    session.commit()
    return log


def resolve_open_conflicts_bg(ks_id: int, model: str | None = None) -> list[str]:
    """Triage the KS's open auto-resolvable conflicts with its own session (call via
    ``asyncio.to_thread`` so the LLM work never blocks the event loop). Returns a job-log summary."""
    if not settings.agentic_conflict_resolution:
        return []
    with Session(engine) as session:
        ks = session.get(KnowledgeSystem, ks_id)
        if not ks:
            return []
        return _resolve(session, ks, model)
