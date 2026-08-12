"""Read the ontology (curated JSON view + Turtle export), provenance, and manual edits."""
from __future__ import annotations

from fastapi import APIRouter, Depends, HTTPException, Response
from pydantic import BaseModel, ConfigDict
from sqlalchemy import delete, func
from sqlmodel import Session, select

from app import audit
from app.api.conflicts import sync_conflicts
from app.api.knowledge import refresh_ks_stats
from app.db.database import get_session
from app.db.models import (
    AboxProvenance,
    AxiomProvenance,
    Chunk,
    Conflict,
    Document,
    EntityResolution,
    ExtractionJob,
    KnowledgeSystem,
    TboxReconciliation,
    TermProposal,
    User,
    ValidationDecision,
)
from app.permissions import extraction_active, ks_reader, ks_writer
from app.security import current_user
from app.ontology import editor, retrieval, schema, skos, statement_provenance, store

router = APIRouter(prefix="/api/knowledge", tags=["ontology"])


def _local(iri: str) -> str:
    return iri.rsplit("#", 1)[-1].rsplit("/", 1)[-1] if iri else ""


def _edit_summary(op: dict) -> str:
    t = op.get("op")
    label = op.get("label") or _local(op.get("iri", "")) or _local(op.get("source", ""))
    return {
        "add_class": f'Added class "{op.get("label", "")}"',
        "update_class": f'Updated class "{label}"',
        "delete_class": f'Deleted class "{label}"',
        "add_property": f'Added property "{op.get("label", "")}"',
        "update_property": f'Updated property "{label}"',
        "delete_property": f'Deleted property "{label}"',
        "add_axiom": f'Added {op.get("type", "")} axiom',
        "delete_axiom": f'Deleted {op.get("type", "")} axiom',
        "merge_classes": f'Merged "{_local(op.get("source", ""))}" into "{_local(op.get("target", ""))}"',
        "set_property_union": f'Set union {op.get("slot", "")} on "{_local(op.get("iri", ""))}"',
    }.get(t, f"Edit: {t}")


@router.get("/{ks_id}/ontology")
def get_ontology(ks: KnowledgeSystem = Depends(ks_reader)) -> dict:
    view = schema.build_view(ks.graph_iri)
    view["knowledge_system"] = {"id": ks.id, "name": ks.name, "base_iri": ks.base_iri}
    return view


@router.get("/{ks_id}/ontology/export")
def export_ontology(fmt: str = "turtle", ks: KnowledgeSystem = Depends(ks_reader)) -> Response:
    """Serialize the ontology graph for download in the requested RDF format."""
    if fmt not in store.EXPORT_FORMATS:
        raise HTTPException(status_code=400, detail=f"Unsupported format: {fmt}")
    _, media_type, _ = store.EXPORT_FORMATS[fmt]
    content = store.serialize_graph(ks.graph_iri, fmt)
    return Response(content=content, media_type=media_type)


class EditRequest(BaseModel):
    """A single ontology edit. `op` selects the operation; extra fields are its params.

    Ops: add_class, update_class, delete_class, add_property, update_property,
    delete_property, add_axiom, delete_axiom, merge_classes.
    """
    model_config = ConfigDict(extra="allow")
    op: str


class ResetOntologyRequest(BaseModel):
    confirm: bool = False


