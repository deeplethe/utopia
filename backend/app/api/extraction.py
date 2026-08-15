"""Run LLM TBox extraction as a background job; clients poll job progress."""
from __future__ import annotations

import asyncio
import logging

from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel
from sqlalchemy.exc import OperationalError
from sqlmodel import Session, select

from app.api.conflicts import sync_conflicts
from app.api.knowledge import refresh_ks_stats
from app.config import settings
from app.db.database import engine, get_session
from app.db.models import AxiomProvenance, Chunk, Document, ExtractionJob, KnowledgeSystem, User, utcnow
from app.api.abox import abox_iri_for
from app.permissions import extraction_active, ks_reader, ks_writer
from app.security import current_user
from app import audit, model_config, prompt_config
from app.ontology import (
    abox_extract, conflict_agent, extract, retrieval, schema, skos, store, structure_agent, tbox_reconcile,
    terminology_agent, terminology_sync, validation_agent, workbench,
)

logger = logging.getLogger(__name__)
router = APIRouter(prefix="/api/knowledge", tags=["extraction"])

# Keep strong references to in-flight background tasks so they aren't GC'd.
_tasks: set[asyncio.Task] = set()


def _spawn(coro) -> None:
    task = asyncio.create_task(coro)
    _tasks.add(task)
    task.add_done_callback(_tasks.discard)


def _database_is_locked(error: BaseException) -> bool:
    current: BaseException | None = error
    while current is not None:
        if "database is locked" in str(current).casefold():
            return True
        current = current.__cause__ or current.__context__
    return False


def _write_job_fields(job_id: int, fields: dict[str, object]) -> bool:
    with Session(engine) as session:
        job = session.get(ExtractionJob, job_id)
        if not job:
            return False
        for field, value in fields.items():
            setattr(job, field, value)
        session.add(job)
        session.commit()
        return True


async def _update_job_fields(job_id: int, **fields: object) -> bool:
    """Persist lightweight job state without making extraction depend on progress writes."""
    delay = 0.25
    for attempt in range(6):
        try:
            return await asyncio.to_thread(_write_job_fields, job_id, fields)
        except OperationalError as error:
            if not _database_is_locked(error):
                raise
            if attempt == 5:
                logger.warning("job %s progress update skipped after SQLite lock retries", job_id)
                return False
            await asyncio.sleep(delay)
            delay = min(delay * 2, 4.0)
    return False


def _ok_chunk_ids(result: dict) -> list[int]:
    """Chunk ids processed without error. Only these mark their document extracted — so a document
    whose chunks all failed (e.g. LLM rate-limited) isn't silently shown as done and skipped on a
    re-run. A chunk that succeeded with 0 axioms still counts (it WAS processed)."""
    return [e["chunk_id"] for e in result.get("per_chunk", []) if e.get("status") == "ok"]


def _require_any_success(result: dict, layer: str) -> None:
    """Reject an outage-shaped run instead of reporting an all-failed batch as completed."""
    entries = [entry for entry in result.get("per_chunk", []) if isinstance(entry, dict)]
    if not entries or any(entry.get("status") == "ok" for entry in entries):
        return
    details = "; ".join(
        str(entry.get("error") or entry.get("status") or "unknown error")[:160]
        for entry in entries[:3]
    )
    raise RuntimeError(
        f"{layer} extraction produced no successful chunks ({len(entries)}/{len(entries)} failed)"
        + (f": {details}" if details else "")
    )


def _mark_docs_extracted(
    session: Session,
    chunk_ids: list[int],
    kind: str,
    *,
    commit: bool = True,
) -> None:
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
    if commit:
        session.commit()
    else:
        session.flush()


def _sync_conflicts_bg(ks_id: int) -> None:
    """Run conflict detection with its own session (used via asyncio.to_thread so the
    embedding model's CPU work never blocks the event loop)."""
    with Session(engine) as session:
        ks = session.get(KnowledgeSystem, ks_id)
        if ks:
            sync_conflicts(session, ks)


