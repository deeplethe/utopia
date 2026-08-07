"""Run LLM TBox extraction as a background job; clients poll job progress."""
from __future__ import annotations

import asyncio
import logging

from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel
from sqlmodel import Session, select

from app.api.conflicts import sync_conflicts
from app.api.knowledge import refresh_ks_stats
from app.config import settings
from app.db.database import engine, get_session
from app.db.models import AxiomProvenance, Chunk, Document, ExtractionJob, KnowledgeSystem, User, utcnow
from app.api.abox import abox_iri_for
from app.permissions import extraction_active, ks_reader, ks_writer
from app.security import current_user
from app import audit
from app.ontology import (
    abox_extract, conflict_agent, extract, store, structure_agent, tbox_reconcile, validation_agent,
)

logger = logging.getLogger(__name__)
router = APIRouter(prefix="/api/knowledge", tags=["extraction"])

# Keep strong references to in-flight background tasks so they aren't GC'd.
_tasks: set[asyncio.Task] = set()


def _spawn(coro) -> None:
    task = asyncio.create_task(coro)
    _tasks.add(task)
    task.add_done_callback(_tasks.discard)


def _ok_chunk_ids(result: dict) -> list[int]:
    """Chunk ids processed without error. Only these mark their document extracted — so a document
    whose chunks all failed (e.g. LLM rate-limited) isn't silently shown as done and skipped on a
    re-run. A chunk that succeeded with 0 axioms still counts (it WAS processed)."""
    return [e["chunk_id"] for e in result.get("per_chunk", []) if e.get("status") == "ok"]


def _mark_docs_extracted(session: Session, chunk_ids: list[int], kind: str) -> None:
    """Stamp the documents behind these chunks as TBox- or ABox-extracted (on job success)."""
    if not chunk_ids:
        return
    doc_ids = {c.document_id for c in session.exec(select(Chunk).where(Chunk.id.in_(chunk_ids))).all()}
    now = utcnow()
    for d in session.exec(select(Document).where(Document.id.in_(doc_ids))).all():
        if kind == "abox":
            d.abox_extracted_at = now
        else:
            d.tbox_extracted_at = now
        session.add(d)
    session.commit()


def _sync_conflicts_bg(ks_id: int) -> None:
    """Run conflict detection with its own session (used via asyncio.to_thread so the
    embedding model's CPU work never blocks the event loop)."""
    with Session(engine) as session:
        ks = session.get(KnowledgeSystem, ks_id)
        if ks:
            sync_conflicts(session, ks)


class ExtractRequest(BaseModel):
    chunk_ids: list[int]
    model: str | None = None


