"""ABox provenance: which chunk(s) each individual / assertion came from.

Mirrors the TBox ``AxiomProvenance`` idea for instance data. A fact is identified by a canonical
key (built the same way at write time in extraction and at read time in the API, so they match),
and can carry several source chunks. Recording is rebuilt per chunk on each extraction so re-runs
stay idempotent and never leave a fact attributed to a chunk that no longer mentions it.
"""
from __future__ import annotations

from sqlmodel import Session, select

from app.db.models import AboxProvenance, Chunk, Document, ExtractionJob


def ind_key(iri: str) -> str:
    return f"ind|{iri}"


def data_key(subject: str, prop: str, value: str) -> str:
    return f"data|{subject}|{prop}|{value}"


def obj_key(subject: str, prop: str, target: str) -> str:
    return f"obj|{subject}|{prop}|{target}"


def rebuild_for_chunk(
    session: Session,
    ks_id: int,
    chunk_id: int,
    fact_keys: list[str],
    job_id: int | None = None,
    actor_name: str = "extraction-agent",
) -> None:
    """Replace this chunk's provenance rows with `fact_keys` (deduped). Idempotent per chunk."""
    for row in session.exec(
        select(AboxProvenance).where(
            AboxProvenance.knowledge_system_id == ks_id, AboxProvenance.chunk_id == chunk_id
        )
    ).all():
        session.delete(row)
    for key in {k for k in fact_keys if k}:
        session.add(AboxProvenance(
            knowledge_system_id=ks_id,
            fact_key=key,
            chunk_id=chunk_id,
            job_id=job_id,
            actor_name=actor_name,
        ))


def sources_for(session: Session, ks_id: int, fact_keys: list[str], snippet_len: int = 240) -> dict[str, list[dict]]:
    """Map each fact key → its source list ``[{chunk_id, document_id, document, snippet}]``."""
    keys = [k for k in set(fact_keys) if k]
    if not keys:
        return {}
    rows = session.exec(
        select(AboxProvenance).where(
            AboxProvenance.knowledge_system_id == ks_id, AboxProvenance.fact_key.in_(keys)
        )
    ).all()
    chunk_ids = {r.chunk_id for r in rows if r.chunk_id is not None}
    chunks = {c.id: c for c in session.exec(select(Chunk).where(Chunk.id.in_(chunk_ids))).all()} if chunk_ids else {}
    doc_ids = {c.document_id for c in chunks.values()}
    docs = {d.id: d for d in session.exec(select(Document).where(Document.id.in_(doc_ids))).all()} if doc_ids else {}
    job_ids = {row.job_id for row in rows if row.job_id is not None}
    jobs = {
        job.id: job for job in session.exec(select(ExtractionJob).where(ExtractionJob.id.in_(job_ids))).all()
    } if job_ids else {}

    out: dict[str, list[dict]] = {}
    seen: set[tuple[str, int | None, int | None]] = set()
    for r in rows:
        c = chunks.get(r.chunk_id)
        identity = (r.fact_key, r.chunk_id, r.audit_event_id)
        if identity in seen:
            continue
        seen.add(identity)
        d = docs.get(c.document_id) if c else None
        job = jobs.get(r.job_id)
        out.setdefault(r.fact_key, []).append({
            "chunk_id": c.id if c else None,
            "document_id": c.document_id if c else None,
            "document": d.original_filename if d else None,
            "snippet": (c.text or "")[:snippet_len].strip() if c else "",
            "job_id": r.job_id,
            "model": job.model if job else None,
            "prompt_snapshot": job.prompt_snapshot if job else None,
            "method": r.method,
            "actor": r.actor_name or None,
            "review": r.review_record or None,
        })
    return out