def _terminology_aliases(ks: KnowledgeSystem) -> dict[str, str]:
    view = schema.build_view(ks.graph_iri)
    labels = {
        entity["iri"]: entity["label"]
        for entity in view["classes"] + view["object_properties"] + view["data_properties"]
    }
    return skos.normalization_labels(skos.graph_iri_for(ks), labels)


def _terminology_rows(
    session: Session, ks_id: int, chunk_ids: list[int],
) -> list[tuple[Chunk, Document]]:
    """Load source chunks with document metadata while preserving extraction order."""
    if not chunk_ids:
        return []
    rows = session.exec(
        select(Chunk, Document)
        .join(Document, Chunk.document_id == Document.id)
        .where(
            Document.knowledge_system_id == ks_id,
            Chunk.id.in_(chunk_ids),
        )
    ).all()
    by_id = {chunk.id: (chunk, document) for chunk, document in rows}
    return [
        by_id[chunk_id]
        for chunk_id in chunk_ids
        if chunk_id in by_id
    ][: settings.terminology_suggestion_max_chunks]


def _run_terminology_bg(
    ks_id: int,
    chunk_ids: list[int],
    model: str,
    job_id: int,
    actor_id: int | None,
    actor_name: str,
) -> dict:
    """Synchronize deterministic terms, then queue uncertain LLM proposals.

    Terminology is an enrichment stage: a failure is returned to the extraction job for
    visibility, but never invalidates an ontology or instance extraction that already succeeded.
    """
    result = {
        "terms_added": 0,
        "terms_mapped": 0,
        "terminology_proposals": 0,
        "terminology_error": None,
    }
    if not settings.automatic_terminology:
        return result

    with Session(engine) as session:
        ks = session.get(KnowledgeSystem, ks_id)
        if not ks:
            result["terminology_error"] = "Knowledge system no longer exists"
            return result
        model_config.set_ks_connections(session, ks)
        graph_iri = skos.graph_iri_for(ks)

        try:
            with store.capture(graph_iri, revert_on_error=True) as capture:
                sync_result = terminology_sync.sync_from_ontology(ks)
            added, removed = capture.diff()
            result["terms_added"] = sync_result["terms_added"]
            result["terms_mapped"] = sync_result["terms_mapped"]
            if added or removed:
                audit.record(
                    session,
                    ks_id=ks_id,
                    action="terminology.sync",
                    summary=(
                        "Automatically synchronized controlled terminology: "
                        f"+{sync_result['terms_added']} terms / "
                        f"{sync_result['terms_mapped']} mappings / "
                        f"{sync_result['aliases_added']} aliases"
                    ),
                    actor_id=actor_id,
                    actor_name=actor_name,
                    detail={"job_id": job_id, **sync_result},
                    added=added,
                    removed=removed,
                    graph=graph_iri,
                )
        except Exception as exc:  # noqa: BLE001
            logger.exception("terminology sync failed for extraction job %s", job_id)
            session.rollback()
            result["terminology_error"] = f"Automatic vocabulary sync failed: {exc}"
            return result

        scheme_iri = sync_result.get("scheme_iri")
        if not settings.terminology_suggest_during_extraction or not scheme_iri:
            return result
        rows = _terminology_rows(session, ks_id, chunk_ids)
        if not rows:
            return result
        try:
            proposals = terminology_agent.suggest(
                session,
                ks,
                scheme_iri,
                rows,
                model=model,
                job_id=job_id,
                proposed_by="extraction-agent",
            )
            result["terminology_proposals"] = len(proposals)
            if proposals:
                audit.record(
                    session,
                    ks_id=ks_id,
                    action="terminology.suggest",
                    summary=f"Extraction agent proposed {len(proposals)} terminology change(s)",
                    actor_id=actor_id,
                    actor_name=actor_name,
                    detail={
                        "job_id": job_id,
                        "scheme_iri": scheme_iri,
                        "proposals": len(proposals),
                        "model": model,
                    },
                )
        except Exception as exc:  # noqa: BLE001
            logger.exception("terminology suggestions failed for extraction job %s", job_id)
            session.rollback()
            result["terminology_error"] = f"Terminology suggestions failed: {exc}"
    return result