@router.post("/{ks_id}/ontology/reset")
def reset_ontology(
    body: ResetOntologyRequest,
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict:
    """Clear generated semantic state while retaining source documents and configuration."""
    if extraction_active(session, ks.id):
        raise HTTPException(status_code=409, detail="An extraction is in progress; try again after it finishes.")
    if not body.confirm:
        raise HTTPException(status_code=400, detail="confirm=true is required to reset extracted knowledge")

    abox_iri = f"{ks.graph_iri.rstrip('/')}/abox"
    vocabulary_iri = skos.graph_iri_for(ks)
    graph_iris = (ks.graph_iri, abox_iri, vocabulary_iri)
    row_models = (
        AxiomProvenance,
        AboxProvenance,
        EntityResolution,
        Conflict,
        TermProposal,
        TboxReconciliation,
        ValidationDecision,
    )
    removed_rows = {
        model.__tablename__: session.exec(
            select(func.count(model.id)).where(model.knowledge_system_id == ks.id)
        ).one()
        for model in row_models
    }

    with store.capture(graph_iris[0], revert_on_error=True) as tbox_capture, \
            store.capture(graph_iris[1], revert_on_error=True) as abox_capture, \
            store.capture(graph_iris[2], revert_on_error=True) as vocabulary_capture:
        for graph_iri in graph_iris:
            store.clear_graph(graph_iri)

    graph_diffs = (
        ("ontology", graph_iris[0], tbox_capture.diff()),
        ("instances", graph_iris[1], abox_capture.diff()),
        ("vocabulary", graph_iris[2], vocabulary_capture.diff()),
    )
    for model in row_models:
        session.exec(delete(model).where(model.knowledge_system_id == ks.id))
    documents = session.exec(select(Document).where(Document.knowledge_system_id == ks.id)).all()
    for document in documents:
        document.tbox_extracted_at = None
        document.abox_extracted_at = None
        session.add(document)

    retrieval.invalidate(ks.graph_iri)
    refresh_ks_stats(session, ks)

    import secrets
    group_id = secrets.token_hex(8)
    removed_triples: dict[str, int] = {}
    for layer, graph_iri, (_, removed_nt) in graph_diffs:
        removed_triples[layer] = len(store.load_triples(removed_nt)) if removed_nt else 0
        audit.record(
            session,
            ks_id=ks.id,
            action="ontology.reset",
            summary=f"Reset extracted {layer} for clean re-extraction",
            actor_id=user.id,
            actor_name=user.username,
            detail={"layer": layer, "removed_rows": removed_rows},
            removed=removed_nt,
            graph=graph_iri,
            group_id=group_id,
        )
    return {
        "removed_triples": removed_triples,
        "removed_rows": removed_rows,
        "documents_reset": len(documents),
    }


@router.post("/{ks_id}/ontology/edit")
def edit_ontology(
    body: EditRequest,
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict:
    op = body.model_dump()
    if extraction_active(session, ks.id):
        raise HTTPException(status_code=409, detail="An extraction is in progress; try again after it finishes.")
    # Some edits (delete/merge class or property) cascade into the ABox to avoid orphaning instance
    # data; capture that graph too so the cascade is recorded in history and is rollbackable.
    abox_iri = f"{ks.graph_iri.rstrip('/')}/abox"
    try:
        with store.capture(ks.graph_iri, revert_on_error=True) as cap, \
                store.capture(abox_iri, revert_on_error=True) as acap:
            result = editor.apply_edit(ks.graph_iri, ks.base_iri, op)
    except editor.EditError as e:
        raise HTTPException(status_code=400, detail=str(e)) from e
    except Exception as e:  # noqa: BLE001
        raise HTTPException(status_code=400, detail=f"Edit failed: {e}") from e

    added_nt, removed_nt = cap.diff()
    a_added, a_removed = acap.diff()
    refresh_ks_stats(session, ks)
    # Structural checks only on manual edits (fast, no API calls); the semantic duplicate
    # pass runs on extraction and on the explicit "detect conflicts" action.
    open_conflicts = sync_conflicts(session, ks, semantic=False)
    import secrets
    gid = secrets.token_hex(8) if (a_added or a_removed) else None  # link the TBox + ABox events
    tbox_event = audit.record(
        session, ks_id=ks.id, action="ontology.edit", summary=_edit_summary(op),
        actor_id=user.id, actor_name=user.username, detail=op, added=added_nt, removed=removed_nt, group_id=gid,
    )
    statement_provenance.record_tbox_diff(session, ks.id, added_nt, removed_nt, tbox_event)
    if a_added or a_removed:  # the edit also touched instance data — record it as its own ABox event
        audit.record(
            session, ks_id=ks.id, action="ontology.edit", summary=f"{_edit_summary(op)} — cascaded to instances",
            actor_id=user.id, actor_name=user.username, detail=op,
            added=a_added, removed=a_removed, graph=abox_iri, group_id=gid,
        )
    return {
        "result": result,
        "view": schema.build_view(ks.graph_iri),
        "open_conflicts": open_conflicts,
    }


@router.get("/{ks_id}/sources")
def get_sources(ks: KnowledgeSystem = Depends(ks_reader), session: Session = Depends(get_session)) -> list[dict]:
    """Documents that contributed to this knowledge system (derived from provenance),
    with how many chunks and distinct axioms each produced."""
    rows = session.exec(
        select(AxiomProvenance).where(AxiomProvenance.knowledge_system_id == ks.id)
    ).all()
    chunk_ids = {r.chunk_id for r in rows if r.chunk_id is not None}
    doc_by_chunk: dict[int, int] = {}
    if chunk_ids:
        for c in session.exec(select(Chunk).where(Chunk.id.in_(chunk_ids))).all():
            doc_by_chunk[c.id] = c.document_id
    axioms_by_doc: dict[int, set[str]] = {}
    chunks_by_doc: dict[int, set[int]] = {}
    for r in rows:
        doc_id = doc_by_chunk.get(r.chunk_id)
        if doc_id is None:
            continue
        axioms_by_doc.setdefault(doc_id, set()).add(r.axiom_key)
        chunks_by_doc.setdefault(doc_id, set()).add(r.chunk_id)

    out = []
    for doc_id, keys in axioms_by_doc.items():
        d = session.get(Document, doc_id)
        out.append({
            "document_id": doc_id,
            "filename": d.original_filename if d else "(deleted)",
            "folder": d.folder if d else None,
            "exists": d is not None,
            "chunk_count": len(chunks_by_doc.get(doc_id, set())),
            "axiom_count": len(keys),
        })
    out.sort(key=lambda x: -x["axiom_count"])
    return out


@router.get("/{ks_id}/provenance")
def get_provenance(ks: KnowledgeSystem = Depends(ks_reader), session: Session = Depends(get_session)) -> list[dict]:
    """Which chunk/document each axiom came from (grouped by axiom key)."""
    rows = session.exec(
        select(AxiomProvenance).where(AxiomProvenance.knowledge_system_id == ks.id)
    ).all()
    # Enrich with document ids for the chunks.
    chunk_ids = {r.chunk_id for r in rows if r.chunk_id is not None}
    doc_by_chunk: dict[int, int] = {}
    if chunk_ids:
        for c in session.exec(select(Chunk).where(Chunk.id.in_(chunk_ids))).all():
            doc_by_chunk[c.id] = c.document_id
    job_ids = {row.job_id for row in rows if row.job_id is not None}
    jobs = {
        job.id: job for job in session.exec(select(ExtractionJob).where(ExtractionJob.id.in_(job_ids))).all()
    } if job_ids else {}

    grouped: dict[str, dict] = {}
    for r in rows:
        g = grouped.setdefault(r.axiom_key, {"axiom_key": r.axiom_key, "sources": []})
        job = jobs.get(r.job_id)
        g["sources"].append({
            "chunk_id": r.chunk_id,
            "document_id": doc_by_chunk.get(r.chunk_id),
            "job_id": r.job_id,
            "model": job.model if job else None,
            "prompt_snapshot": job.prompt_snapshot if job else None,
            "method": r.method,
            "actor": r.actor_name or None,
            "review": r.review_record or None,
        })
    return list(grouped.values())