async def _run_extraction_job(
    job_id: int, ks_id: int, chunks: list[tuple[int, str]], model: str,
    actor_id: int | None = None, actor_name: str = "system",
) -> None:
    """Background worker: extract into the graph, updating progress in the job row."""
    with Session(engine) as session:
        job = session.get(ExtractionJob, job_id)
        ks = session.get(KnowledgeSystem, ks_id)
        if not job or not ks:
            return
        from app import model_config
        model_config.set_ks_connections(session, ks)  # route LLM/embedding to the KS's providers
        job.status = "running"
        session.add(job)
        session.commit()

        async def progress(ev: dict) -> None:
            job.processed_chunks = ev["index"] + 1
            session.add(job)
            session.commit()

        try:
            with store.capture(ks.graph_iri, revert_on_error=True) as cap:
                result = await extract.extract_tbox_from_chunks(
                    base_iri=ks.base_iri, graph_iri=ks.graph_iri, chunks=chunks, model=model, progress=progress,
                )
                recon = await asyncio.to_thread(tbox_reconcile.reconcile, ks_id, ks.graph_iri, ks.base_iri, model)
            added_nt, removed_nt = cap.diff()
            if recon:
                result["log"] = (result.get("log", "") + "\nreconciled: " + "; ".join(recon)).strip()
            for axiom_key, chunk_id in result["provenance"]:
                session.add(AxiomProvenance(
                    knowledge_system_id=ks_id, axiom_key=axiom_key, chunk_id=chunk_id, job_id=job.id,
                ))
            job.log = result["log"]
            job.classes_added = result["classes_added"]
            job.properties_added = result["properties_added"]
            job.axioms_added = result["axioms_added"]
            job.processed_chunks = job.total_chunks
            session.add(job)
            session.commit()
            refresh_ks_stats(session, ks)

            # Detect conflicts BEFORE marking the job completed, so a client that reacts to
            # "completed" already sees the conflict queue. Off the event loop (embeddings /
            # HTTP), so progress-polling requests stay responsive meanwhile.
            await asyncio.to_thread(_sync_conflicts_bg, ks_id)
            # Agent triages the auto-resolvable conflicts (duplicates / over-specialized predicates):
            # applies the confident ones, recommends the rest. Runs before ABox so predicate merges
            # (which repoint ABox usages) act on a still-empty ABox.
            await asyncio.to_thread(conflict_agent.resolve_open_conflicts_bg, ks_id, model)
            # Attach classes the LLM left unrooted (no parent, no relations) under a broader kind.
            await asyncio.to_thread(structure_agent.attach_isolated_bg, ks_id, model)
            refresh_ks_stats(session, ks)  # agents may have merged/added classes → re-sync cached stats

            job = session.get(ExtractionJob, job_id)
            job.status = "completed"
            job.finished_at = utcnow()
            session.add(job)
            session.commit()
            _mark_docs_extracted(session, _ok_chunk_ids(result), "tbox")
            audit.record(
                session, ks_id=ks_id, action="extraction.run",
                summary=(
                    f"Extracted from {len(chunks)} chunk(s) ({model}): "
                    f"+{job.classes_added} classes / +{job.properties_added} properties / +{job.axioms_added} axioms"
                ),
                actor_id=actor_id, actor_name=actor_name,
                detail={"job_id": job.id, "model": model, "chunks": len(chunks)},
                added=added_nt, removed=removed_nt,
            )
        except Exception as e:  # noqa: BLE001
            logger.exception("extraction job %s failed", job_id)
            session.rollback()
            job = session.get(ExtractionJob, job_id)
            if job:
                job.status = "failed"
                job.error = str(e)
                job.finished_at = utcnow()
                session.add(job)
                session.commit()


async def _run_abox_extraction_job(
    job_id: int, ks_id: int, chunks: list[tuple[int, str]], model: str,
    actor_id: int | None = None, actor_name: str = "system",
) -> None:
    """Background worker: extract individuals + assertions into the ABox graph, resolving
    each mention. Recorded as a graph-scoped (``abox.extract``) history event."""
    with Session(engine) as session:
        job = session.get(ExtractionJob, job_id)
        ks = session.get(KnowledgeSystem, ks_id)
        if not job or not ks:
            return
        from app import model_config
        model_config.set_ks_connections(session, ks)  # route LLM/embedding to the KS's providers
        abox_iri = abox_iri_for(ks)
        job.status = "running"
        session.add(job)
        session.commit()

        async def progress(ev: dict) -> None:
            job.processed_chunks = ev["index"] + 1
            session.add(job)
            session.commit()

        try:
            with store.capture(abox_iri, revert_on_error=True) as cap:
                result = await abox_extract.extract_instances_from_chunks(
                    base_iri=ks.base_iri, graph_iri=ks.graph_iri, abox_iri=abox_iri,
                    ks_id=ks_id, chunks=chunks, model=model, progress=progress,
                )
            added_nt, removed_nt = cap.diff()
            job.log = result["log"]
            job.individuals_added = result["created"]
            job.assertions_added = result["assertions"]
            job.pending_added = result["queued"]
            job.unknown_classes = result.get("unknown_classes", {})
            job.axioms_added = result["created"] + result["matched"]  # individuals touched
            job.processed_chunks = job.total_chunks
            job.status = "completed"
            job.finished_at = utcnow()
            session.add(job)
            session.commit()
            _mark_docs_extracted(session, _ok_chunk_ids(result), "abox")
            audit.record(
                session, ks_id=ks_id, action="abox.extract",
                summary=(
                    f"Extracted instances from {len(chunks)} chunk(s) ({model}): "
                    f"+{result['created']} new / {result['matched']} linked / "
                    f"{result['queued']} queued / {result['assertions']} assertions"
                ),
                actor_id=actor_id, actor_name=actor_name,
                detail={"job_id": job.id, "model": model, "chunks": len(chunks), **{
                    k: result[k] for k in ("created", "matched", "queued", "assertions")}},
                added=added_nt, removed=removed_nt, graph=abox_iri,
            )
            # Agent triages datatype violations now that instances exist (relax mistyped numeric
            # properties to text / drop noisy values), applying only the confident calls.
            await asyncio.to_thread(validation_agent.triage_bg, ks_id, model)
        except Exception as e:  # noqa: BLE001
            logger.exception("abox extraction job %s failed", job_id)
            session.rollback()
            job = session.get(ExtractionJob, job_id)
            if job:
                job.status = "failed"
                job.error = str(e)
                job.finished_at = utcnow()
                session.add(job)
                session.commit()


