"""Conflict queue: detect, list, resolve, dismiss."""
from __future__ import annotations

import asyncio

from fastapi import APIRouter, Depends, HTTPException, Query
from pydantic import BaseModel
from sqlalchemy import func
from sqlmodel import Session, select

from app import audit
from app.api.knowledge import refresh_ks_stats
from app.db.database import get_session
from app.db.models import Conflict, KnowledgeSystem, TboxReconciliation, User, utcnow
from app.permissions import extraction_active, ks_reader, ks_writer
from app.security import current_user
from app.ontology import conflicts as detector
from app.ontology import editor, schema, store

router = APIRouter(prefix="/api/knowledge", tags=["conflicts"])


def sync_conflicts(session: Session, ks: KnowledgeSystem, *, semantic: bool = True) -> list[Conflict]:
    """Re-detect conflicts and reconcile with stored ones (upsert + auto-clear stale).

    - new signature            -> create as open
    - existing open            -> refresh detail/payload
    - existing resolved + still detected -> re-open (it came back)
    - existing dismissed       -> stay dismissed (user judged it a non-issue)
    - open but no longer detected -> auto-resolve (the issue is gone)

    ``semantic=False`` skips the costly embedding+LLM duplicate pass (snappy edits). When
    skipped, existing open 'duplicate' conflicts are left untouched (not auto-cleared),
    since this pass didn't actually re-check them.
    """
    detected = detector.detect(ks.graph_iri, semantic=semantic)
    by_sig = {d.signature: d for d in detected}
    existing = session.exec(
        select(Conflict).where(Conflict.knowledge_system_id == ks.id)
    ).all()
    existing_by_sig = {c.signature: c for c in existing}

    for sig, d in by_sig.items():
        c = existing_by_sig.get(sig)
        payload = {"entities": d.entities, "resolutions": d.resolutions}
        if c is None:
            session.add(Conflict(
                knowledge_system_id=ks.id, signature=sig, ctype=d.ctype,
                severity=d.severity, status="open", title=d.title, detail=d.detail,
                payload=payload,
            ))
        elif c.status == "dismissed":
            continue
        else:  # open or resolved -> ensure open + refresh
            c.status = "open"
            c.resolved_at = None
            c.resolution = None
            c.title, c.detail, c.severity, c.payload = d.title, d.detail, d.severity, payload
            session.add(c)

    for c in existing:
        if c.status == "open" and c.signature not in by_sig:
            if not semantic and c.ctype == "duplicate":
                continue  # duplicates weren't re-checked this pass; don't auto-clear them
            c.status = "resolved"
            c.resolved_at = utcnow()
            c.resolution = "auto-cleared"
            session.add(c)

    session.commit()
    return list(session.exec(
        select(Conflict)
        .where(Conflict.knowledge_system_id == ks.id, Conflict.status == "open")
        .order_by(Conflict.severity.desc(), Conflict.id)
    ).all())


@router.post("/{ks_id}/conflicts/detect", response_model=list[Conflict])
async def detect_conflicts(
    ks: KnowledgeSystem = Depends(ks_writer), session: Session = Depends(get_session)
) -> list[Conflict]:
    from app.ontology import conflict_agent, structure_agent

    from app import model_config
    with model_config.use_ks_connections(session, ks):
        sync_conflicts(session, ks)
        # Triage the freshly detected auto-resolvable conflicts right here — the agent runs at
        # discovery time (extraction does the same), so there's no separate "run the agent" step.
        if not extraction_active(session, ks.id):
            await asyncio.to_thread(conflict_agent.resolve_open_conflicts_bg, ks.id, None)
            await asyncio.to_thread(structure_agent.attach_isolated_bg, ks.id, None)
            session.expire_all()
    return list(session.exec(
        select(Conflict).where(Conflict.knowledge_system_id == ks.id, Conflict.status == "open")
        .order_by(Conflict.severity.desc(), Conflict.id)
    ).all())


@router.get("/{ks_id}/conflicts", response_model=list[Conflict])
def list_conflicts(
    status: str = "open",
    ctype: str | None = None,
    ks: KnowledgeSystem = Depends(ks_reader),
    session: Session = Depends(get_session),
) -> list[Conflict]:
    q = select(Conflict).where(Conflict.knowledge_system_id == ks.id)
    if status != "all":
        q = q.where(Conflict.status == status)
    if ctype:  # e.g. "duplicate" — the class-dedup decision history
        q = q.where(Conflict.ctype == ctype)
    return list(session.exec(q.order_by(Conflict.severity.desc(), Conflict.id)).all())


