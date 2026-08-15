"""Knowledge system CRUD + per-KS membership (owner/editor/viewer)."""
from __future__ import annotations

from datetime import datetime

from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel
from sqlmodel import Session, select

from app import agent_memory, audit
from app.db.database import get_session
from app.db.models import (
    AboxProvenance, AuditEvent, AxiomProvenance, Chunk, Conflict, Document, EntityResolution,
    ExportJob, ExtractionJob, KnowledgeApiToken, KnowledgePromptOverride, KnowledgeSystem, KSGrant,
    McpUserToken,
    OntologyRelease, ReleaseDeployment, ReleaseStatementProvenance, TboxReconciliation, User,
    TermProposal, ValidationDecision, utcnow,
)
from app.permissions import accessible_ks_ids, effective_role, ks_owner, ks_reader, ks_writer
from app.security import current_user
from app.ontology import abox_validate, release_service, retrieval, schema, store

router = APIRouter(prefix="/api/knowledge", tags=["knowledge"])

GRAPH_ROOT = "http://ontopilot.local/ks"


def graph_iri_for(ks_id: int) -> str:
    return f"{GRAPH_ROOT}/{ks_id}"


def base_iri_for(ks_id: int) -> str:
    return f"{GRAPH_ROOT}/{ks_id}/onto#"


class KSOut(BaseModel):
    """KS payload enriched with the requesting user's effective role, so the frontend can
    gate write/manage controls without re-deriving permissions client-side."""

    id: int
    public_id: str
    name: str
    description: str
    owner_id: int | None
    graph_iri: str
    base_iri: str
    created_at: datetime
    updated_at: datetime
    class_count: int
    property_count: int
    axiom_count: int
    llm_model: str | None  # per-KS model override; None -> system/.env default
    llm_provider_id: int | None
    embedding_provider_id: int | None
    embedding_model: str | None
    my_role: str  # owner | editor | viewer


def ks_out(session: Session, ks: KnowledgeSystem, user: User) -> KSOut:
    return KSOut(
        id=ks.id, public_id=ks.public_id, name=ks.name, description=ks.description, owner_id=ks.owner_id,
        graph_iri=ks.graph_iri, base_iri=ks.base_iri,
        created_at=ks.created_at, updated_at=ks.updated_at,
        class_count=ks.class_count, property_count=ks.property_count, axiom_count=ks.axiom_count,
        llm_model=ks.llm_model,
        llm_provider_id=ks.llm_provider_id,
        embedding_provider_id=ks.embedding_provider_id,
        embedding_model=ks.embedding_model,
        my_role=effective_role(session, ks, user) or "viewer",
    )


class CreateKS(BaseModel):
    name: str
    description: str = ""
    llm_model: str | None = None  # per-KS model override (None -> system/.env default)
    llm_provider_id: int | None = None       # 0/None -> use the system default provider
    embedding_provider_id: int | None = None
    embedding_model: str | None = None


class UpdateKS(BaseModel):
    name: str | None = None
    description: str | None = None
    llm_model: str | None = None  # "" clears the override; a model sets it; omit/null = unchanged
    llm_provider_id: int | None = None       # 0 clears to system default; omit/null = unchanged
    embedding_provider_id: int | None = None
    embedding_model: str | None = None


@router.post("", response_model=KSOut)
def create_ks(
    body: CreateKS, user: User = Depends(current_user), session: Session = Depends(get_session)
) -> KSOut:
    if not body.name.strip():
        raise HTTPException(status_code=400, detail="Name is required")
    model = (body.llm_model or "").strip() or None
    ks = KnowledgeSystem(
        name=body.name.strip(), description=body.description or "", owner_id=user.id, llm_model=model,
        llm_provider_id=body.llm_provider_id or None,
        embedding_provider_id=body.embedding_provider_id or None,
        embedding_model=(body.embedding_model or "").strip() or None,
    )
    session.add(ks)
    session.commit()
    session.refresh(ks)
    ks.graph_iri = graph_iri_for(ks.id)
    ks.base_iri = base_iri_for(ks.id)
    session.add(ks)
    session.commit()
    session.refresh(ks)
    return ks_out(session, ks, user)


