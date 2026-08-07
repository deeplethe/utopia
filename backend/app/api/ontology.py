"""Read the ontology (curated JSON view + Turtle export), provenance, and manual edits."""
from __future__ import annotations

from fastapi import APIRouter, Depends, HTTPException, Response
from pydantic import BaseModel, ConfigDict
from sqlmodel import Session, select

from app import audit
from app.api.conflicts import sync_conflicts
from app.api.knowledge import refresh_ks_stats
from app.db.database import get_session
from app.db.models import AxiomProvenance, Chunk, Document, KnowledgeSystem, User
from app.permissions import extraction_active, ks_reader, ks_writer
from app.security import current_user
from app.ontology import editor, schema, store

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
    audit.record(
        session, ks_id=ks.id, action="ontology.edit", summary=_edit_summary(op),
        actor_id=user.id, actor_name=user.username, detail=op, added=added_nt, removed=removed_nt, group_id=gid,
    )
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

    grouped: dict[str, dict] = {}
    for r in rows:
        g = grouped.setdefault(r.axiom_key, {"axiom_key": r.axiom_key, "sources": []})
        g["sources"].append(
            {"chunk_id": r.chunk_id, "document_id": doc_by_chunk.get(r.chunk_id), "job_id": r.job_id}
        )
    return list(grouped.values())
