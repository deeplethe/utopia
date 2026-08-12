"""Helpers for statement-level provenance created outside model extraction."""
from __future__ import annotations

import hashlib

from sqlmodel import Session, select

from app.db.models import AboxProvenance, AuditEvent, AxiomProvenance
from app.ontology import abox_provenance, store


def triple_key(subject, predicate, obj) -> str:
    line = store.dump_triples([(subject, predicate, obj)])
    return "triple|" + hashlib.sha256(line).hexdigest()


def record_tbox_diff(
    session: Session,
    ks_id: int,
    added_nt: bytes,
    removed_nt: bytes,
    event: AuditEvent,
) -> None:
    removed_keys = {triple_key(*triple) for triple in store.load_triples(removed_nt)} if removed_nt else set()
    if removed_keys:
        for row in session.exec(select(AxiomProvenance).where(
            AxiomProvenance.knowledge_system_id == ks_id,
            AxiomProvenance.axiom_key.in_(removed_keys),
        )).all():
            session.delete(row)
    for triple in store.load_triples(added_nt) if added_nt else []:
        key = triple_key(*triple)
        session.add(AxiomProvenance(
            knowledge_system_id=ks_id,
            axiom_key=key,
            method="manual",
            actor_name=event.actor_name,
            audit_event_id=event.id,
            review_record={
                "action": event.action,
                "summary": event.summary,
                "detail": event.detail,
                "statement": store.dump_triples([triple]).decode("utf-8").strip(),
            },
        ))
    session.commit()


def record_abox_fact(
    session: Session,
    ks_id: int,
    fact_key: str,
    event: AuditEvent,
    *,
    chunk_id: int | None = None,
    job_id: int | None = None,
) -> None:
    existing = session.exec(select(AboxProvenance).where(
        AboxProvenance.knowledge_system_id == ks_id,
        AboxProvenance.fact_key == fact_key,
        AboxProvenance.chunk_id == chunk_id,
    )).first()
    if existing:
        existing.method = "review" if chunk_id else "manual"
        existing.actor_name = event.actor_name
        existing.audit_event_id = event.id
        existing.review_record = {
            "action": event.action,
            "summary": event.summary,
            "detail": event.detail,
        }
        session.add(existing)
    else:
        session.add(AboxProvenance(
            knowledge_system_id=ks_id,
            fact_key=fact_key,
            chunk_id=chunk_id,
            job_id=job_id,
            method="review" if chunk_id else "manual",
            actor_name=event.actor_name,
            audit_event_id=event.id,
            review_record={
                "action": event.action,
                "summary": event.summary,
                "detail": event.detail,
            },
        ))
    session.commit()


def remove_abox_facts(session: Session, ks_id: int, fact_keys: set[str]) -> None:
    if not fact_keys:
        return
    for row in session.exec(select(AboxProvenance).where(
        AboxProvenance.knowledge_system_id == ks_id,
        AboxProvenance.fact_key.in_(fact_keys),
    )).all():
        session.delete(row)
    session.commit()


def assertion_key(subject: str, prop: str, kind: str, target: str | None, value: str | None) -> str:
    if kind == "object":
        return abox_provenance.obj_key(subject, prop, target or "")
    return abox_provenance.data_key(subject, prop, value or "")
