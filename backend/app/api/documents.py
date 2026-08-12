"""Document upload, parsing (docling + fallbacks) and chunking — scoped to a knowledge system.

Documents are bound 1:N to a knowledge system and inherit its permissions: reading a KS's
documents needs viewer, uploading/parsing/moving/deleting needs editor. The raw bytes are
content-addressed and shared on disk across KS, but each KS keeps its own Document row.
"""
from __future__ import annotations

import os

from fastapi import APIRouter, Depends, File, Form, HTTPException, Query, UploadFile
from pydantic import BaseModel, Field
from sqlalchemy import func, or_
from sqlmodel import Session, select

from app import audit
from app.api.conflicts import sync_conflicts
from app.api.knowledge import refresh_ks_stats
from app.db.database import get_session
from app.db.models import AboxProvenance, AxiomProvenance, Chunk, Document, KnowledgeSystem, User, utcnow
from app.permissions import extraction_active, ks_reader, ks_writer
from app.security import current_user
from app.ontology import provenance, schema, store
from app.parsing import chunker, parser
from app.storage import blobstore

router = APIRouter(prefix="/api/knowledge", tags=["documents"])


def _ext_of(filename: str) -> str:
    return os.path.splitext(filename)[1].lower().lstrip(".")


def _norm_folder(folder: str | None) -> str:
    """Normalize a virtual folder path to a leading-slash, no-trailing-slash form."""
    cleaned = (folder or "/").strip().strip("/")
    return "/" + cleaned if cleaned else "/"


def _doc_in_ks(session: Session, ks: KnowledgeSystem, document_id: int) -> Document:
    doc = session.get(Document, document_id)
    if not doc or doc.knowledge_system_id != ks.id:
        raise HTTPException(status_code=404, detail="Document not found")
    return doc


class ParseResponse(BaseModel):
    document_id: int
    parse_status: str
    parser_backend: str | None
    text_char_count: int | None
    chunk_count: int
    error: str | None = None


class DocumentListResponse(BaseModel):
    items: list[Document]
    total: int
    folders: list[str]


class ParseBatchIn(BaseModel):
    document_ids: list[int] = Field(default_factory=list, max_length=1000)
    folders: list[str] = Field(default_factory=list, max_length=100)
    recursive: bool = True


class ParseBatchResponse(BaseModel):
    items: list[ParseResponse]
    total: int
    parsed: int
    failed: int


