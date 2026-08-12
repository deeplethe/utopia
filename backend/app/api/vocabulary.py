"""Controlled vocabulary CRUD, terminology suggestions, and human review decisions."""
from __future__ import annotations

from fastapi import APIRouter, Depends, HTTPException, Query, Response
from pydantic import BaseModel, Field
from sqlalchemy import func, or_
from sqlmodel import Session, select

from app import audit, model_config, prompt_config
from app.config import settings
from app.db.database import get_session
from app.db.models import Chunk, Document, KnowledgeSystem, TermProposal, User, utcnow
from app.permissions import extraction_active, ks_reader, ks_writer
from app.security import current_user
from app.ontology import schema, skos, store, terminology_agent, terminology_sync

router = APIRouter(prefix="/api/knowledge", tags=["vocabulary"])


class LabelIn(BaseModel):
    value: str = Field(min_length=1, max_length=300)
    language: str = Field(default="zh-CN", max_length=35)


class SchemeIn(BaseModel):
    title: str = Field(min_length=1, max_length=300)
    description: str = Field(default="", max_length=4000)
    default_language: str = Field(default="zh-CN", max_length=35)


class ConceptIn(BaseModel):
    scheme_iri: str
    pref_labels: list[LabelIn] = Field(min_length=1)
    alt_labels: list[LabelIn] = Field(default_factory=list)
    hidden_labels: list[LabelIn] = Field(default_factory=list)
    description: str = Field(default="", max_length=4000)
    notation: str = Field(default="", max_length=300)
    broader: list[str] = Field(default_factory=list)
    related: list[str] = Field(default_factory=list)
    mapped_entity_iri: str | None = None
    status: str = "active"


class SuggestIn(BaseModel):
    scheme_iri: str
    chunk_ids: list[int] = Field(default_factory=list)
    model: str | None = None


class ProposalDecision(BaseModel):
    payload: dict | None = None
    note: str = Field(default="", max_length=1000)


def _bad_request(exc: Exception) -> HTTPException:
    return HTTPException(status_code=400, detail=str(exc))


def _guard(session: Session, ks: KnowledgeSystem) -> None:
    if extraction_active(session, ks.id):
        raise HTTPException(status_code=409, detail="An extraction is in progress; try again after it finishes.")


def _concept_payload(concept: dict) -> dict:
    return {
        "scheme_iri": concept["scheme_iri"],
        "pref_labels": concept["pref_labels"],
        "alt_labels": concept["alt_labels"],
        "hidden_labels": concept["hidden_labels"],
        "description": concept["description"],
        "notation": concept["notation"],
        "broader": concept["broader"],
        "related": concept["related"],
        "mapped_entity_iri": concept["mapped_entity_iri"],
        "status": concept["status"],
        "origin": concept.get("origin", "manual"),
    }


def _proposal_out(row: TermProposal, concept_labels: dict[str, str] | None = None) -> dict:
    concept_labels = concept_labels or {}
    return {
        "id": row.id,
        "action": row.action,
        "term": row.term,
        "target_iri": row.target_iri,
        "target_label": concept_labels.get(row.target_iri or ""),
        "status": row.status,
        "payload": row.payload,
        "confidence": row.confidence,
        "reason": row.reason,
        "evidence": row.evidence,
        "source_chunk_ids": row.source_chunk_ids,
        "extraction_job_id": row.extraction_job_id,
        "proposed_by": row.proposed_by,
        "resolved_by": row.resolved_by,
        "resolution_note": row.resolution_note,
        "created_at": row.created_at.isoformat(),
        "resolved_at": row.resolved_at.isoformat() if row.resolved_at else None,
    }


@router.get("/{ks_id}/vocabulary")
def get_vocabulary(ks: KnowledgeSystem = Depends(ks_reader)) -> dict:
    return skos.build_view(skos.graph_iri_for(ks))


@router.get("/{ks_id}/vocabulary/schemes")
def list_schemes(ks: KnowledgeSystem = Depends(ks_reader)) -> dict:
    view = skos.build_view(skos.graph_iri_for(ks))
    return {"items": view["schemes"], "total": len(view["schemes"]), "stats": view["stats"]}