async def _run_combined_extraction_job(
    job_id: int, ks_id: int, chunks: list[tuple[int, str]], model: str,
    actor_id: int | None = None, actor_name: str = "system",
) -> None:
    """Background worker for the one-click 'schema + instances' flow: run TBox extraction
    first, then ABox extraction over the SAME chunks (so instances type against the schema
    just extracted). Two graph-scoped history events are recorded (extraction.run on the TBox
    graph, abox.extract on the ABox graph), keeping rollback per-layer."""
    chunk_ids = [c[0] for c in chunks]
    n = len(chunks)
    with Session(engine) as session:
        job = session.get(ExtractionJob, job_id)
        ks = session.get(KnowledgeSystem, ks_id)
        if not job or not ks:
            return
        from app import model_config
        model_config.set_ks_connections(session, ks)  # route LLM/embedding to the KS's providers
        abox_iri = abox_iri_for(ks)
        job.status = "running"
        session.add(job)
        session.commit()

        async def prog_tbox(ev: dict) -> None:
            job.processed_chunks = ev["index"] + 1
            session.add(job)
            session.commit()

        async def prog_abox(ev: dict) -> None:
            job.processed_chunks = n + ev["index"] + 1
            session.add(job)
            session.commit()

        try:
            # Phase 1 — TBox (+ agentic domain/range reconciliation)
            with store.capture(ks.graph_iri, revert_on_error=True) as cap_t:
                tres = await extract.extract_tbox_from_chunks(
                    base_iri=ks.base_iri, graph_iri=ks.graph_iri, chunks=chunks, model=model, progress=prog_tbox,
                )
                recon = await asyncio.to_thread(tbox_reconcile.reconcile, ks_id, ks.graph_iri, ks.base_iri, model)
            t_add, t_rem = cap_t.diff()
            if recon:
                tres["log"] = (tres.get("log", "") + "\nreconciled: " + "; ".join(recon)).strip()
            for axiom_key, chunk_id in tres["provenance"]:
                session.add(AxiomProvenance(
                    knowledge_system_id=ks_id, axiom_key=axiom_key, chunk_id=chunk_id, job_id=job.id,
                ))
            job.classes_added = tres["classes_added"]
            job.properties_added = tres["properties_added"]
            job.axioms_added = tres["axioms_added"]
            session.add(job)
            session.commit()
            refresh_ks_stats(session, ks)
            await asyncio.to_thread(_sync_conflicts_bg, ks_id)
            # Agent triages the auto-resolvable conflicts (duplicates / over-specialized predicates):
            # applies the confident ones, recommends the rest. Runs before ABox so predicate merges
            # (which repoint ABox usages) act on a still-empty ABox.
            await asyncio.to_thread(conflict_agent.resolve_open_conflicts_bg, ks_id, model)
            # Attach classes the LLM left unrooted (no parent, no relations) under a broader kind.
            await asyncio.to_thread(structure_agent.attach_isolated_bg, ks_id, model)
            refresh_ks_stats(session, ks)  # agents may have merged/added classes → re-sync cached stats
            _mark_docs_extracted(session, _ok_chunk_ids(tres), "tbox")
            audit.record(
                session, ks_id=ks_id, action="extraction.run",
                summary=(f"Extracted from {n} chunk(s) ({model}): +{tres['classes_added']} classes / "
                         f"+{tres['properties_added']} properties / +{tres['axioms_added']} axioms"),
                actor_id=actor_id, actor_name=actor_name,
                detail={"job_id": job.id, "model": model, "chunks": n, "combined": True},
                added=t_add, removed=t_rem,
            )

            # Phase 2 — ABox (types against the schema just extracted)
            ks = session.get(KnowledgeSystem, ks_id)
            with store.capture(abox_iri, revert_on_error=True) as cap_a:
                ares = await abox_extract.extract_instances_from_chunks(
                    base_iri=ks.base_iri, graph_iri=ks.graph_iri, abox_iri=abox_iri,
                    ks_id=ks_id, chunks=chunks, model=model, progress=prog_abox,
                )
            a_add, a_rem = cap_a.diff()
            job.individuals_added = ares["created"]
            job.assertions_added = ares["assertions"]
            job.pending_added = ares["queued"]
            job.unknown_classes = ares.get("unknown_classes", {})
            job.log = f"TBox:\n{tres['log']}\n\nABox:\n{ares['log']}"
            job.processed_chunks = 2 * n
            job.status = "completed"
            job.finished_at = utcnow()
            session.add(job)
            session.commit()
            _mark_docs_extracted(session, _ok_chunk_ids(ares), "abox")
            audit.record(
                session, ks_id=ks_id, action="abox.extract",
                summary=(f"Extracted instances from {n} chunk(s) ({model}): +{ares['created']} new / "
                         f"{ares['matched']} linked / {ares['queued']} queued / {ares['assertions']} assertions"),
                actor_id=actor_id, actor_name=actor_name,
                detail={"job_id": job.id, "model": model, "chunks": n, "combined": True, **{
                    k: ares[k] for k in ("created", "matched", "queued", "assertions")}},
                added=a_add, removed=a_rem, graph=abox_iri,
            )
            # Agent triages datatype violations (relax mistyped numeric props / drop noise).
            await asyncio.to_thread(validation_agent.triage_bg, ks_id, model)
        except Exception as e:  # noqa: BLE001
            logger.exception("combined extraction job %s failed", job_id)
            session.rollback()
            job = session.get(ExtractionJob, job_id)
            if job:
                job.status = "failed"
                job.error = str(e)
                job.finished_at = utcnow()
                session.add(job)
                session.commit()