@router.post("/{ks_id}/documents/upload", response_model=Document)
async def upload_document(
    file: UploadFile = File(...),
    folder: str = Form("/"),
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> Document:
    filename = file.filename or "upload"
    ext = _ext_of(filename)
    if ext not in parser.SUPPORTED_EXTS:
        raise HTTPException(
            status_code=400,
            detail=f"Unsupported file type '.{ext}'. Supported: {sorted(parser.SUPPORTED_EXTS)}",
        )

    data = await file.read()
    if not data:
        raise HTTPException(status_code=400, detail="Empty file")

    blob = blobstore.store_bytes(data)

    # Per-KS content-addressed dedup: identical bytes already in *this* KS => same Document
    # (just move it to the target folder). The same bytes in another KS are independent.
    existing = session.exec(
        select(Document).where(
            Document.knowledge_system_id == ks.id, Document.sha256 == blob.sha256
        )
    ).first()
    if existing:
        existing.folder = _norm_folder(folder)
        session.add(existing)
        session.commit()
        session.refresh(existing)
        return existing

    doc = Document(
        knowledge_system_id=ks.id,
        sha256=blob.sha256,
        original_filename=filename,
        folder=_norm_folder(folder),
        ext=ext,
        mime=file.content_type,
        size_bytes=blob.size_bytes,
        storage_path=blob.relative_path,
        parse_status="pending",
    )
    session.add(doc)
    session.commit()
    session.refresh(doc)
    audit.record(
        session, ks_id=ks.id, action="document.upload",
        summary=f'Uploaded "{doc.original_filename}"',
        actor_id=user.id, actor_name=user.username, detail={"document_id": doc.id},
    )
    return doc


@router.get("/{ks_id}/documents", response_model=list[Document])
def list_documents(
    ks: KnowledgeSystem = Depends(ks_reader), session: Session = Depends(get_session)
) -> list[Document]:
    return list(
        session.exec(
            select(Document)
            .where(Document.knowledge_system_id == ks.id)
            .order_by(Document.uploaded_at.desc())
        ).all()
    )


@router.get("/{ks_id}/documents/page", response_model=DocumentListResponse)
def list_documents_page(
    folder: str | None = None,
    q: str | None = None,
    status: str | None = None,
    limit: int = Query(default=20, ge=1, le=100),
    offset: int = Query(default=0, ge=0),
    ks: KnowledgeSystem = Depends(ks_reader),
    session: Session = Depends(get_session),
) -> DocumentListResponse:
    conditions = [Document.knowledge_system_id == ks.id]
    if folder is not None:
        conditions.append(Document.folder == _norm_folder(folder))
    if q and q.strip():
        conditions.append(Document.original_filename.ilike(f"%{q.strip()}%"))
    if status:
        conditions.append(Document.parse_status == status)

    total = session.exec(select(func.count(Document.id)).where(*conditions)).one()
    items = list(
        session.exec(
            select(Document)
            .where(*conditions)
            .order_by(Document.uploaded_at.desc(), Document.id.desc())
            .limit(limit)
            .offset(offset)
        ).all()
    )
    folders = list(
        session.exec(
            select(Document.folder)
            .where(Document.knowledge_system_id == ks.id)
            .distinct()
            .order_by(Document.folder)
        ).all()
    )
    return DocumentListResponse(items=items, total=total, folders=folders)


@router.get("/{ks_id}/documents/{document_id}", response_model=Document)
def get_document(
    document_id: int, ks: KnowledgeSystem = Depends(ks_reader), session: Session = Depends(get_session)
) -> Document:
    return _doc_in_ks(session, ks, document_id)


def _parse_document(doc: Document, ks: KnowledgeSystem, user: User, session: Session) -> ParseResponse:
    document_id = doc.id
    path = blobstore.abs_path(doc.storage_path)
    if not path.exists():
        raise HTTPException(status_code=410, detail="Blob missing on disk")

    try:
        result = parser.parse_file(path, doc.ext)
        spans = chunker.chunk_document(result.text, result.structured_document)

        # Replace any existing chunks (idempotent re-parse). Also drop provenance that
        # referenced the old chunks, so re-parsing never leaves dangling provenance.
        old_chunks = session.exec(select(Chunk).where(Chunk.document_id == document_id)).all()
        old_ids = [c.id for c in old_chunks]
        if old_ids:
            for pr in session.exec(select(AxiomProvenance).where(AxiomProvenance.chunk_id.in_(old_ids))).all():
                session.delete(pr)
            for pr in session.exec(select(AboxProvenance).where(AboxProvenance.chunk_id.in_(old_ids))).all():
                session.delete(pr)
        for old in old_chunks:
            session.delete(old)
        for span in spans:
            session.add(
                Chunk(
                    document_id=document_id,
                    idx=span.idx,
                    text=span.text,
                    char_start=span.char_start,
                    char_end=span.char_end,
                    token_estimate=span.token_estimate,
                )
            )

        doc.parse_status = "parsed"
        doc.parser_backend = result.backend
        doc.parse_error = None
        doc.text_char_count = len(result.text)
        doc.chunk_count = len(spans)
        session.add(doc)
        session.commit()
        session.refresh(doc)
        audit.record(
            session, ks_id=ks.id, action="document.parse",
            summary=f'Parsed "{doc.original_filename}" ({doc.chunk_count} chunks)',
            actor_id=user.id, actor_name=user.username, detail={"document_id": doc.id},
        )
    except Exception as e:  # noqa: BLE001
        session.rollback()
        doc = session.get(Document, document_id)
        doc.parse_status = "failed"
        doc.parse_error = str(e)
        session.add(doc)
        session.commit()
        return ParseResponse(
            document_id=document_id,
            parse_status="failed",
            parser_backend=None,
            text_char_count=None,
            chunk_count=0,
            error=str(e),
        )

    return ParseResponse(
        document_id=document_id,
        parse_status=doc.parse_status,
        parser_backend=doc.parser_backend,
        text_char_count=doc.text_char_count,
        chunk_count=doc.chunk_count,
    )


@router.post("/{ks_id}/documents/parse-batch", response_model=ParseBatchResponse)
def parse_documents_batch(
    body: ParseBatchIn,
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> ParseBatchResponse:
    if extraction_active(session, ks.id):
        raise HTTPException(status_code=409, detail="An extraction is in progress; try again after it finishes.")

    selectors = []
    document_ids = sorted({document_id for document_id in body.document_ids if document_id > 0})
    if document_ids:
        selectors.append(Document.id.in_(document_ids))
    for raw_folder in sorted({folder.strip() for folder in body.folders if folder.strip()}):
        folder = _norm_folder(raw_folder)
        if body.recursive:
            prefix = "/" if folder == "/" else f"{folder}/"
            selectors.append(or_(
                Document.folder == folder,
                Document.folder.startswith(prefix, autoescape=True),
            ))
        else:
            selectors.append(Document.folder == folder)
    if not selectors:
        raise HTTPException(status_code=400, detail="Select at least one document or folder")

    documents = list(
        session.exec(
            select(Document)
            .where(Document.knowledge_system_id == ks.id, or_(*selectors))
            .order_by(Document.uploaded_at.desc(), Document.id.desc())
        ).all()
    )
    results: list[ParseResponse] = []
    for doc in documents:
        try:
            results.append(_parse_document(doc, ks, user, session))
        except HTTPException as exc:
            results.append(ParseResponse(
                document_id=doc.id,
                parse_status="failed",
                parser_backend=None,
                text_char_count=None,
                chunk_count=0,
                error=str(exc.detail),
            ))
    parsed = sum(item.parse_status == "parsed" for item in results)
    return ParseBatchResponse(
        items=results,
        total=len(results),
        parsed=parsed,
        failed=len(results) - parsed,
    )


@router.post("/{ks_id}/documents/{document_id}/parse", response_model=ParseResponse)
def parse_document(
    document_id: int,
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> ParseResponse:
    """Parse a document to text and split into chunks (sync -> runs in FastAPI threadpool)."""
    if extraction_active(session, ks.id):
        raise HTTPException(status_code=409, detail="An extraction is in progress; try again after it finishes.")
    return _parse_document(_doc_in_ks(session, ks, document_id), ks, user, session)


@router.get("/{ks_id}/documents/{document_id}/chunks", response_model=list[Chunk])
def list_chunks(
    document_id: int, ks: KnowledgeSystem = Depends(ks_reader), session: Session = Depends(get_session)
) -> list[Chunk]:
    _doc_in_ks(session, ks, document_id)
    return list(
        session.exec(
            select(Chunk).where(Chunk.document_id == document_id).order_by(Chunk.idx)
        ).all()
    )


@router.get("/{ks_id}/documents/{document_id}/contribution")
def document_contribution(
    document_id: int, ks: KnowledgeSystem = Depends(ks_reader), session: Session = Depends(get_session)
) -> dict:
    """What this document put into the ontology: distinct TBox axioms (via AxiomProvenance) and
    distinct ABox individuals (via EntityResolution.source_chunk_id) traced to its chunks."""
    from app.db.models import EntityResolution

    _doc_in_ks(session, ks, document_id)
    chunk_ids = [c.id for c in session.exec(select(Chunk).where(Chunk.document_id == document_id)).all()]
    axioms: set[str] = set()
    individuals: set[str] = set()
    if chunk_ids:
        for pr in session.exec(
            select(AxiomProvenance).where(
                AxiomProvenance.knowledge_system_id == ks.id, AxiomProvenance.chunk_id.in_(chunk_ids)
            )
        ).all():
            axioms.add(pr.axiom_key)
        for er in session.exec(
            select(EntityResolution).where(
                EntityResolution.knowledge_system_id == ks.id,
                EntityResolution.source_chunk_id.in_(chunk_ids),
                EntityResolution.status.in_(("new", "matched")),
            )
        ).all():
            if er.individual_iri:
                individuals.add(er.individual_iri)
    return {"chunk_count": len(chunk_ids), "axiom_count": len(axioms), "individual_count": len(individuals)}


# --------------------------------------------------------------------------- #
# Move / rename (within the KS)
# --------------------------------------------------------------------------- #
class MoveRequest(BaseModel):
    folder: str | None = None
    original_filename: str | None = None


@router.patch("/{ks_id}/documents/{document_id}", response_model=Document)
def move_document(
    document_id: int,
    body: MoveRequest,
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> Document:
    doc = _doc_in_ks(session, ks, document_id)
    if body.folder is not None:
        doc.folder = _norm_folder(body.folder)
    if body.original_filename:
        doc.original_filename = body.original_filename.strip()
    session.add(doc)
    session.commit()
    session.refresh(doc)
    audit.record(
        session, ks_id=ks.id, action="document.move",
        summary=f'Moved "{doc.original_filename}" to {doc.folder}',
        actor_id=user.id, actor_name=user.username, detail={"document_id": doc.id, "folder": doc.folder},
    )
    return doc


# --------------------------------------------------------------------------- #
# Delete — with ontology impact review (scoped to this KS)
# --------------------------------------------------------------------------- #
def _local(iri: str) -> str:
    return iri.rsplit("#", 1)[-1] if "#" in iri else iri.rstrip("/").rsplit("/", 1)[-1]


def _document_impact(session: Session, ks: KnowledgeSystem, doc: Document) -> list[dict]:
    """Axioms in this KS that came *solely* from this document (no other supporting chunk) —
    the candidates for retraction when the document is deleted."""
    chunk_ids = [c.id for c in session.exec(select(Chunk).where(Chunk.document_id == doc.id)).all()]
    if not chunk_ids:
        return []
    keys = {
        p.axiom_key
        for p in session.exec(
            select(AxiomProvenance).where(
                AxiomProvenance.knowledge_system_id == ks.id,
                AxiomProvenance.chunk_id.in_(chunk_ids),
            )
        ).all()
    }
    if not keys:
        return []

    labels = schema.build_view(ks.graph_iri)["labels"]
    local2label = {_local(iri): lbl for iri, lbl in labels.items()}

    def lblf(local: str, m: dict = local2label) -> str:
        return m.get(local, local)

    sole = []
    for key in sorted(keys):
        other = session.exec(
            select(AxiomProvenance).where(
                AxiomProvenance.knowledge_system_id == ks.id,
                AxiomProvenance.axiom_key == key,
                AxiomProvenance.chunk_id.notin_(chunk_ids),
            )
        ).first()
        if other is None:
            sole.append({"axiom_key": key, "description": provenance.describe_axiom(key, lblf)})
    if not sole:
        return []
    return [{"knowledge_system_id": ks.id, "knowledge_system_name": ks.name, "axioms": sole}]


@router.get("/{ks_id}/documents/{document_id}/impact")
def get_impact(
    document_id: int, ks: KnowledgeSystem = Depends(ks_reader), session: Session = Depends(get_session)
) -> dict:
    doc = _doc_in_ks(session, ks, document_id)
    return {"document_id": document_id, "systems": _document_impact(session, ks, doc)}


@router.post("/{ks_id}/documents/{document_id}/delete")
def delete_document(
    document_id: int,
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict:
    doc = _doc_in_ks(session, ks, document_id)
    filename = doc.original_filename
    if extraction_active(session, ks.id):
        raise HTTPException(status_code=409, detail="An extraction is in progress; try again after it finishes.")

    # 1) Retract ALL axioms that came solely from this document — a sole-source axiom has
    #    no other support, so keeping it after deleting its only source would leave an
    #    orphaned, unsourced axiom. Co-supported / manually-added axioms are untouched.
    with store.capture(ks.graph_iri) as cap:
        for sys in _document_impact(session, ks, doc):
            keys = [a["axiom_key"] for a in sys["axioms"]]
            if keys:
                provenance.retract_axioms(ks.graph_iri, ks.base_iri, keys)
                refresh_ks_stats(session, ks)
                sync_conflicts(session, ks, semantic=False)
    added_nt, removed_nt = cap.diff()

    # 2) Remove provenance + chunks for this document.
    chunk_ids = [c.id for c in session.exec(select(Chunk).where(Chunk.document_id == document_id)).all()]
    if chunk_ids:
        for pr in session.exec(select(AxiomProvenance).where(AxiomProvenance.chunk_id.in_(chunk_ids))).all():
            session.delete(pr)
        for pr in session.exec(select(AboxProvenance).where(AboxProvenance.chunk_id.in_(chunk_ids))).all():
            session.delete(pr)
    for c in session.exec(select(Chunk).where(Chunk.document_id == document_id)).all():
        session.delete(c)

    # 3) Remove the physical blob only if no other Document (in any KS) still references the
    #    same bytes — blobs are content-addressed and shared across knowledge systems.
    storage_path, sha256 = doc.storage_path, doc.sha256
    session.delete(doc)
    session.commit()
    others = session.exec(select(Document).where(Document.sha256 == sha256)).first()
    if others is None:
        blobstore.delete(storage_path)
    audit.record(
        session, ks_id=ks.id, action="document.delete",
        summary=f'Deleted document "{filename}"',
        actor_id=user.id, actor_name=user.username, detail={"document_id": document_id},
        added=added_nt, removed=removed_nt,
    )
    return {"deleted": document_id}