def _apply_terminology_result(job: ExtractionJob, result: dict) -> None:
    job.terms_added = result["terms_added"]
    job.terms_mapped = result["terms_mapped"]
    job.terminology_proposals = result["terminology_proposals"]
    job.terminology_error = result["terminology_error"]


class ExtractRequest(BaseModel):
    chunk_ids: list[int]
    model: str | None = None
    agentic_resolution: bool | None = None


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
        prompt_config.set_ks_prompts(session, ks.id)
        job.status = "running"
        job.phase = "tbox"
        session.add(job)
        session.commit()

        async def progress(ev: dict) -> None:
            stage = ev.get("type", "chunk")
            if stage == "chunk":
                fields = {"phase": "tbox", "processed_chunks": ev["index"] + 1}
            elif stage == "role_recovery":
                fields = {"phase": "role_recovery", "processed_chunks": job.total_chunks}
            elif stage == "hierarchy":
                fields = {"phase": "hierarchy", "processed_chunks": job.total_chunks}
            else:
                return
            await _update_job_fields(job_id, **fields)

        try:
            terminology_aliases = _terminology_aliases(ks)
            abox_iri = abox_iri_for(ks)
            with store.capture(ks.graph_iri, revert_on_error=True) as cap, \
                    store.capture(abox_iri, revert_on_error=True):
                baseline_errors = workbench.structural_error_signatures(ks.graph_iri)
                result = await extract.extract_tbox_from_chunks(
                    base_iri=ks.base_iri, graph_iri=ks.graph_iri, chunks=chunks, model=model,
                    progress=progress, terminology_aliases=terminology_aliases,
                )
                _require_any_success(result, "TBox")
                job.phase = "reconciling"
                session.add(job)
                recon, reconciliation_rows = await asyncio.to_thread(
                    tbox_reconcile.reconcile,
                    ks_id,
                    ks.graph_iri,
                    ks.base_iri,
                    model,
                )
                new_errors = workbench.new_structural_errors(ks.graph_iri, baseline_errors)
                if new_errors:
                    raise RuntimeError(
                        "TBox extraction introduced structural errors: " + ", ".join(new_errors)
                    )

                added_nt, removed_nt = cap.diff()
                if recon:
                    result["log"] = (result.get("log", "") + "\nreconciled: " + "; ".join(recon)).strip()
                for axiom_key, chunk_id in result["provenance"]:
                    session.add(AxiomProvenance(
                        knowledge_system_id=ks_id, axiom_key=axiom_key, chunk_id=chunk_id,
                        job_id=job.id, actor_name=actor_name,
                    ))
                session.add_all(reconciliation_rows)
                job.log = result["log"]
                job.classes_added = result["classes_added"]
                job.properties_added = result["properties_added"]
                job.axioms_added = result["axioms_added"]
                job.processed_chunks = job.total_chunks
                session.add(job)
                refresh_ks_stats(session, ks, commit=False)
                _mark_docs_extracted(session, _ok_chunk_ids(result), "tbox", commit=False)
                audit.record(
                    session, ks_id=ks_id, action="extraction.run",
                    summary=(
                        f"Extracted from {len(chunks)} chunk(s) ({model}): "
                        f"+{job.classes_added} classes / +{job.properties_added} properties / "
                        f"+{job.axioms_added} axioms"
                    ),
                    actor_id=actor_id, actor_name=actor_name,
                    detail={"job_id": job.id, "model": model, "chunks": len(chunks)},
                    added=added_nt, removed=removed_nt, commit=False,
                )
                # RDF, provenance, learned decisions, document status, stats, and
                # history cross the durable boundary together while captures can revert.
                session.commit()

            try:
                retrieval.invalidate(ks.graph_iri)
            except Exception:  # noqa: BLE001
                logger.exception("extraction cache invalidation failed for %s", ks.graph_iri)

            # Detect conflicts BEFORE marking the job completed, so a client that reacts to
            # "completed" already sees the conflict queue. Off the event loop (embeddings /
            # HTTP), so progress-polling requests stay responsive meanwhile.
            job.phase = "conflicts"
            session.add(job)
            session.commit()
            await asyncio.to_thread(_sync_conflicts_bg, ks_id)
            # Agent triages the auto-resolvable conflicts (duplicates / over-specialized predicates):
            # applies the confident ones, recommends the rest. Runs before ABox so predicate merges
            # (which repoint ABox usages) act on a still-empty ABox.
            await asyncio.to_thread(conflict_agent.resolve_open_conflicts_bg, ks_id, model)
            # Attach classes the LLM left unrooted (no parent, no relations) under a broader kind.
            job.phase = "structure"
            session.add(job)
            session.commit()
            structure_log = await asyncio.to_thread(structure_agent.attach_isolated_bg, ks_id, model)
            if structure_log:
                job.log = (job.log + "\nstructure: " + "; ".join(structure_log)).strip()
                session.add(job)
            refresh_ks_stats(session, ks)  # agents may have merged/added classes → re-sync cached stats

            job.phase = "terminology"
            session.add(job)
            session.commit()
            terminology_result = await asyncio.to_thread(
                _run_terminology_bg,
                ks_id,
                [chunk_id for chunk_id, _ in chunks],
                model,
                job_id,
                actor_id,
                actor_name,
            )
            _apply_terminology_result(job, terminology_result)
            if job.terminology_error:
                job.log = (job.log + f"\nterminology: {job.terminology_error}").strip()
            job.phase = "completed"
            job.status = "completed"
            job.error = None
            job.finished_at = utcnow()
            session.add(job)
            session.commit()
        except Exception as e:  # noqa: BLE001
            logger.exception("extraction job %s failed", job_id)
            session.rollback()
            await _update_job_fields(
                job_id, phase="failed", status="failed", error=str(e), finished_at=utcnow(),
            )