@router.get("", response_model=list[KSOut])
def list_ks(user: User = Depends(current_user), session: Session = Depends(get_session)) -> list[KSOut]:
    q = select(KnowledgeSystem).order_by(KnowledgeSystem.created_at.desc())
    ids = accessible_ks_ids(session, user)
    if ids is not None:  # not admin -> restrict to owned/granted
        if not ids:
            return []
        q = q.where(KnowledgeSystem.id.in_(ids))
    return [ks_out(session, ks, user) for ks in session.exec(q).all()]


@router.get("/{ks_id}", response_model=KSOut)
def get_ks(
    ks: KnowledgeSystem = Depends(ks_reader),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> KSOut:
    return ks_out(session, ks, user)


@router.get("/{ks_id}/review/counts")
def review_counts(
    ks: KnowledgeSystem = Depends(ks_reader), session: Session = Depends(get_session),
) -> dict:
    """Pending-item counts for the Review sidebar badges: open conflicts, the entity-resolution
    queue, terminology proposals, and current ABox validation violations."""
    from sqlalchemy import func

    conflicts = session.exec(
        select(func.count(Conflict.id)).where(Conflict.knowledge_system_id == ks.id, Conflict.status == "open")
    ).one()
    resolution = session.exec(
        select(func.count(EntityResolution.id)).where(
            EntityResolution.knowledge_system_id == ks.id, EntityResolution.status == "pending")
    ).one()
    terminology = session.exec(
        select(func.count(TermProposal.id)).where(
            TermProposal.knowledge_system_id == ks.id, TermProposal.status == "pending")
    ).one()
    try:
        v = abox_validate.validate(ks.graph_iri, f"{GRAPH_ROOT}/{ks.id}/abox")
        validation = v["counts"]["error"] + v["counts"]["warning"]
    except Exception:  # noqa: BLE001  (never let a badge break the sidebar)
        validation = 0
    return {
        "conflicts": conflicts,
        "resolution": resolution,
        "terminology": terminology,
        "validation": validation,
        "total": conflicts + resolution + terminology + validation,
    }


@router.patch("/{ks_id}", response_model=KSOut)
def update_ks(
    body: UpdateKS,
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> KSOut:
    if body.name is not None:
        ks.name = body.name.strip()
    if body.description is not None:
        ks.description = body.description
    if body.llm_model is not None:
        ks.llm_model = body.llm_model.strip() or None
    if body.llm_provider_id is not None:
        ks.llm_provider_id = body.llm_provider_id or None  # 0 => clear to system default
    if body.embedding_provider_id is not None:
        ks.embedding_provider_id = body.embedding_provider_id or None
    if body.embedding_model is not None:
        ks.embedding_model = body.embedding_model.strip() or None
    ks.updated_at = utcnow()
    session.add(ks)
    session.commit()
    session.refresh(ks)
    audit.record(
        session, ks_id=ks.id, action="ks.update", summary="Updated knowledge system settings",
        actor_id=user.id, actor_name=user.username,
    )
    return ks_out(session, ks, user)


@router.delete("/{ks_id}")
def delete_ks(ks: KnowledgeSystem = Depends(ks_owner), session: Session = Depends(get_session)) -> dict:
    """Fully delete a KS and everything scoped to it. Important: KS ids can be reused, so any
    orphaned per-KS state (ABox graph, resolution memory, conflicts, history, documents) would
    otherwise bleed into a future KS with the same id."""
    from app.storage import blobstore

    ks_id = ks.id
    # All RDF graphs: TBox, ABox, and the SKOS controlled-vocabulary graph.
    if ks.graph_iri:
        store.clear_graph(ks.graph_iri)
        retrieval.invalidate(ks.graph_iri)  # drop cached entity vectors so a reused id can't inherit them
    store.clear_graph(f"{GRAPH_ROOT}/{ks_id}/abox")
    store.clear_graph(f"{GRAPH_ROOT}/{ks_id}/vocabulary")
    deployments = session.exec(
        select(ReleaseDeployment).where(ReleaseDeployment.knowledge_system_id == ks_id)
    ).all()
    with store.use_store(release_service.get_store()):
        from pyoxigraph import NamedNode

        serving_store = store.get_store()
        for deployment in deployments:
            for graph_iri in (
                deployment.tbox_graph_iri,
                deployment.vocabulary_graph_iri,
                deployment.abox_graph_iri,
            ):
                if graph_iri:
                    serving_store.clear_graph(NamedNode(graph_iri))

    # Documents + chunks + content-addressed blobs (blob removed only if unreferenced elsewhere).
    docs = session.exec(select(Document).where(Document.knowledge_system_id == ks_id)).all()
    for doc in docs:
        for ch in session.exec(select(Chunk).where(Chunk.document_id == doc.id)).all():
            session.delete(ch)
    # Delete the doc rows before checking blob references.
    doc_shas = [(d.storage_path, d.sha256) for d in docs]
    for doc in docs:
        session.delete(doc)
    session.flush()
    for storage_path, sha256 in doc_shas:
        if session.exec(select(Document).where(Document.sha256 == sha256)).first() is None:
            blobstore.delete(storage_path)

    # All per-KS SQL rows.
    agent_memory.delete_scoped_conversations(
        session,
        knowledge_system_id=ks_id,
        commit=False,
    )
    for model in (AxiomProvenance, AboxProvenance, ReleaseStatementProvenance, ReleaseDeployment,
                  ExportJob, OntologyRelease, ExtractionJob, KSGrant, KnowledgeApiToken, McpUserToken,
                  KnowledgePromptOverride, EntityResolution, TermProposal, Conflict, AuditEvent,
                  TboxReconciliation, ValidationDecision):
        for row in session.exec(select(model).where(model.knowledge_system_id == ks_id)).all():
            session.delete(row)

    session.delete(ks)
    session.commit()
    import shutil
    from app.config import settings

    for root in (settings.release_dir / ks.public_id, settings.export_dir / ks.public_id):
        if root.exists():
            shutil.rmtree(root)
    return {"deleted": ks_id}


# --------------------------------------------------------------------------- #
# Membership (owner-managed)
# --------------------------------------------------------------------------- #
class MemberOut(BaseModel):
    user_id: int
    username: str
    role: str  # owner | editor | viewer


class AddMember(BaseModel):
    username: str
    role: str = "viewer"


@router.get("/{ks_id}/members", response_model=list[MemberOut])
def list_members(ks: KnowledgeSystem = Depends(ks_reader), session: Session = Depends(get_session)) -> list[MemberOut]:
    out: list[MemberOut] = []
    if ks.owner_id:
        owner = session.get(User, ks.owner_id)
        if owner:
            out.append(MemberOut(user_id=owner.id, username=owner.username, role="owner"))
    for g in session.exec(select(KSGrant).where(KSGrant.knowledge_system_id == ks.id)).all():
        u = session.get(User, g.user_id)
        if u:
            out.append(MemberOut(user_id=u.id, username=u.username, role=g.role))
    return out


@router.get("/{ks_id}/members/candidates")
def grantable_users(
    q: str | None = None,
    ks: KnowledgeSystem = Depends(ks_owner),
    session: Session = Depends(get_session),
) -> list[dict]:
    """Active users the owner can still grant access to (not already a member / the owner),
    for the 'add member' picker. Owner-scoped: exposes only id + username so a non-admin owner
    can search users to grant, without the full admin user list."""
    taken = {g.user_id for g in session.exec(
        select(KSGrant).where(KSGrant.knowledge_system_id == ks.id)).all()}
    if ks.owner_id:
        taken.add(ks.owner_id)
    stmt = select(User).where(User.active == True)  # noqa: E712
    if q and q.strip():
        stmt = stmt.where(User.username.ilike(f"%{q.strip()}%"))
    stmt = stmt.order_by(User.username)
    return [
        {"id": u.id, "username": u.username, "is_admin": u.is_admin}
        for u in session.exec(stmt).all() if u.id not in taken
    ][:50]


@router.get("/{ks_id}/members/{user_id}/detail")
def member_detail(
    user_id: int,
    ks: KnowledgeSystem = Depends(ks_reader),
    requester: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict:
    """A member's cross-KS access (role per KS) + recent activity — limited to the knowledge
    systems the *requester* can see (admins see all), so it never leaks other teams' data."""
    user = session.get(User, user_id)
    if not user:
        raise HTTPException(status_code=404, detail="User not found")
    # The target must actually be a member of THIS knowledge system — otherwise a viewer could
    # probe arbitrary user ids and read back their username / admin flag (user-table enumeration).
    if not effective_role(session, ks, user):
        raise HTTPException(status_code=404, detail="User not found")

    acc = accessible_ks_ids(session, requester)  # None = all (admin)
    all_ks = session.exec(select(KnowledgeSystem)).all()
    ks_names = {k.id: k.name for k in all_ks}
    access = [
        {"ks_id": k.id, "ks_name": k.name, "role": role}
        for k in all_ks
        if (acc is None or k.id in acc) and (role := effective_role(session, k, user))
    ]

    conds = [AuditEvent.actor_id == user_id]
    if acc is not None:
        conds.append(AuditEvent.knowledge_system_id.in_(acc))
    events = session.exec(
        select(AuditEvent).where(*conds).order_by(AuditEvent.created_at.desc()).limit(30)
    ).all()
    activity = [
        {"ks_name": ks_names.get(e.knowledge_system_id, "?"), "action": e.action,
         "summary": e.summary, "created_at": e.created_at.isoformat()}
        for e in events
    ]
    return {
        "user": {"id": user.id, "username": user.username, "is_admin": user.is_admin, "active": user.active},
        "access": access,
        "activity": activity,
    }


@router.post("/{ks_id}/members", response_model=list[MemberOut])
def add_member(
    body: AddMember,
    ks: KnowledgeSystem = Depends(ks_owner),
    actor: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> list[MemberOut]:
    if body.role not in ("viewer", "editor"):
        raise HTTPException(status_code=400, detail="role must be viewer or editor")
    target = session.exec(select(User).where(User.username == body.username.strip())).first()
    if not target:
        raise HTTPException(status_code=404, detail="User not found")
    if target.id == ks.owner_id:
        raise HTTPException(status_code=400, detail="This user is the owner")
    grant = session.exec(
        select(KSGrant).where(KSGrant.knowledge_system_id == ks.id, KSGrant.user_id == target.id)
    ).first()
    if grant:
        grant.role = body.role
    else:
        grant = KSGrant(knowledge_system_id=ks.id, user_id=target.id, role=body.role)
    session.add(grant)
    session.commit()
    audit.record(
        session, ks_id=ks.id, action="member.add",
        summary=f'Granted {body.role} to "{target.username}"',
        actor_id=actor.id, actor_name=actor.username, detail={"user_id": target.id, "role": body.role},
    )
    return list_members(ks=ks, session=session)


@router.delete("/{ks_id}/members/{user_id}")
def remove_member(
    user_id: int,
    ks: KnowledgeSystem = Depends(ks_owner),
    actor: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict:
    grant = session.exec(
        select(KSGrant).where(KSGrant.knowledge_system_id == ks.id, KSGrant.user_id == user_id)
    ).first()
    if grant:
        target = session.get(User, user_id)
        session.delete(grant)
        session.commit()
        audit.record(
            session, ks_id=ks.id, action="member.remove",
            summary=f'Removed member "{target.username if target else user_id}"',
            actor_id=actor.id, actor_name=actor.username, detail={"user_id": user_id},
        )
    return {"removed": user_id}


def refresh_ks_stats(session: Session, ks: KnowledgeSystem, *, commit: bool = True) -> None:
    """Recompute cached class/property/axiom counts from the RDF graph."""
    stats = schema.graph_stats(ks.graph_iri)
    ks.class_count = stats["class_count"]
    ks.property_count = stats["property_count"]
    ks.axiom_count = stats["axiom_count"]
    ks.updated_at = utcnow()
    session.add(ks)
    if commit:
        session.commit()
    else:
        session.flush()