def _select_chunks(session: Session, ks: KnowledgeSystem, chunk_ids: list[int]) -> list[tuple[int, str]]:
    """Chunk (id, text) pairs, restricted to chunks whose document is bound to this KS.
    De-duplicates chunk_ids (preserving order) so a repeated id isn't extracted twice."""
    seen: set[int] = set()
    unique_ids = [cid for cid in chunk_ids if not (cid in seen or seen.add(cid))]
    rows = session.exec(select(Chunk).where(Chunk.id.in_(unique_ids))).all()
    doc_ids = {c.document_id for c in rows}
    ks_doc_ids = {
        d.id
        for d in session.exec(
            select(Document).where(Document.id.in_(doc_ids), Document.knowledge_system_id == ks.id)
        ).all()
    } if doc_ids else set()
    by_id = {c.id: c for c in rows if c.document_id in ks_doc_ids}
    return [(cid, by_id[cid].text) for cid in unique_ids if cid in by_id]


def _resolve_model(session: Session, ks: KnowledgeSystem, body: ExtractRequest) -> str:
    """Effective extraction model: request > per-KS > system runtime config > .env default."""
    from app.model_config import resolve_extract_model

    return resolve_extract_model(session, ks, body.model)


@router.post("/{ks_id}/extract", response_model=ExtractionJob)
async def run_extraction(
    body: ExtractRequest,
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> ExtractionJob:
    """Start a background extraction and return the job immediately (poll it for progress)."""
    if not body.chunk_ids:
        raise HTTPException(status_code=400, detail="No chunks selected")
    if extraction_active(session, ks.id):
        raise HTTPException(status_code=409, detail="An extraction is already in progress; try again after it finishes.")

    chunks = _select_chunks(session, ks, body.chunk_ids)
    if not chunks:
        raise HTTPException(status_code=404, detail="Selected chunks not found in this knowledge system")

    model = _resolve_model(session, ks, body)
    job = ExtractionJob(
        knowledge_system_id=ks.id,
        status="pending",
        model=model,
        chunk_ids=[c[0] for c in chunks],
        total_chunks=len(chunks),
    )
    session.add(job)
    session.commit()
    session.refresh(job)

    _spawn(_run_extraction_job(job.id, ks.id, chunks, model, actor_id=user.id, actor_name=user.username))
    return job


@router.post("/{ks_id}/extract-instances", response_model=ExtractionJob)
async def run_instance_extraction(
    body: ExtractRequest,
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> ExtractionJob:
    """Start a background ABox (instance) extraction: pull individuals + assertions from the
    selected chunks, typed by the existing TBox, resolving each mention against the entity
    resolution memory (ambiguous → manual queue)."""
    if not body.chunk_ids:
        raise HTTPException(status_code=400, detail="No chunks selected")
    if extraction_active(session, ks.id):
        raise HTTPException(status_code=409, detail="An extraction is already in progress; try again after it finishes.")
    if ks.class_count == 0:
        raise HTTPException(status_code=400, detail="This knowledge system has no classes yet — extract a TBox first.")

    chunks = _select_chunks(session, ks, body.chunk_ids)
    if not chunks:
        raise HTTPException(status_code=404, detail="Selected chunks not found in this knowledge system")

    model = _resolve_model(session, ks, body)
    job = ExtractionJob(
        knowledge_system_id=ks.id, kind="abox", status="pending", model=model,
        chunk_ids=[c[0] for c in chunks], total_chunks=len(chunks),
    )
    session.add(job)
    session.commit()
    session.refresh(job)

    _spawn(_run_abox_extraction_job(job.id, ks.id, chunks, model, actor_id=user.id, actor_name=user.username))
    return job


@router.post("/{ks_id}/extract-all", response_model=ExtractionJob)
async def run_combined_extraction(
    body: ExtractRequest,
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> ExtractionJob:
    """One-click 'schema + instances': extract the TBox from the selected chunks, then the
    ABox from the same chunks, in a single background job (ordered so instances type against
    the freshly-extracted schema)."""
    if not body.chunk_ids:
        raise HTTPException(status_code=400, detail="No chunks selected")
    if extraction_active(session, ks.id):
        raise HTTPException(status_code=409, detail="An extraction is already in progress; try again after it finishes.")

    chunks = _select_chunks(session, ks, body.chunk_ids)
    if not chunks:
        raise HTTPException(status_code=404, detail="Selected chunks not found in this knowledge system")

    model = _resolve_model(session, ks, body)
    job = ExtractionJob(
        knowledge_system_id=ks.id, kind="both", status="pending", model=model,
        chunk_ids=[c[0] for c in chunks], total_chunks=2 * len(chunks),
    )
    session.add(job)
    session.commit()
    session.refresh(job)

    _spawn(_run_combined_extraction_job(job.id, ks.id, chunks, model, actor_id=user.id, actor_name=user.username))
    return job


@router.get("/{ks_id}/jobs", response_model=list[ExtractionJob])
def list_jobs(
    ks: KnowledgeSystem = Depends(ks_reader), session: Session = Depends(get_session)
) -> list[ExtractionJob]:
    return list(
        session.exec(
            select(ExtractionJob)
            .where(ExtractionJob.knowledge_system_id == ks.id)
            .order_by(ExtractionJob.created_at.desc())
        ).all()
    )


@router.get("/{ks_id}/jobs/{job_id}", response_model=ExtractionJob)
def get_job(
    job_id: int, ks: KnowledgeSystem = Depends(ks_reader), session: Session = Depends(get_session)
) -> ExtractionJob:
    job = session.get(ExtractionJob, job_id)
    if not job or job.knowledge_system_id != ks.id:
        raise HTTPException(status_code=404, detail="Job not found")
    return job