async def _run_abox_extraction_job(
    job_id: int, ks_id: int, chunks: list[tuple[int, str]], model: str,
    actor_id: int | None = None, actor_name: str = "system",
    agentic_resolution: bool | None = None,
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
        prompt_config.set_ks_prompts(session, ks.id)
        abox_iri = abox_iri_for(ks)
        job.status = "running"
        job.phase = "abox"
        session.add(job)
        session.commit()

        extraction_committed = False

        async def progress(ev: dict) -> None:
            # Keep progress in the extraction transaction.  A second writer
            # cannot update this row while SQLite holds the outer transaction,
            # and committing here would split job state from RDF/provenance.
            job.processed_chunks = ev["index"] + 1
            session.add(job)

        try:
            # ABox validity depends on the TBox, so lock the semantic pair in the
            # same order as every other ontology mutation.  The graph captures
            # remain active through the one SQL commit so either side can be
            # compensated if validation, provenance, audit, or commit fails.
            with store.capture(ks.graph_iri, revert_on_error=True), store.capture(
                abox_iri, revert_on_error=True,
            ) as cap:
                baseline_errors = workbench.structural_error_signatures(ks.graph_iri)
                result = await abox_extract.extract_instances_from_chunks(
                    base_iri=ks.base_iri, graph_iri=ks.graph_iri, abox_iri=abox_iri,
                    ks_id=ks_id, chunks=chunks, job_id=job.id, actor_name=actor_name,
                    model=model, progress=progress,
                    agentic_resolution=agentic_resolution,
                    session=session, commit=False, fail_fast=True,
                )
                _require_any_success(result, "ABox")
                new_errors = workbench.new_structural_errors(ks.graph_iri, baseline_errors)
                if new_errors:
                    raise RuntimeError(
                        "ABox extraction introduced structural errors: " + ", ".join(new_errors)
                    )
                added_nt, removed_nt = cap.diff()
                job.log = result["log"]
                job.individuals_added = result["created"]
                job.assertions_added = result["assertions"]
                job.pending_added = result["queued"]
                job.unknown_classes = result.get("unknown_classes", {})
                job.axioms_added = result["created"] + result["matched"]  # individuals touched
                job.processed_chunks = job.total_chunks
                job.phase = "terminology"
                session.add(job)
                _mark_docs_extracted(session, _ok_chunk_ids(result), "abox", commit=False)
                # An idempotent re-run may only refresh source attribution.  It
                # has no graph history to roll back, so do not emit a no-op event.
                if added_nt or removed_nt:
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
                        added=added_nt, removed=removed_nt, graph=abox_iri, commit=False,
                    )
                session.commit()
                extraction_committed = True

            try:
                retrieval.invalidate(ks.graph_iri)
            except Exception:  # noqa: BLE001
                logger.exception("extraction cache invalidation failed for %s", ks.graph_iri)

            terminology_result = await asyncio.to_thread(
                _run_terminology_bg,
                ks_id,
                [chunk_id for chunk_id, _ in chunks],
                model,
                job_id,
                actor_id,
                actor_name,
            )
            _apply_terminology_result(job, terminology_result)
            if job.terminology_error:
                job.log = (job.log + f"\nterminology: {job.terminology_error}").strip()
            # Agent triages datatype violations now that instances exist (relax mistyped numeric
            # properties to text / drop noisy values), applying only the confident calls.
            job.phase = "finalizing"
            session.add(job)
            session.commit()
            await asyncio.to_thread(validation_agent.triage_bg, ks_id, model)
            job.phase = "completed"
            job.status = "completed"
            job.error = None
            job.finished_at = utcnow()
            session.add(job)
            session.commit()
        except Exception as e:  # noqa: BLE001
            logger.exception("abox extraction job %s failed", job_id)
            session.rollback()
            if extraction_committed:
                # The extraction transaction is already durable.  A later
                # optional terminology/validation failure must not report the
                # whole job as failed and invite a duplicate retry.
                try:
                    await _update_job_fields(
                        job_id,
                        phase="completed",
                        status="completed",
                        error=f"Post-processing failed after extraction committed: {e}",
                        finished_at=utcnow(),
                    )
                except Exception:  # noqa: BLE001
                    logger.exception("failed to finalize committed extraction job %s", job_id)
                return
            await _update_job_fields(
                job_id, phase="failed", status="failed", error=str(e), finished_at=utcnow(),
            )


