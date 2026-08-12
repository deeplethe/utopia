"""Change-log helper: record an AuditEvent for a knowledge system.

Call ``record(...)`` at each mutation site (after the change is committed). Graph-changing
events pass their N-Triples diff (``added`` / ``removed``); it is gzipped and stored so the
event can be reversed for rollback. Kept separate from the API routers to avoid cycles.
"""
from __future__ import annotations

import gzip

from sqlmodel import Session

from app.db.models import AuditEvent


def record(
    session: Session,
    *,
    ks_id: int,
    action: str,
    summary: str,
    actor_id: int | None = None,
    actor_name: str = "system",
    detail: dict | None = None,
    added: bytes | None = None,
    removed: bytes | None = None,
    graph: str | None = None,
    group_id: str | None = None,
) -> AuditEvent:
    event = AuditEvent(
            knowledge_system_id=ks_id,
            actor_id=actor_id,
            actor_name=actor_name or "system",
            action=action,
            summary=summary,
            detail=detail or {},
            graph=graph,
            group_id=group_id,
            added=gzip.compress(added) if added else None,
            removed=gzip.compress(removed) if removed else None,
        )
    session.add(event)
    session.commit()
    session.refresh(event)
    return event