class ResolveRequest(BaseModel):
    resolution_id: str


@router.post("/{ks_id}/conflicts/{cid}/resolve")
def resolve_conflict(
    cid: int,
    body: ResolveRequest,
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict:
    c = session.get(Conflict, cid)
    if not c or c.knowledge_system_id != ks.id:
        raise HTTPException(status_code=404, detail="Conflict not found")
    if c.status != "open":
        raise HTTPException(status_code=409, detail=f"Conflict already {c.status}")

    resolutions = c.payload.get("resolutions", [])
    chosen = next((r for r in resolutions if r["id"] == body.resolution_id), None)
    if not chosen:
        raise HTTPException(status_code=400, detail="Unknown resolution id")
    if extraction_active(session, ks.id):
        raise HTTPException(status_code=409, detail="An extraction is in progress; try again after it finishes.")

    # A resolution can cascade into the ABox (merge_classes retypes instances, merge_properties
    # repoints assertion predicates) — capture that graph too so the change is audited/rollbackable.
    abox_iri = f"{ks.graph_iri.rstrip('/')}/abox"
    try:
        with store.capture(ks.graph_iri, revert_on_error=True) as cap, \
                store.capture(abox_iri, revert_on_error=True) as acap:
            editor.apply_edit(ks.graph_iri, ks.base_iri, chosen["op"])
    except Exception as e:  # noqa: BLE001
        raise HTTPException(status_code=400, detail=f"Resolution failed: {e}") from e
    added_nt, removed_nt = cap.diff()
    a_added, a_removed = acap.diff()
    import secrets
    gid = secrets.token_hex(8) if (a_added or a_removed) else None  # link the TBox + ABox events

    c.status = "resolved"
    c.resolved_at = utcnow()
    c.resolution = body.resolution_id
    session.add(c)
    session.commit()

    # Record a domain/range reconciliation into the learned memory the TBox agent consults,
    # so a human's decision here teaches future automatic reconciliations.
    if c.ctype in ("domain_multi", "range_multi"):
        from app.ontology import tbox_reconcile

        slot = "domain" if c.ctype == "domain_multi" else "range"
        ents = c.payload.get("entities", [])
        prop = ents[0] if ents else {}
        rid = body.resolution_id
        choice = ("union" if rid == "union"
                  else "common_super" if rid.startswith("super-")
                  else "keep")
        if choice == "union":
            chosen_label = "union"
        else:
            cls_iri = chosen["op"].get(slot)
            chosen_label = schema.build_view(ks.graph_iri)["labels"].get(cls_iri, cls_iri) if cls_iri else None
        tbox_reconcile.record_manual(
            ks.id, slot, prop.get("label", ""), prop.get("iri"),
            [e.get("label") for e in ents[1:]], choice, chosen_label, user.username,
        )

    refresh_ks_stats(session, ks)
    # Structural re-check only (fast, no API calls) — same as manual edits. The costly
    # semantic duplicate pass (embeddings + LLM judge, ~10s) would block the UI on every
    # resolve; it stays gated to extraction and the explicit "detect conflicts" action.
    open_conflicts = sync_conflicts(session, ks, semantic=False)
    audit.record(
        session, ks_id=ks.id, action="conflict.resolve",
        summary=f'Resolved conflict "{c.title}" ({chosen["label"]})',
        actor_id=user.id, actor_name=user.username,
        detail={"conflict_id": cid, "resolution": body.resolution_id, "op": chosen["op"]},
        added=added_nt, removed=removed_nt, group_id=gid,
    )
    if a_added or a_removed:  # the resolution also touched instance data
        audit.record(
            session, ks_id=ks.id, action="conflict.resolve",
            summary=f'Resolved conflict "{c.title}" — cascaded to instances',
            actor_id=user.id, actor_name=user.username,
            detail={"conflict_id": cid, "resolution": body.resolution_id, "op": chosen["op"]},
            added=a_added, removed=a_removed, graph=abox_iri, group_id=gid,
        )
    return {
        "resolved": cid,
        "open_conflicts": open_conflicts,
        "view": schema.build_view(ks.graph_iri),
    }


@router.post("/{ks_id}/conflicts/{cid}/dismiss", response_model=Conflict)
def dismiss_conflict(
    cid: int,
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> Conflict:
    c = session.get(Conflict, cid)
    if not c or c.knowledge_system_id != ks.id:
        raise HTTPException(status_code=404, detail="Conflict not found")
    c.status = "dismissed"
    c.resolved_at = utcnow()
    c.resolution = "dismissed"
    session.add(c)
    session.commit()
    audit.record(
        session, ks_id=ks.id, action="conflict.dismiss",
        summary=f'Dismissed conflict "{c.title}"',
        actor_id=user.id, actor_name=user.username, detail={"conflict_id": cid},
    )
    session.refresh(c)
    return c


@router.post("/{ks_id}/conflicts/{cid}/reopen", response_model=Conflict)
def reopen_conflict(
    cid: int,
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> Conflict:
    """Reconsider a resolved/dismissed conflict — the learned-memory 'revoke' for duplicate-class
    decisions: flip it back to open so the judge re-evaluates the pair. Doesn't undo any graph
    change a prior resolution made (that's a History rollback)."""
    c = session.get(Conflict, cid)
    if not c or c.knowledge_system_id != ks.id:
        raise HTTPException(status_code=404, detail="Conflict not found")
    if c.status == "open":
        raise HTTPException(status_code=400, detail="Conflict is already open")
    c.status = "open"
    c.resolved_at = None
    c.resolution = None
    session.add(c)
    audit.record(
        session, ks_id=ks.id, action="conflict.reopen",
        summary=f'Reopened conflict "{c.title}"',
        actor_id=user.id, actor_name=user.username, detail={"conflict_id": cid},
    )
    session.commit()
    session.refresh(c)
    return c


@router.get("/{ks_id}/reconciliations")
def list_reconciliations(
    q: str | None = None,
    limit: int = Query(default=50, le=1000),
    offset: int = 0,
    ks: KnowledgeSystem = Depends(ks_reader),
    session: Session = Depends(get_session),
) -> dict:
    """The learned TBox domain/range reconciliation memory — decisions the reconcile agent
    consults. One row per property/slot decision (union | common_super | keep), by agent or human."""
    conds = [TboxReconciliation.knowledge_system_id == ks.id]
    if q and q.strip():
        conds.append(TboxReconciliation.property_label.ilike(f"%{q.strip()}%"))
    total = session.exec(select(func.count(TboxReconciliation.id)).where(*conds)).one()
    rows = session.exec(
        select(TboxReconciliation).where(*conds)
        .order_by(TboxReconciliation.id.desc()).limit(limit).offset(offset)
    ).all()
    items = [{
        "id": r.id, "slot": r.slot, "property_label": r.property_label, "property_iri": r.property_iri,
        "candidates": r.candidates, "choice": r.choice, "chosen_label": r.chosen_label,
        "reason": r.reason, "resolved_by": r.resolved_by, "created_at": r.created_at.isoformat(),
    } for r in rows]
    return {"items": items, "total": total}


@router.delete("/{ks_id}/reconciliations/{rid}")
def revoke_reconciliation(
    rid: int,
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict:
    """Forget one domain/range reconciliation decision so the agent re-decides that property next
    time. Edits the agent's MEMORY only — the schema edit it made stays (undo via History)."""
    row = session.get(TboxReconciliation, rid)
    if not row or row.knowledge_system_id != ks.id:
        raise HTTPException(status_code=404, detail="Reconciliation not found")
    label = row.property_label
    session.delete(row)
    audit.record(
        session, ks_id=ks.id, action="reconciliation.revoke",
        summary=f'Forgot reconciliation memory for "{label}"',
        actor_id=user.id, actor_name=user.username, detail={"property": label, "slot": row.slot},
    )
    session.commit()
    return {"revoked": rid}


class ReasonUpdate(BaseModel):
    reason: str = ""


@router.patch("/{ks_id}/reconciliations/{rid}")
def edit_reconciliation_reason(
    rid: int,
    body: ReasonUpdate,
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict:
    """Edit the rationale for a domain/range reconciliation decision. Shown here and fed back to
    the reconcile agent's prompt (via lookup_experience) as experience. Kept short."""
    row = session.get(TboxReconciliation, rid)
    if not row or row.knowledge_system_id != ks.id:
        raise HTTPException(status_code=404, detail="Reconciliation not found")
    row.reason = (body.reason or "").strip()[:200]
    session.add(row)
    audit.record(
        session, ks_id=ks.id, action="reconciliation.edit_reason",
        summary=f'Edited reconciliation reason for "{row.property_label}"',
        actor_id=user.id, actor_name=user.username, detail={"property": row.property_label, "reason": row.reason},
    )
    session.commit()
    return {"id": rid, "reason": row.reason}