@router.post("/{ks_id}/vocabulary/schemes")
def create_scheme(
    body: SchemeIn,
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict:
    _guard(session, ks)
    graph_iri = skos.graph_iri_for(ks)
    try:
        with store.capture(graph_iri, revert_on_error=True) as cap:
            item = skos.create_scheme(graph_iri, body.model_dump())
        added, removed = cap.diff()
    except skos.VocabularyValidationError as exc:
        raise _bad_request(exc) from exc
    audit.record(
        session, ks_id=ks.id, action="vocabulary.create_scheme",
        summary=f'Created controlled vocabulary "{item["title"]}"',
        actor_id=user.id, actor_name=user.username,
        detail={"scheme_iri": item["iri"]}, added=added, removed=removed, graph=graph_iri,
    )
    return item


@router.patch("/{ks_id}/vocabulary/schemes")
def update_scheme(
    iri: str,
    body: SchemeIn,
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict:
    _guard(session, ks)
    graph_iri = skos.graph_iri_for(ks)
    try:
        with store.capture(graph_iri, revert_on_error=True) as cap:
            item = skos.update_scheme(graph_iri, iri, body.model_dump())
        added, removed = cap.diff()
    except skos.VocabularyValidationError as exc:
        raise _bad_request(exc) from exc
    audit.record(
        session, ks_id=ks.id, action="vocabulary.update_scheme",
        summary=f'Updated controlled vocabulary "{item["title"]}"',
        actor_id=user.id, actor_name=user.username,
        detail={"scheme_iri": iri}, added=added, removed=removed, graph=graph_iri,
    )
    return item


@router.delete("/{ks_id}/vocabulary/schemes")
def delete_scheme(
    iri: str,
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict:
    _guard(session, ks)
    graph_iri = skos.graph_iri_for(ks)
    existing = skos.get_scheme(graph_iri, iri)
    if not existing:
        raise HTTPException(status_code=404, detail="Vocabulary not found")
    with store.capture(graph_iri, revert_on_error=True) as cap:
        removed_count = skos.delete_scheme(graph_iri, iri)
    added, removed = cap.diff()
    audit.record(
        session, ks_id=ks.id, action="vocabulary.delete_scheme",
        summary=f'Deleted controlled vocabulary "{existing["title"]}"',
        actor_id=user.id, actor_name=user.username,
        detail={"scheme_iri": iri, "removed_triples": removed_count},
        added=added, removed=removed, graph=graph_iri,
    )
    return {"deleted": iri, "removed_triples": removed_count}


@router.get("/{ks_id}/vocabulary/concepts")
def list_concepts(
    scheme_iri: str | None = None,
    q: str | None = None,
    status: str | None = Query(default=None, pattern="^(active|deprecated)$"),
    mapping: str | None = Query(default=None, pattern="^(mapped|standalone)$"),
    origin: str | None = Query(default=None, pattern="^(manual|extraction|agent)$"),
    start_date: str | None = Query(default=None, pattern=r"^\d{4}-\d{2}-\d{2}$"),
    end_date: str | None = Query(default=None, pattern=r"^\d{4}-\d{2}-\d{2}$"),
    limit: int = Query(default=100, ge=1, le=1000),
    offset: int = Query(default=0, ge=0),
    ks: KnowledgeSystem = Depends(ks_reader),
) -> dict:
    return skos.list_concepts(
        skos.graph_iri_for(ks), scheme_iri=scheme_iri, q=q, status=status,
        mapping=mapping, origin=origin, start_date=start_date, end_date=end_date,
        limit=limit, offset=offset,
    )


@router.post("/{ks_id}/vocabulary/concepts")
def create_concept(
    body: ConceptIn,
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict:
    _guard(session, ks)
    graph_iri = skos.graph_iri_for(ks)
    try:
        with store.capture(graph_iri, revert_on_error=True) as cap:
            item = skos.create_concept(graph_iri, body.model_dump())
        added, removed = cap.diff()
    except skos.VocabularyValidationError as exc:
        raise _bad_request(exc) from exc
    audit.record(
        session, ks_id=ks.id, action="vocabulary.create_concept",
        summary=f'Created controlled term "{item["display_label"]}"',
        actor_id=user.id, actor_name=user.username,
        detail={"concept_iri": item["iri"], "mapped_entity_iri": item["mapped_entity_iri"]},
        added=added, removed=removed, graph=graph_iri,
    )
    return item


@router.patch("/{ks_id}/vocabulary/concepts")
def update_concept(
    iri: str,
    body: ConceptIn,
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict:
    _guard(session, ks)
    graph_iri = skos.graph_iri_for(ks)
    try:
        with store.capture(graph_iri, revert_on_error=True) as cap:
            item = skos.update_concept(graph_iri, iri, body.model_dump())
        added, removed = cap.diff()
    except skos.VocabularyValidationError as exc:
        raise _bad_request(exc) from exc
    audit.record(
        session, ks_id=ks.id, action="vocabulary.update_concept",
        summary=f'Updated controlled term "{item["display_label"]}"',
        actor_id=user.id, actor_name=user.username,
        detail={"concept_iri": iri, "mapped_entity_iri": item["mapped_entity_iri"]},
        added=added, removed=removed, graph=graph_iri,
    )
    return item


@router.delete("/{ks_id}/vocabulary/concepts")
def delete_concept(
    iri: str,
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict:
    _guard(session, ks)
    graph_iri = skos.graph_iri_for(ks)
    existing = skos.get_concept(graph_iri, iri)
    if not existing:
        raise HTTPException(status_code=404, detail="Concept not found")
    with store.capture(graph_iri, revert_on_error=True) as cap:
        removed_count = skos.delete_concept(graph_iri, iri)
    added, removed = cap.diff()
    audit.record(
        session, ks_id=ks.id, action="vocabulary.delete_concept",
        summary=f'Deleted controlled term "{existing["display_label"]}"',
        actor_id=user.id, actor_name=user.username,
        detail={"concept_iri": iri, "removed_triples": removed_count},
        added=added, removed=removed, graph=graph_iri,
    )
    return {"deleted": iri, "removed_triples": removed_count}


@router.get("/{ks_id}/vocabulary/resolve")
def resolve_term(
    q: str = Query(min_length=1),
    language: str | None = None,
    limit: int = Query(default=10, ge=1, le=100),
    ks: KnowledgeSystem = Depends(ks_reader),
) -> dict:
    return skos.resolve(skos.graph_iri_for(ks), q, language=language, limit=limit)


@router.get("/{ks_id}/vocabulary/export")
def export_vocabulary(
    fmt: str = "turtle", ks: KnowledgeSystem = Depends(ks_reader),
) -> Response:
    if fmt not in store.EXPORT_FORMATS:
        raise HTTPException(status_code=400, detail=f"Unsupported format: {fmt}")
    _, media_type, _ = store.EXPORT_FORMATS[fmt]
    return Response(
        content=store.serialize_graph(skos.graph_iri_for(ks), fmt),
        media_type=media_type,
    )


@router.post("/{ks_id}/vocabulary/sync")
def sync_vocabulary(
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict:
    """Run the same deterministic ontology-to-SKOS sync used by extraction jobs."""
    _guard(session, ks)
    graph_iri = skos.graph_iri_for(ks)
    try:
        with store.capture(graph_iri, revert_on_error=True) as capture:
            result = terminology_sync.sync_from_ontology(ks)
        added, removed = capture.diff()
    except skos.VocabularyValidationError as exc:
        raise _bad_request(exc) from exc
    if added or removed:
        audit.record(
            session,
            ks_id=ks.id,
            action="terminology.sync",
            summary=(
                "Synchronized controlled terminology from the ontology: "
                f"+{result['terms_added']} terms / {result['terms_mapped']} mappings"
            ),
            actor_id=user.id,
            actor_name=user.username,
            detail=result,
            added=added,
            removed=removed,
            graph=graph_iri,
        )
    return {**result, "view": skos.build_view(graph_iri)}


@router.post("/{ks_id}/vocabulary/suggest")
def suggest_terms(
    body: SuggestIn,
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict:
    selected = select(Chunk, Document).join(Document, Chunk.document_id == Document.id).where(
        Document.knowledge_system_id == ks.id,
        Document.parse_status == "parsed",
    )
    if body.chunk_ids:
        selected = selected.where(Chunk.id.in_(body.chunk_ids))
    rows = session.exec(
        selected.order_by(Chunk.created_at.desc()).limit(settings.terminology_suggestion_max_chunks)
    ).all()
    model = model_config.resolve_extract_model(session, ks, body.model)
    try:
        with model_config.use_ks_connections(session, ks), prompt_config.use_ks_prompts(session, ks.id):
            proposals = terminology_agent.suggest(
                session, ks, body.scheme_iri, list(rows), model=model,
            )
    except skos.VocabularyValidationError as exc:
        raise _bad_request(exc) from exc
    except Exception as exc:  # noqa: BLE001
        raise HTTPException(status_code=502, detail=f"Terminology agent failed: {exc}") from exc
    audit.record(
        session, ks_id=ks.id, action="terminology.suggest",
        summary=f"Terminology agent proposed {len(proposals)} change(s)",
        actor_id=user.id, actor_name=user.username,
        detail={"scheme_iri": body.scheme_iri, "proposals": len(proposals), "model": model},
    )
    labels = {concept["iri"]: concept["display_label"] for concept in skos.build_view(skos.graph_iri_for(ks))["concepts"]}
    return {"items": [_proposal_out(row, labels) for row in proposals], "total": len(proposals)}


@router.get("/{ks_id}/vocabulary/proposals")
def list_proposals(
    status: str = "all",
    q: str | None = None,
    limit: int = Query(default=50, ge=1, le=1000),
    offset: int = Query(default=0, ge=0),
    ks: KnowledgeSystem = Depends(ks_reader),
    session: Session = Depends(get_session),
) -> dict:
    conds = [TermProposal.knowledge_system_id == ks.id]
    if status != "all":
        conds.append(TermProposal.status == status)
    if q and q.strip():
        like = f"%{q.strip()}%"
        conds.append(or_(TermProposal.term.ilike(like), TermProposal.reason.ilike(like)))
    total = session.exec(select(func.count(TermProposal.id)).where(*conds)).one()
    rows = session.exec(
        select(TermProposal).where(*conds)
        .order_by(TermProposal.created_at.desc(), TermProposal.id.desc())
        .limit(limit).offset(offset)
    ).all()
    labels = {concept["iri"]: concept["display_label"] for concept in skos.build_view(skos.graph_iri_for(ks))["concepts"]}
    return {"items": [_proposal_out(row, labels) for row in rows], "total": total}


def _apply_proposal(graph_iri: str, row: TermProposal, payload: dict) -> dict:
    if row.action == "create":
        return skos.create_concept(graph_iri, payload)
    if not row.target_iri:
        raise skos.VocabularyValidationError("Proposal has no target concept")
    existing = skos.get_concept(graph_iri, row.target_iri)
    if not existing:
        raise skos.VocabularyValidationError("Target concept no longer exists")
    merged = _concept_payload(existing)
    if row.action == "add_alias":
        additions = payload.get("add_alt_labels", [])
        merged["alt_labels"] = merged["alt_labels"] + additions
    elif row.action == "update":
        broader = payload.get("broader_iri")
        if broader and broader not in merged["broader"]:
            merged["broader"] = merged["broader"] + [broader]
        if payload.get("mapped_entity_iri"):
            merged["mapped_entity_iri"] = payload["mapped_entity_iri"]
    else:
        raise skos.VocabularyValidationError("Unsupported proposal action")
    return skos.update_concept(graph_iri, row.target_iri, merged)


@router.post("/{ks_id}/vocabulary/proposals/{proposal_id}/accept")
def accept_proposal(
    proposal_id: int,
    body: ProposalDecision,
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict:
    _guard(session, ks)
    row = session.get(TermProposal, proposal_id)
    if not row or row.knowledge_system_id != ks.id:
        raise HTTPException(status_code=404, detail="Terminology proposal not found")
    if row.status != "pending":
        raise HTTPException(status_code=400, detail="This proposal has already been reviewed")
    payload = body.payload if body.payload is not None else row.payload
    graph_iri = skos.graph_iri_for(ks)
    try:
        with store.capture(graph_iri, revert_on_error=True) as cap:
            concept = _apply_proposal(graph_iri, row, payload)
        added, removed = cap.diff()
    except skos.VocabularyValidationError as exc:
        raise _bad_request(exc) from exc
    row.payload = payload
    row.status = "accepted"
    row.resolved_by = user.username
    row.resolution_note = body.note.strip() or None
    row.resolved_at = utcnow()
    session.add(row)
    audit.record(
        session, ks_id=ks.id, action="terminology.accept",
        summary=f'Accepted terminology proposal for "{row.term}"',
        actor_id=user.id, actor_name=user.username,
        detail={"proposal_id": row.id, "action": row.action, "concept_iri": concept["iri"]},
        added=added, removed=removed, graph=graph_iri,
    )
    labels = {item["iri"]: item["display_label"] for item in skos.build_view(graph_iri)["concepts"]}
    return {"proposal": _proposal_out(row, labels), "concept": concept}


@router.post("/{ks_id}/vocabulary/proposals/{proposal_id}/reject")
def reject_proposal(
    proposal_id: int,
    body: ProposalDecision,
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict:
    row = session.get(TermProposal, proposal_id)
    if not row or row.knowledge_system_id != ks.id:
        raise HTTPException(status_code=404, detail="Terminology proposal not found")
    if row.status != "pending":
        raise HTTPException(status_code=400, detail="This proposal has already been reviewed")
    row.status = "rejected"
    row.resolved_by = user.username
    row.resolution_note = body.note.strip() or None
    row.resolved_at = utcnow()
    session.add(row)
    audit.record(
        session, ks_id=ks.id, action="terminology.reject",
        summary=f'Rejected terminology proposal for "{row.term}"',
        actor_id=user.id, actor_name=user.username,
        detail={"proposal_id": row.id, "action": row.action, "note": row.resolution_note},
    )
    return _proposal_out(row)
