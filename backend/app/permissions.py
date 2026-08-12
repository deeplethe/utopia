"""Per-knowledge-system authorization.

Effective role for (user, KS): admin → owner everywhere; the KS owner → owner; an explicit
grant → viewer/editor; otherwise no access. FastAPI dependencies read ``ks_id`` from the
path and return the authorized KnowledgeSystem (or raise 403/404).
"""
from __future__ import annotations

from fastapi import Depends, HTTPException
from sqlmodel import Session, select

from app.db.database import get_session
from app.db.models import ExtractionJob, KnowledgeSystem, KSGrant, User
from app.security import current_user

_RANK = {"viewer": 1, "editor": 2, "owner": 3}


def extraction_active(session: Session, ks_id: int) -> bool:
    """True if an extraction is pending/running for the KS. Graph-changing ops are blocked
    while one runs so each extraction stays an atomic, cleanly-reversible change."""
    return session.exec(
        select(ExtractionJob).where(
            ExtractionJob.knowledge_system_id == ks_id,
            ExtractionJob.status.in_(("pending", "running")),
        )
    ).first() is not None


def effective_role(session: Session, ks: KnowledgeSystem, user: User) -> str | None:
    if user.is_admin or ks.owner_id == user.id:
        return "owner"
    grant = session.exec(
        select(KSGrant).where(KSGrant.knowledge_system_id == ks.id, KSGrant.user_id == user.id)
    ).first()
    return grant.role if grant else None


def accessible_ks_ids(session: Session, user: User) -> set[int] | None:
    """The KS ids a user may see; None means 'all' (admin)."""
    if user.is_admin:
        return None
    owned = {
        k.id for k in session.exec(select(KnowledgeSystem).where(KnowledgeSystem.owner_id == user.id)).all()
    }
    granted = {
        g.knowledge_system_id for g in session.exec(select(KSGrant).where(KSGrant.user_id == user.id)).all()
    }
    return owned | granted


def _require(min_role: str):
    def dep(
        ks_id: int,
        user: User = Depends(current_user),
        session: Session = Depends(get_session),
    ) -> KnowledgeSystem:
        ks = session.get(KnowledgeSystem, ks_id)
        if not ks:
            raise HTTPException(status_code=404, detail="Knowledge system not found")
        role = effective_role(session, ks, user)
        if role is None:
            raise HTTPException(status_code=403, detail="You don't have access to this knowledge system")
        if _RANK[role] < _RANK[min_role]:
            raise HTTPException(status_code=403, detail="Insufficient permissions")
        return ks

    return dep


ks_reader = _require("viewer")   # read access (viewer/editor/owner/admin)
ks_writer = _require("editor")   # content ops (editor/owner/admin)
ks_owner = _require("owner")     # manage/delete (owner/admin)
