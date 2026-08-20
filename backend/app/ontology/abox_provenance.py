"""ABox provenance: which chunk(s) each individual / assertion came from.

Mirrors the TBox ``AxiomProvenance`` idea for instance data. A fact is identified by a canonical
key (built the same way at write time in extraction and at read time in the API, so they match),
and can carry several source chunks. Recording is rebuilt per chunk on each extraction so re-runs
stay idempotent and never leave a fact attributed to a chunk that no longer mentions it.
"""
from __future__ import annotations

from pyoxigraph import Literal, NamedNode
from sqlmodel import Session, select

from app.db.models import AboxProvenance, Chunk, Document, ExtractionJob
from app.ontology import store, vocab


def ind_key(iri: str) -> str:
    return f"ind|{iri}"


def data_key(subject: str, prop: str, value: str) -> str:
    return f"data|{subject}|{prop}|{value}"


def obj_key(subject: str, prop: str, target: str) -> str:
    return f"obj|{subject}|{prop}|{target}"


def fact_entities(key: str) -> set[str]:
    parts = key.split("|", 3)
    if len(parts) < 2:
        return set()
    if parts[0] in {"ind", "data"}:
        return {parts[1]}
    if parts[0] == "obj" and len(parts) == 4:
        return {parts[1], parts[3]}
    return set()


def describe_fact(
    key: str,
    individual_labels: dict[str, str],
    property_labels: dict[str, str],
) -> str:
    parts = key.split("|", 3)
    if len(parts) < 2:
        return key
    subject = individual_labels.get(parts[1], parts[1])
    if parts[0] == "ind":
        return f'Individual "{subject}"'
    if parts[0] == "data" and len(parts) == 4:
        return f'{subject} — {property_labels.get(parts[2], parts[2])} = "{parts[3]}"'
    if parts[0] == "obj" and len(parts) == 4:
        target = individual_labels.get(parts[3], parts[3])
        return f'{subject} — {property_labels.get(parts[2], parts[2])} → {target}'
    return key


def retract_fact_keys(abox_iri: str, keys: set[str]) -> int:
    """Retract ABox facts that lost their last source without harming shared assertions."""
    removed = 0
    triples = store.read_triples(abox_iri)
    # Assertions first; identity deletion below may then safely garbage-collect the entity.
    for key in sorted(keys):
        parts = key.split("|", 3)
        if parts[0] == "data" and len(parts) == 4:
            matches = [
                (s, p, o) for s, p, o in triples
                if isinstance(s, NamedNode) and s.value == parts[1]
                and p.value == parts[2] and isinstance(o, Literal) and o.value == parts[3]
            ]
            removed += store.remove_triples(abox_iri, matches)
        elif parts[0] == "obj" and len(parts) == 4:
            removed += store.remove_pattern(
                abox_iri, NamedNode(parts[1]), NamedNode(parts[2]), NamedNode(parts[3]),
            )

    for key in sorted(keys):
        parts = key.split("|", 1)
        if len(parts) != 2 or parts[0] != "ind":
            continue
        iri = parts[1]
        # Do not delete an identity still participating in an assertion outside this replacement
        # set (including manual/legacy assertions whose provenance may be incomplete).
        still_used = False
        for subject, predicate, obj in store.read_triples(abox_iri):
            if getattr(subject, "value", None) != iri and getattr(obj, "value", None) != iri:
                continue
            if getattr(subject, "value", None) == iri and predicate.value in {
                vocab.RDF_TYPE.value, vocab.RDFS_LABEL.value,
            }:
                continue
            represented = (
                obj_key(subject.value, predicate.value, obj.value)
                if isinstance(subject, NamedNode) and isinstance(obj, NamedNode)
                else data_key(subject.value, predicate.value, obj.value)
                if isinstance(subject, NamedNode) and isinstance(obj, Literal)
                else ""
            )
            if represented and represented not in keys:
                still_used = True
                break
        if not still_used:
            removed += store.remove_entity(abox_iri, iri)
    return removed


def rebuild_for_chunk(
    session: Session,
    ks_id: int,
    chunk_id: int,
    fact_keys: list[str],
    job_id: int | None = None,
    actor_name: str = "extraction-agent",
) -> None:
    """Replace this chunk's provenance rows with `fact_keys` (deduped). Idempotent per chunk."""
    chunk = session.get(Chunk, chunk_id)
    document = session.get(Document, chunk.document_id) if chunk is not None else None
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
            source_document_id=chunk.document_id if chunk is not None else None,
            source_document_sha256=document.sha256 if document is not None else None,
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
    doc_ids.update(r.source_document_id for r in rows if r.source_document_id is not None)
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
        d = docs.get(c.document_id) if c else docs.get(r.source_document_id)
        job = jobs.get(r.job_id)
        out.setdefault(r.fact_key, []).append({
            "chunk_id": c.id if c else None,
            "document_id": c.document_id if c else r.source_document_id,
            "document_sha256": r.source_document_sha256 or (d.sha256 if d else None),
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