async def _run_combined_extraction_job(
    job_id: int, ks_id: int, chunks: list[tuple[int, str]], model: str,
    actor_id: int | None = None, actor_name: str = "system",
    agentic_resolution: bool | None = None,
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
        prompt_config.set_ks_prompts(session, ks.id)
        abox_iri = abox_iri_for(ks)
        job.status = "running"
        job.phase = "tbox"
        session.add(job)
        session.commit()
        abox_committed = False

        async def prog_tbox(ev: dict) -> None:
            stage = ev.get("type", "chunk")
            if stage == "chunk":
                fields = {"phase": "tbox", "processed_chunks": ev["index"] + 1}
            elif stage == "role_recovery":
                fields = {"phase": "role_recovery", "processed_chunks": n}
            elif stage == "hierarchy":
                fields = {"phase": "hierarchy", "processed_chunks": n}
            else:
                return
            await _update_job_fields(job_id, **fields)

        async def prog_abox(ev: dict) -> None:
            job.processed_chunks = n + ev["index"] + 1
            session.add(job)

        try:
            # Phase 1 — TBox (+ agentic domain/range reconciliation)
            terminology_aliases = _terminology_aliases(ks)
            with store.capture(ks.graph_iri, revert_on_error=True) as cap_t, \
                    store.capture(abox_iri, revert_on_error=True):
                baseline_errors = workbench.structural_error_signatures(ks.graph_iri)
                tres = await extract.extract_tbox_from_chunks(
                    base_iri=ks.base_iri, graph_iri=ks.graph_iri, chunks=chunks, model=model,
                    progress=prog_tbox, terminology_aliases=terminology_aliases,
                )
                _require_any_success(tres, "TBox")
                job.phase = "reconciling"
                session.add(job)
                recon, reconciliation_rows = await asyncio.to_thread(
                    tbox_reconcile.reconcile,
                    ks_id,
                    ks.graph_iri,
                    ks.base_iri,
                    model,
                )
                new_errors = workbench.new_structural_errors(ks.graph_iri, baseline_errors)
                if new_errors:
                    raise RuntimeError(
                        "TBox extraction introduced structural errors: " + ", ".join(new_errors)
                    )
                t_add, t_rem = cap_t.diff()
                if recon:
                    tres["log"] = (tres.get("log", "") + "\nreconciled: " + "; ".join(recon)).strip()
                for axiom_key, chunk_id in tres["provenance"]:
                    session.add(AxiomProvenance(
                        knowledge_system_id=ks_id, axiom_key=axiom_key, chunk_id=chunk_id,
                        job_id=job.id, actor_name=actor_name,
                    ))
                session.add_all(reconciliation_rows)
                job.classes_added = tres["classes_added"]
                job.properties_added = tres["properties_added"]
                job.axioms_added = tres["axioms_added"]
                session.add(job)
                refresh_ks_stats(session, ks, commit=False)
                _mark_docs_extracted(session, _ok_chunk_ids(tres), "tbox", commit=False)
                audit.record(
                    session, ks_id=ks_id, action="extraction.run",
                    summary=(f"Extracted from {n} chunk(s) ({model}): +{tres['classes_added']} classes / "
                             f"+{tres['properties_added']} properties / +{tres['axioms_added']} axioms"),
                    actor_id=actor_id, actor_name=actor_name,
                    detail={"job_id": job.id, "model": model, "chunks": n, "combined": True},
                    added=t_add, removed=t_rem, commit=False,
                )
                session.commit()
            try:
                retrieval.invalidate(ks.graph_iri)
            except Exception:  # noqa: BLE001
                logger.exception("extraction cache invalidation failed for %s", ks.graph_iri)
            job.phase = "conflicts"
            session.add(job)
            session.commit()
            await asyncio.to_thread(_sync_conflicts_bg, ks_id)
            # Agent triages the auto-resolvable conflicts (duplicates / over-specialized predicates):
            # applies the confident ones, recommends the rest. Runs before ABox so predicate merges
            # (which repoint ABox usages) act on a still-empty ABox.
            await asyncio.to_thread(conflict_agent.resolve_open_conflicts_bg, ks_id, model)
            # Attach classes the LLM left unrooted (no parent, no relations) under a broader kind.
            job.phase = "structure"
            session.add(job)
            session.commit()
            structure_log = await asyncio.to_thread(structure_agent.attach_isolated_bg, ks_id, model)
            if structure_log:
                job.log = (job.log + "\nstructure: " + "; ".join(structure_log)).strip()
                session.add(job)
            refresh_ks_stats(session, ks)  # agents may have merged/added classes → re-sync cached stats
            job.phase = "terminology"
            session.add(job)
            session.commit()
            terminology_result = await asyncio.to_thread(
                _run_terminology_bg,
                ks_id,
                chunk_ids,
                model,
                job_id,
                actor_id,
                actor_name,
            )
            _apply_terminology_result(job, terminology_result)

            # Phase 2 — ABox (types against the schema just extracted)
            job.phase = "abox"
            session.add(job)
            session.commit()
            ks = session.get(KnowledgeSystem, ks_id)
            with store.capture(ks.graph_iri, revert_on_error=True), store.capture(
                abox_iri, revert_on_error=True,
            ) as cap_a:
                baseline_errors = workbench.structural_error_signatures(ks.graph_iri)
                ares = await abox_extract.extract_instances_from_chunks(
                    base_iri=ks.base_iri, graph_iri=ks.graph_iri, abox_iri=abox_iri,
                    ks_id=ks_id, chunks=chunks, job_id=job.id, actor_name=actor_name,
                    model=model, progress=prog_abox,
                    agentic_resolution=agentic_resolution,
                    session=session, commit=False, fail_fast=True,
                )
                _require_any_success(ares, "ABox")
                new_errors = workbench.new_structural_errors(ks.graph_iri, baseline_errors)
                if new_errors:
                    raise RuntimeError(
                        "ABox extraction introduced structural errors: " + ", ".join(new_errors)
                    )
                a_add, a_rem = cap_a.diff()
                job.individuals_added = ares["created"]
                job.assertions_added = ares["assertions"]
                job.pending_added = ares["queued"]
                job.unknown_classes = ares.get("unknown_classes", {})
                terminology_log = f"\n\nTerminology:\n{job.terminology_error}" if job.terminology_error else ""
                job.log = f"TBox:\n{tres['log']}\n\nABox:\n{ares['log']}{terminology_log}"
                job.processed_chunks = 2 * n
                job.phase = "finalizing"
                session.add(job)
                _mark_docs_extracted(session, _ok_chunk_ids(ares), "abox", commit=False)
                if a_add or a_rem:
                    audit.record(
                        session, ks_id=ks_id, action="abox.extract",
                        summary=(f"Extracted instances from {n} chunk(s) ({model}): +{ares['created']} new / "
                                 f"{ares['matched']} linked / {ares['queued']} queued / {ares['assertions']} assertions"),
                        actor_id=actor_id, actor_name=actor_name,
                        detail={"job_id": job.id, "model": model, "chunks": n, "combined": True, **{
                            k: ares[k] for k in ("created", "matched", "queued", "assertions")}},
                        added=a_add, removed=a_rem, graph=abox_iri, commit=False,
                    )
                session.commit()
                abox_committed = True

            try:
                retrieval.invalidate(ks.graph_iri)
            except Exception:  # noqa: BLE001
                logger.exception("extraction cache invalidation failed for %s", ks.graph_iri)

            # Agent triages datatype violations (relax mistyped numeric props / drop noise).
            await asyncio.to_thread(validation_agent.triage_bg, ks_id, model)
            job.phase = "completed"
            job.status = "completed"
            job.finished_at = utcnow()
            session.add(job)
            session.commit()
        except Exception as e:  # noqa: BLE001
            logger.exception("combined extraction job %s failed", job_id)
            session.rollback()
            if abox_committed:
                try:
                    await _update_job_fields(
                        job_id,
                        phase="completed",
                        status="completed",
                        error=f"Post-processing failed after extraction committed: {e}",
                        finished_at=utcnow(),
                    )
                except Exception:  # noqa: BLE001
                    logger.exception("failed to finalize committed combined extraction job %s", job_id)
                return
            await _update_job_fields(
                job_id, phase="failed", status="failed", error=str(e), finished_at=utcnow(),
            )


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
        prompt_snapshot=prompt_config.snapshot(session, ks.id),
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
        prompt_snapshot=prompt_config.snapshot(session, ks.id),
        chunk_ids=[c[0] for c in chunks], total_chunks=len(chunks),
    )
    session.add(job)
    session.commit()
    session.refresh(job)

    _spawn(_run_abox_extraction_job(
        job.id, ks.id, chunks, model, actor_id=user.id, actor_name=user.username,
        agentic_resolution=body.agentic_resolution,
    ))
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
        prompt_snapshot=prompt_config.snapshot(session, ks.id),
        chunk_ids=[c[0] for c in chunks], total_chunks=2 * len(chunks),
    )
    session.add(job)
    session.commit()
    session.refresh(job)

    _spawn(_run_combined_extraction_job(
        job.id, ks.id, chunks, model, actor_id=user.id, actor_name=user.username,
        agentic_resolution=body.agentic_resolution,
    ))
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
