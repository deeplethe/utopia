"""Agentic validation triage.

A datatype violation — a numeric-typed data property carrying a qualitative value (e.g. 井口压力
= "正常") — is a judgement call: either the property is genuinely qualitative (relax its range to
text, keeping the value) or the value is noise (remove it). For each affected property the agent
weighs its value distribution and, when confident, applies the fix; otherwise it leaves both
options for a human. This is the validation-queue analog of the conflict / resolution agents.
"""
from __future__ import annotations

import json
import logging

from sqlmodel import Session, select

from app import prompt_config
from app.config import settings
from app.db.database import engine
from app.db.models import KnowledgeSystem, ValidationDecision
from app.llm import openrouter
from app.ontology import abox, abox_validate, editor, store

logger = logging.getLogger(__name__)


def _prior(session: Session, ks_id: int, prop_iri: str, prop_label: str) -> ValidationDecision | None:
    """The most recent learned fix decision for this property (by iri, else label)."""
    return session.exec(
        select(ValidationDecision).where(
            ValidationDecision.knowledge_system_id == ks_id,
            (ValidationDecision.property_iri == prop_iri) | (ValidationDecision.property_label == prop_label),
        ).order_by(ValidationDecision.id.desc())
    ).first()


def record_decision(session: Session, ks_id: int, prop_iri: str, prop_label: str, xsd: str | None,
                    action: str, reason: str, resolved_by: str) -> None:
    """Remember a datatype-fix decision so the agent self-resolves this property next time."""
    session.add(ValidationDecision(
        knowledge_system_id=ks_id, property_iri=prop_iri, property_label=prop_label,
        xsd_type=xsd, action=action, reason=reason, resolved_by=resolved_by,
    ))

_SYSTEM = """You fix a datatype violation on a data property. The property is declared numeric but
some of its values are not numbers. Decide, from the value distribution, ONE of:
- "relax": the property really holds qualitative values (status words like 正常/略高/偏高); change
  its type to text so those values are valid and kept.
- "remove": the property is genuinely numeric and the non-numeric values are just noise; drop only
  those bad values.
- "skip": genuinely unclear.

Reply with EXACTLY ONE JSON object: {"action":"relax|remove|skip","confidence":<0..1>,"reason":"<=200 chars"}.
Guidance: if MOST values are qualitative → relax; if only a few outliers among real numbers →
remove; if the property name implies a strict measurement but usage is mixed, prefer relax over
losing data; when unsure → skip."""

prompt_config.register(
    key="abox.datatype_validation",
    category="validation",
    title="Datatype validation",
    description="Decide whether qualitative values require a text range or should be removed as noise.",
    default=_SYSTEM,
    order=10,
)


def _decide(st: dict, model: str | None) -> dict | None:
    user = (
        f'Property "{st["label"]}" is declared xsd:{st["xsd"]}.\n'
        f'{len(st["bad"])} of its {st["total"]} values do not parse as {st["xsd"]}.\n'
        f'Sample values: {st["sample_values"]}\n'
        f'Non-numeric values: {st["bad_values"]}\n\nDecide.'
    )
    try:
        reply = openrouter.chat_sync(
            [
                {"role": "system", "content": prompt_config.get("abox.datatype_validation")},
                {"role": "user", "content": user},
            ],
            model=model,
        )
        data = openrouter.extract_json(reply)
    except Exception as e:  # noqa: BLE001  (LLM hiccup → leave it for a human)
        logger.warning("validation agent error (%s) on %s", e, st["label"])
        return None
    if not isinstance(data, dict):
        return None
    return {"action": str(data.get("action", "")).strip(),
            "confidence": data.get("confidence"), "reason": str(data.get("reason", ""))[:200]}


def triage_bg(ks_id: int, model: str | None = None) -> list[str]:
    """Triage the KS's datatype violations with its own session (call via ``asyncio.to_thread``).
    Applies the confident relax/remove decisions; leaves the rest. Returns a job-log summary."""
    if not settings.agentic_validation:
        return []
    from app import audit  # local import avoids a models import cycle at load

    with Session(engine) as session:
        ks = session.get(KnowledgeSystem, ks_id)
        if not ks:
            return []
        abox_iri = ks.graph_iri.rstrip("/") + "/abox"
        stats = abox_validate.datatype_stats(ks.graph_iri, abox_iri)
        log: list[str] = []
        for st in stats:
            # First consult the learned memory: if this property was decided before, self-resolve
            # by that (a human's decision is authoritative) instead of re-judging.
            prior = _prior(session, ks.id, st["prop"], st["label"])
            if prior and prior.action in ("relax", "remove"):
                action, conf, learned = prior.action, 1.0, True
                reason = f'learned from {prior.resolved_by or "prior"}' + (f': {prior.reason}' if prior.reason else "")
            else:
                d = _decide(st, model)
                if not d:
                    continue
                action = d["action"]
                try:
                    conf = float(d.get("confidence") or 0.0)
                except (TypeError, ValueError):
                    conf = 0.0
                reason = d.get("reason") or ""
                learned = False
            if action not in ("relax", "remove"):
                log.append(f'{st["label"]}: agent would "{action}" ({conf:.2f}) — left for a human')
                continue
            if not learned and conf < settings.validation_auto_apply_floor:
                log.append(f'{st["label"]}: agent "{action}" ({conf:.2f}) below auto floor — left for a human')
                continue

            if action == "relax":
                with store.capture(ks.graph_iri) as cap:
                    editor.apply_edit(ks.graph_iri, ks.base_iri,
                                      {"op": "update_property", "iri": st["prop"], "range": "string"})
                added, removed = cap.diff()
                audit.record(
                    session, ks_id=ks.id, action="abox.fix_violation",
                    summary=f'Agent relaxed "{st["label"]}" to text ({len(st["bad"])} qualitative value(s))',
                    actor_id=None, actor_name="validation-agent",
                    detail={"prop": st["prop"], "action": "relax", "reason": reason, "confidence": conf, "agent": True},
                    added=added, removed=removed, graph=ks.graph_iri,
                )
                log.append(f'{st["label"]} → relaxed to text (auto {conf:.2f})')
            else:  # remove the bad values only
                with store.capture(abox_iri) as cap:
                    for b in st["bad"]:
                        abox.remove_data_assertion(abox_iri, b["subject"], st["prop"], b["value"], b.get("datatype"))
                added, removed = cap.diff()
                audit.record(
                    session, ks_id=ks.id, action="abox.fix_violation",
                    summary=f'Agent removed {len(st["bad"])} noisy value(s) of "{st["label"]}"',
                    actor_id=None, actor_name="validation-agent",
                    detail={"prop": st["prop"], "action": "remove", "reason": reason, "confidence": conf, "agent": True},
                    added=added, removed=removed, graph=abox_iri,
                )
                log.append(f'{st["label"]} → removed {len(st["bad"])} value(s) (auto {conf:.2f})')
            if not learned:  # remember a freshly-judged decision for next time
                record_decision(session, ks.id, st["prop"], st["label"], st["xsd"], action, reason, "agent")
        session.commit()
        return log
