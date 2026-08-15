"""Change history for a knowledge system: filter + search + paging, and rollback.

Rollback replays the inverse of every graph-changing event from newest down to the target
(re-add what it removed, remove what it added), restoring the ontology to the state before
that operation. The rollback is itself recorded as a (reversible) event.
"""
from __future__ import annotations

import gzip
import secrets
from contextlib import ExitStack

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
from app.ontology import retrieval, schema, statement_provenance, store, workbench

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
    # Hold every affected graph lock at once so a multi-graph rollback is one RDF critical
    # section. Always include the paired TBox/ABox locks because validation reads both layers even
    # when the selected history range changed only one of them.
    event_graphs = {(e.graph or ks.graph_iri) for e in events if (e.added or e.removed)}
    abox_iri = workbench.abox_iri_for(ks.graph_iri)
    lock_graphs = sorted(event_graphs | {ks.graph_iri, abox_iri})
    captures: dict[str, object] = {}
    changed: dict[str, tuple[bytes, bytes]] = {}
    undone = 0
    open_conflicts: list = []
    tbox_changed = False
    undone_event_ids = {event.id for event in events if event.id is not None}

    try:
        with ExitStack() as stack:
            # Sorting gives every writer the same lock order and avoids cross-graph deadlocks.
            for graph_iri in lock_graphs:
                captures[graph_iri] = stack.enter_context(
                    store.capture(graph_iri, revert_on_error=True)
                )

            baseline_errors = workbench.structural_error_signatures(ks.graph_iri)
            for graph_iri in sorted(event_graphs):
                for event in events:
                    if not (event.added or event.removed):
                        continue
                    if (event.graph or ks.graph_iri) != graph_iri:
                        continue
                    if event.added:  # this event added these -> remove them
                        store.remove_triples(
                            graph_iri, store.load_triples(gzip.decompress(event.added))
                        )
                    if event.removed:  # this event removed these -> add them back
                        store.add_triples(
                            graph_iri, store.load_triples(gzip.decompress(event.removed))
                        )
                    undone += 1

            new_errors = workbench.new_structural_errors(ks.graph_iri, baseline_errors)
            if new_errors:
                raise HTTPException(
                    status_code=422,
                    detail={
                        "code": "ontology_structural_validation_failed",
                        "message": "The rollback would introduce structural ontology errors.",
                        "new_error_count": len(new_errors),
                        "new_error_signatures": new_errors,
                    },
                )

            for graph_iri in sorted(event_graphs):
                added_nt, removed_nt = captures[graph_iri].diff()
                if added_nt or removed_nt:
                    changed[graph_iri] = (added_nt, removed_nt)

            if not changed:
                raise HTTPException(
                    status_code=409,
                    detail={
                        "code": "history_rollback_noop",
                        "message": "The selected rollback no longer changes the ontology.",
                    },
                )

            rb_gid = secrets.token_hex(8) if len(changed) > 1 else None
            summary = (
                f"Rolled back to before #{cutoff}"
                + (" (incl. cascaded instances)" if target.group_id else "")
            )
            detail = {"target_event_id": event_id, "cutoff": cutoff}
            for graph_iri, (added_nt, removed_nt) in changed.items():
                rollback_event = audit.record(
                    session,
                    ks_id=ks.id,
                    action="system.rollback",
                    summary=summary,
                    actor_id=user.id,
                    actor_name=user.username,
                    detail=detail,
                    added=added_nt,
                    removed=removed_nt,
                    graph=graph_iri,
                    group_id=rb_gid,
                    commit=False,
                )
                if graph_iri == ks.graph_iri:
                    tbox_changed = True
                    statement_provenance.record_tbox_diff(
                        session, ks.id, added_nt, removed_nt, rollback_event, commit=False,
                    )
                elif graph_iri == abox_iri:
                    statement_provenance.record_abox_diff(
                        session,
                        ks.id,
                        added_nt,
                        removed_nt,
                        rollback_event,
                        abox_iri=abox_iri,
                        reverse_event_ids=undone_event_ids,
                        commit=False,
                    )

            if tbox_changed:
                refresh_ks_stats(session, ks, commit=False)
                open_conflicts = sync_conflicts(session, ks, semantic=False, commit=False)
            session.commit()
    except HTTPException:
        session.rollback()
        raise
    except Exception:
        session.rollback()
        raise

    if tbox_changed:
        try:
            retrieval.invalidate(ks.graph_iri)
        except Exception:  # noqa: BLE001 - cache invalidation must not split committed state
            pass
    return {
        "undone": undone,
        "view": schema.build_view(ks.graph_iri),
        "open_conflicts": open_conflicts,
    }
