"""Change history for a knowledge system: filter + search + paging, and rollback.

Rollback replays the inverse of every graph-changing event from newest down to the target
(re-add what it removed, remove what it added), restoring the ontology to the state before
that operation. The rollback is itself recorded as a (reversible) event.
"""
from __future__ import annotations

import gzip

from fastapi import APIRouter, Depends, HTTPException, Query
from sqlalchemy import func, or_
from sqlmodel import Session, select

from app import audit
from app.api.conflicts import sync_conflicts
from app.api.knowledge import refresh_ks_stats
from app.db.database import get_session
from app.db.models import AuditEvent, KnowledgeSystem, User
from app.permissions import extraction_active, ks_reader, ks_writer
from app.security import current_user
from app.ontology import schema, store

router = APIRouter(prefix="/api/knowledge", tags=["history"])


def _item(ev: AuditEvent) -> dict:
    """Serialize an event for the client, omitting the (binary) diff and flagging rollbackable."""
    return {
        "id": ev.id,
        "actor_name": ev.actor_name,
        "action": ev.action,
        "summary": ev.summary,
        "detail": ev.detail,
        "created_at": ev.created_at.isoformat(),
        "can_rollback": bool(ev.added or ev.removed),
    }


@router.get("/{ks_id}/history")
def get_history(
    category: str | None = None,
    q: str | None = None,
    limit: int = Query(default=50, le=200),
    offset: int = 0,
    ks: KnowledgeSystem = Depends(ks_reader),
    session: Session = Depends(get_session),
) -> dict:
    conds = [AuditEvent.knowledge_system_id == ks.id]
    if category:
        conds.append(AuditEvent.action.like(f"{category}.%"))
    if q and q.strip():
        like = f"%{q.strip()}%"
        conds.append(or_(AuditEvent.summary.ilike(like), AuditEvent.actor_name.ilike(like)))

    total = session.exec(select(func.count(AuditEvent.id)).where(*conds)).one()
    items = session.exec(
        select(AuditEvent)
        .where(*conds)
        .order_by(AuditEvent.created_at.desc(), AuditEvent.id.desc())
        .limit(limit)
        .offset(offset)
    ).all()
    return {"items": [_item(ev) for ev in items], "total": total}


@router.post("/{ks_id}/history/{event_id}/rollback")
def rollback(
    event_id: int,
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict:
    if extraction_active(session, ks.id):
        raise HTTPException(status_code=409, detail="An extraction is in progress; try again after it finishes.")
    target = session.get(AuditEvent, event_id)
    if not target or target.knowledge_system_id != ks.id:
        raise HTTPException(status_code=404, detail="History event not found")
    if not (target.added or target.removed):
        raise HTTPException(status_code=400, detail="This event did not change the ontology, nothing to roll back")

    # Rollback is a point-in-time revert of a graph to before an event. A cascading action
    # (delete/merge) records a TBox event AND an ABox event sharing a group_id — rolling back
    # either reverts the whole group across BOTH graphs. Otherwise a single graph is reverted.
    # A null graph means the KS's TBox graph (legacy events).
    if target.group_id:
        grp = session.exec(
            select(AuditEvent).where(
                AuditEvent.knowledge_system_id == ks.id, AuditEvent.group_id == target.group_id)
        ).all()
        cutoff = min(e.id for e in grp)
    else:
        cutoff = event_id

    # Undo the cutoff event and everything after it, per graph, newest first.
    events = session.exec(
        select(AuditEvent)
        .where(AuditEvent.knowledge_system_id == ks.id, AuditEvent.id >= cutoff)
        .order_by(AuditEvent.id.desc())
    ).all()
    # A point-in-time revert must replay EVERY graph touched after the cutoff, not just the
    # target's own graph — otherwise a *later* cascading (dual-graph) edit gets only its
    # same-graph half undone, stranding the other graph (e.g. class restored on the TBox but its
    # individuals left untyped in the ABox). Derive the graph set from the events being replayed.
    graphs = {(e.graph or ks.graph_iri) for e in events if (e.added or e.removed)}

    import secrets
    rb_gid = secrets.token_hex(8) if len(graphs) > 1 else None  # keep a multi-graph rollback undoable as one
    undone = 0
    tbox_changed = False
    for g in graphs:
        # Capture each graph's rollback diff so the rollback is itself reversible.
        with store.capture(g) as cap:
            for ev in events:
                if not (ev.added or ev.removed) or (ev.graph or ks.graph_iri) != g:
                    continue
                if ev.added:  # this event added these -> remove them
                    store.remove_triples(g, store.load_triples(gzip.decompress(ev.added)))
                if ev.removed:  # this event removed these -> add them back
                    store.add_triples(g, store.load_triples(gzip.decompress(ev.removed)))
                undone += 1
        added_nt, removed_nt = cap.diff()
        if not (added_nt or removed_nt):
            continue
        if g == ks.graph_iri:
            tbox_changed = True
        audit.record(
            session, ks_id=ks.id, action="system.rollback",
            summary=f"Rolled back to before #{cutoff}" + (" (incl. cascaded instances)" if target.group_id else ""),
            actor_id=user.id, actor_name=user.username,
            detail={"target_event_id": event_id, "cutoff": cutoff},
            added=added_nt, removed=removed_nt, graph=g, group_id=rb_gid,
        )

    # TBox stats/conflicts only matter when the TBox graph itself changed.
    open_conflicts = 0
    if tbox_changed:
        refresh_ks_stats(session, ks)
        open_conflicts = sync_conflicts(session, ks, semantic=False)
    return {
        "undone": undone,
        "view": schema.build_view(ks.graph_iri),
        "open_conflicts": open_conflicts,
    }
