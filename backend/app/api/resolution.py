"""Entity-resolution queue + decision log API.

The ABox extractor sends ambiguous mentions to a manual queue (``EntityResolution`` rows with
``status == "pending"``). Here a human clears them — "match" an existing individual or create
a "new" one — and the decision is written back so the same surface form resolves automatically
next time. Also exposes the accumulated decision log (the learned resolution memory).
"""
from __future__ import annotations

import logging
from datetime import datetime

from fastapi import APIRouter, Depends, HTTPException, Query
from pyoxigraph import NamedNode
from pydantic import BaseModel
from sqlalchemy import func, update
from sqlmodel import Session, select

from app import audit
from app.api.abox import abox_iri_for
from app.db.database import get_session
from app.db.models import (
    AboxProvenance, Chunk, Document, EntityResolution, KnowledgeSystem, User, utcnow,
)
from app.permissions import extraction_active, ks_reader, ks_writer
from app.security import current_user
from app.ontology import abox, abox_provenance, retrieval, schema, store, vocab, workbench

router = APIRouter(prefix="/api/knowledge", tags=["resolution"])
logger = logging.getLogger(__name__)


def _class_labels(ks: KnowledgeSystem) -> dict[str, str]:
    view = schema.build_view(ks.graph_iri)
    return {c["iri"]: c.get("label") or c["iri"] for c in view["classes"]}


def _queue_item(row: EntityResolution, class_labels: dict[str, str]) -> dict:
    context = row.context or {}
    return {
        "id": row.id,
        "surface_form": row.surface_form,
        "class_iri": row.class_iri,
        "class_label": class_labels.get(row.class_iri or "", row.class_iri),
        "confidence": row.confidence,
        "candidates": context.get("candidates", []),
        "reason": context.get("reason") or None,
        "evidence": context.get("evidence") or None,
        "pending_attributes": context.get("pending_attributes", []) or [],
        "pending_relations": context.get("pending_relations", []) or [],
        "source_chunk_id": row.source_chunk_id,
        "source_document_id": row.source_document_id,
        "created_at": row.created_at.isoformat(),
        "updated_at": row.updated_at.isoformat(),
    }


def _decision_individual_label(
    row: EntityResolution,
    individual_labels: dict[str, str],
) -> str | None:
    if not row.individual_iri:
        return None
    if label := individual_labels.get(row.individual_iri):
        return label
    context = row.context or {}
    if label := context.get("matched_label"):
        return str(label)
    for candidate in context.get("candidates", []) or []:
        if (isinstance(candidate, dict)
                and candidate.get("iri") == row.individual_iri
                and candidate.get("label")):
            return str(candidate["label"])
    return row.surface_form or None


def _record_resolution_fact(
    session: Session,
    *,
    ks_id: int,
    fact_key: str,
    event,
    chunk_id: int | None,
) -> None:
    """Attach the reviewed source mention without committing the caller's transaction."""
    chunk = session.get(Chunk, chunk_id) if chunk_id is not None else None
    document = session.get(Document, chunk.document_id) if chunk is not None else None
    provenance = session.exec(select(AboxProvenance).where(
        AboxProvenance.knowledge_system_id == ks_id,
        AboxProvenance.fact_key == fact_key,
        AboxProvenance.chunk_id == chunk_id,
    )).first()
    review_record = {
        "action": event.action,
        "summary": event.summary,
        "detail": event.detail,
    }
    if provenance is None:
        provenance = AboxProvenance(
            knowledge_system_id=ks_id,
            fact_key=fact_key,
            chunk_id=chunk_id,
            source_document_id=chunk.document_id if chunk is not None else None,
            source_document_sha256=document.sha256 if document is not None else None,
            method="review" if chunk_id is not None else "manual",
            actor_name=event.actor_name,
            audit_event_id=event.id,
            review_record=review_record,
        )
    else:
        provenance.method = "review" if chunk_id is not None else "manual"
        provenance.actor_name = event.actor_name
        provenance.audit_event_id = event.id
        provenance.review_record = review_record
        if chunk is not None:
            provenance.source_document_id = chunk.document_id
            provenance.source_document_sha256 = document.sha256 if document is not None else None
    session.add(provenance)


@router.get("/{ks_id}/resolution/queue")
def get_queue(
    q: str | None = None,
    limit: int = Query(default=50, le=1000),
    offset: int = 0,
    ks: KnowledgeSystem = Depends(ks_reader),
    session: Session = Depends(get_session),
) -> dict:
    conds = [EntityResolution.knowledge_system_id == ks.id, EntityResolution.status == "pending"]
    if q and q.strip():
        conds.append(EntityResolution.surface_form.ilike(f"%{q.strip()}%"))
    total = session.exec(select(func.count(EntityResolution.id)).where(*conds)).one()
    rows = session.exec(
        select(EntityResolution).where(*conds)
        .order_by(EntityResolution.id.desc()).limit(limit).offset(offset)
    ).all()
    class_labels = _class_labels(ks)
    return {"items": [_queue_item(r, class_labels) for r in rows], "total": total}


@router.get("/{ks_id}/resolution/decisions")
def get_decisions(
    status: str | None = None,
    q: str | None = None,
    limit: int = Query(default=50, le=1000),
    offset: int = 0,
    ks: KnowledgeSystem = Depends(ks_reader),
    session: Session = Depends(get_session),
) -> dict:
    """The learned resolution memory: decided rows (matched/new/distinct)."""
    conds = [EntityResolution.knowledge_system_id == ks.id]
    if status:
        conds.append(EntityResolution.status == status)
    else:
        conds.append(EntityResolution.status != "pending")
    if q and q.strip():
        conds.append(EntityResolution.surface_form.ilike(f"%{q.strip()}%"))
    total = session.exec(select(func.count(EntityResolution.id)).where(*conds)).one()
    rows = session.exec(
        select(EntityResolution).where(*conds)
        .order_by(EntityResolution.id.desc()).limit(limit).offset(offset)
    ).all()
    class_labels = _class_labels(ks)
    individual_index = abox.build_resolution_index(abox_iri_for(ks))
    ind_labels = individual_index.label_index()
    items = []
    for r in rows:
        ind_label = _decision_individual_label(r, ind_labels)
        individual_deleted = bool(
            r.individual_iri and not individual_index.exists(r.individual_iri)
        )
        items.append({
            "id": r.id, "surface_form": r.surface_form,
            "class_label": class_labels.get(r.class_iri or "", r.class_iri),
            "status": r.status, "individual_iri": r.individual_iri, "individual_label": ind_label,
            "individual_deleted": individual_deleted,
            "confidence": r.confidence, "resolved_by": r.resolved_by,
            "reason": (r.context or {}).get("reason") or None,
            "created_at": r.created_at.isoformat(),
            "resolved_at": r.resolved_at.isoformat() if r.resolved_at else None,
            "review_after": r.review_after.isoformat() if r.review_after else None,
        })
    return {"items": items, "total": total}


@router.delete("/{ks_id}/resolution/decisions/{res_id}")
def revoke_decision(
    res_id: int,
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict:
    """Forget one learned resolution decision so the agent re-judges that surface form next time.
    This edits the AGENT'S MEMORY only — it does NOT undo the individual already written to the
    graph (use History rollback for that)."""
    row = session.get(EntityResolution, res_id)
    if not row or row.knowledge_system_id != ks.id:
        raise HTTPException(status_code=404, detail="Decision not found")
    if row.status == "pending":
        raise HTTPException(status_code=400, detail="That is a pending queue item, not a decision")
    surface = row.surface_form
    session.delete(row)
    audit.record(
        session, ks_id=ks.id, action="resolution.revoke",
        summary=f'Forgot resolution memory for "{surface}"',
        actor_id=user.id, actor_name=user.username, detail={"surface": surface},
    )
    session.commit()
    return {"revoked": res_id}


class ReasonUpdate(BaseModel):
    reason: str = ""


@router.patch("/{ks_id}/resolution/decisions/{res_id}")
def edit_decision_reason(
    res_id: int,
    body: ReasonUpdate,
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict:
    """Edit the rationale a human/agent recorded for a resolution decision. It's shown here and
    fed back into the resolution agent's prompt (via lookup_alias) as experience. Kept short."""
    row = session.get(EntityResolution, res_id)
    if not row or row.knowledge_system_id != ks.id:
        raise HTTPException(status_code=404, detail="Decision not found")
    if row.status == "pending":
        raise HTTPException(status_code=400, detail="That is a pending queue item, not a decision")
    reason = (body.reason or "").strip()[:200]
    ctx = dict(row.context or {})
    ctx["reason"] = reason
    row.context = ctx  # reassign so SQLAlchemy detects the JSON change
    session.add(row)
    audit.record(
        session, ks_id=ks.id, action="resolution.edit_reason",
        summary=f'Edited resolution reason for "{row.surface_form}"',
        actor_id=user.id, actor_name=user.username, detail={"surface": row.surface_form, "reason": reason},
    )
    session.commit()
    return {"id": res_id, "reason": reason}


class ResolveRequest(BaseModel):
    action: str            # match | new | reject | defer
    individual_iri: str | None = None   # required for "match"
    reason: str = ""
    review_after: datetime | None = None
    expected_updated_at: datetime | None = None


def _same_decision(row: EntityResolution, body: ResolveRequest) -> bool:
    context = row.context or {}
    action = context.get("decision_action")
    if not action:
        action = {"matched": "match", "new": "new", "rejected": "reject", "deferred": "defer"}.get(row.status)
    return bool(
        action == body.action
        and (body.action != "match" or row.individual_iri == body.individual_iri)
    )


def _result(row: EntityResolution, summary: str, *, idempotent: bool = False) -> dict:
    return {
        "id": row.id,
        "status": row.status,
        "individual_iri": row.individual_iri,
        "summary": summary,
        "idempotent": idempotent,
    }


def _claim_pending(session: Session, row: EntityResolution, expected: datetime | None) -> None:
    """Atomically claim one queue item so concurrent reviewers cannot overwrite a decision."""
    now = utcnow()
    conditions = [EntityResolution.id == row.id, EntityResolution.status == "pending"]
    if expected is not None:
        conditions.append(EntityResolution.updated_at == expected)
    result = session.exec(
        update(EntityResolution)
        .where(*conditions)
        .values(status="resolving", updated_at=now)
        .execution_options(synchronize_session=False)
    )
    if result.rowcount != 1:
        session.rollback()
        raise HTTPException(status_code=409, detail="This item was changed by another reviewer; refresh and try again.")
    session.expire(row)
    session.refresh(row)


@router.post("/{ks_id}/resolution/{res_id}/resolve")
def resolve(
    res_id: int,
    body: ResolveRequest,
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict:
    row = session.get(EntityResolution, res_id)
    if not row or row.knowledge_system_id != ks.id:
        raise HTTPException(status_code=404, detail="Queue item not found")
    if row.status != "pending":
        if _same_decision(row, body):
            return _result(row, f'"{row.surface_form}" was already resolved.', idempotent=True)
        raise HTTPException(status_code=409, detail="This item has already been resolved by another decision")
    if body.action not in ("match", "new", "reject", "defer"):
        raise HTTPException(status_code=400, detail="action must be 'match', 'new', 'reject', or 'defer'")

    reason = (body.reason or "").strip()[:500]
    if body.action in ("reject", "defer"):
        if not reason:
            raise HTTPException(status_code=400, detail=f"{body.action} requires a reason")
        _claim_pending(session, row, body.expected_updated_at)
        row.status = "rejected" if body.action == "reject" else "deferred"
        row.individual_iri = None
        row.resolved_by = user.username
        row.resolved_at = utcnow()
        row.review_after = body.review_after if body.action == "defer" else None
        row.updated_at = row.resolved_at
        row.context = {
            **(row.context or {}),
            "decision_action": body.action,
            "reason": reason,
        }
        session.add(row)
        summary = (
            f'Rejected invalid mention "{row.surface_form}"'
            if body.action == "reject"
            else f'Deferred review of "{row.surface_form}"'
        )
        audit.record(
            session, ks_id=ks.id, action=f"abox.resolution.{body.action}",
            summary=summary, actor_id=user.id, actor_name=user.username,
            detail={"resolution_id": row.id, "reason": reason,
                    "review_after": row.review_after.isoformat() if row.review_after else None},
            commit=False,
        )
        session.commit()
        return _result(row, summary)
    if not row.class_iri:
        raise HTTPException(status_code=400, detail="Queue item has no class")

    abox_iri = abox_iri_for(ks)
    is_new = body.action == "new"
    if not is_new and (not body.individual_iri or not abox.exists(abox_iri, body.individual_iri)):
        raise HTTPException(status_code=400, detail="match requires an existing individual")
    # Both paths now write to the ABox graph (new mints an individual; both replay the
    # mention's queued facts onto it), so guard against a concurrent extraction.
    if extraction_active(session, ks.id):
        raise HTTPException(status_code=409, detail="An extraction is in progress; try again after it finishes.")

    ctx = row.context or {}
    pending_attrs = ctx.get("pending_attributes", []) or []
    pending_rels = ctx.get("pending_relations", []) or []
    resolved_relations: list[tuple[str, str]] = []
    replayed = 0

    try:
        # Resolution reads the TBox and mutates the paired ABox. Hold both
        # captures in the global order until the SQL transaction is durable, so
        # provenance/audit/commit failures restore the RDF graph as well.
        with store.capture(ks.graph_iri, revert_on_error=True), store.capture(
            abox_iri, revert_on_error=True,
        ) as cap:
            _claim_pending(session, row, body.expected_updated_at)
            baseline_errors = workbench.structural_error_signatures(ks.graph_iri)
            subject_iri = (
                abox.create_individual(abox_iri, ks.base_iri, row.surface_form, row.class_iri)
                if is_new else body.individual_iri
            )
            # Replay the facts captured when this mention was queued, so they aren't lost.
            for item in pending_attrs:
                if abox.add_data_assertion(
                    abox_iri,
                    subject_iri,
                    item["prop"],
                    item["value"],
                    item.get("datatype"),
                ):
                    replayed += 1
            if pending_rels:
                # Resolve each target by label against existing individuals (best effort).
                label_to_iri: dict[str, str] = {}
                for iri, label in abox.label_index(abox_iri).items():
                    label_to_iri.setdefault(label.strip().lower(), iri)
                for relation in pending_rels:
                    target = label_to_iri.get(
                        str(relation.get("target_label", "")).strip().lower()
                    )
                    if target:
                        added = abox.add_object_assertion(
                            abox_iri, subject_iri, relation["prop"], target,
                        )
                        if added:
                            replayed += 1
                        resolved_relations.append((relation["prop"], target))

            new_errors = workbench.new_structural_errors(ks.graph_iri, baseline_errors)
            if new_errors:
                raise HTTPException(
                    status_code=422,
                    detail={
                        "code": "ontology_structural_validation_failed",
                        "message": "The resolution introduces structural ontology errors.",
                        "new_error_count": len(new_errors),
                        "new_error_signatures": new_errors,
                    },
                )
            added_nt, removed_nt = cap.diff()

            row.status = "new" if is_new else "matched"
            row.individual_iri = subject_iri
            row.confidence = None if is_new else 1.0
            row.resolved_by = user.username
            row.resolved_at = utcnow()
            row.updated_at = row.resolved_at
            row.context = {
                **(row.context or {}),
                "decision_action": body.action,
                **({"reason": reason} if reason else {}),
            }
            session.add(row)
            extra = f" (+{replayed} assertion(s))" if replayed else ""
            event = audit.record(
                session, ks_id=ks.id, action="abox.resolve",
                summary=f'Resolved "{row.surface_form}" → {"new" if is_new else "existing"} individual{extra}',
                actor_id=user.id, actor_name=user.username,
                detail={"iri": subject_iri, "class_iri": row.class_iri, "from_resolution": row.id,
                        "action": body.action, "replayed": replayed},
                added=added_nt, removed=removed_nt, graph=abox_iri, commit=False,
            )
            _record_resolution_fact(
                session,
                ks_id=ks.id,
                fact_key=abox_provenance.ind_key(subject_iri),
                event=event,
                chunk_id=row.source_chunk_id,
            )
            for item in pending_attrs:
                _record_resolution_fact(
                    session,
                    ks_id=ks.id,
                    fact_key=abox_provenance.data_key(
                        subject_iri, item["prop"], item["value"],
                    ),
                    event=event,
                    chunk_id=row.source_chunk_id,
                )
            for prop, target in resolved_relations:
                _record_resolution_fact(
                    session,
                    ks_id=ks.id,
                    fact_key=abox_provenance.obj_key(subject_iri, prop, target),
                    event=event,
                    chunk_id=row.source_chunk_id,
                )
            session.commit()
    except Exception:
        session.rollback()
        raise

    try:
        retrieval.invalidate(ks.graph_iri)
    except Exception:  # noqa: BLE001
        logger.exception("ontology retrieval cache invalidation failed for %s", ks.graph_iri)
    summary = f'Resolved "{row.surface_form}" → {"new" if is_new else "existing"} individual{extra}'
    return _result(row, summary)


class MergeIndividualsRequest(BaseModel):
    source_iri: str
    canonical_iri: str
    reason: str
    resolution_id: int | None = None
    expected_updated_at: datetime | None = None


def _merged_fact_key(key: str, source: str, canonical: str) -> str:
    if key == abox_provenance.ind_key(source):
        return abox_provenance.ind_key(canonical)
    if key.startswith("data|"):
        parts = key.split("|", 3)
        if len(parts) == 4 and parts[1] == source:
            parts[1] = canonical
            return "|".join(parts)
    if key.startswith("obj|"):
        parts = key.split("|", 3)
        if len(parts) == 4:
            if parts[1] == source:
                parts[1] = canonical
            if parts[3] == source:
                parts[3] = canonical
            return "|".join(parts)
    return key


@router.post("/{ks_id}/resolution/merge")
def merge_individuals(
    body: MergeIndividualsRequest,
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict:
    """Consolidate an existing duplicate into a canonical individual, preserving evidence."""
    source = body.source_iri.strip()
    canonical = body.canonical_iri.strip()
    reason = body.reason.strip()[:500]
    if not source or not canonical or source == canonical:
        raise HTTPException(status_code=400, detail="Select two different existing individuals")
    if not reason:
        raise HTTPException(status_code=400, detail="merge requires a reason")
    if extraction_active(session, ks.id):
        raise HTTPException(status_code=409, detail="An extraction is in progress; try again after it finishes.")
    abox_iri = abox_iri_for(ks)
    queue_row = session.get(EntityResolution, body.resolution_id) if body.resolution_id else None
    if body.resolution_id and (queue_row is None or queue_row.knowledge_system_id != ks.id):
        raise HTTPException(status_code=404, detail="Queue item not found")
    if queue_row is not None and queue_row.status != "pending":
        context = queue_row.context or {}
        if (
            context.get("decision_action") == "merge"
            and context.get("merged_from_iri") == source
            and queue_row.individual_iri == canonical
        ):
            return {"source_iri": source, "canonical_iri": canonical, "idempotent": True}
        raise HTTPException(status_code=409, detail="This item has already been resolved by another decision")
    source_exists = abox.exists(abox_iri, source)
    canonical_exists = abox.exists(abox_iri, canonical)
    if not canonical_exists:
        raise HTTPException(status_code=400, detail="Canonical individual does not exist")
    already_merged = not source_exists and store.has_triple(
        abox_iri, NamedNode(source), vocab.OWL_SAME_AS, NamedNode(canonical),
    )
    if not source_exists and not already_merged:
        raise HTTPException(status_code=404, detail="Source individual does not exist")
    if already_merged and queue_row is None:
        return {"source_iri": source, "canonical_iri": canonical, "idempotent": True}

    try:
        with store.capture(ks.graph_iri, revert_on_error=True), store.capture(
            abox_iri, revert_on_error=True,
        ) as cap:
            if queue_row is not None:
                _claim_pending(session, queue_row, body.expected_updated_at)
            baseline_errors = workbench.structural_error_signatures(ks.graph_iri)
            removed = 0 if already_merged else abox.merge_individual(abox_iri, source, canonical)
            new_errors = workbench.new_structural_errors(ks.graph_iri, baseline_errors)
            if new_errors:
                raise HTTPException(status_code=422, detail={
                    "code": "ontology_structural_validation_failed",
                    "message": "The merge introduces structural ontology errors.",
                    "new_error_count": len(new_errors),
                    "new_error_signatures": new_errors,
                })
            added_nt, removed_nt = cap.diff()
            event = audit.record(
                session, ks_id=ks.id, action="abox.merge_individuals",
                summary="Merged a duplicate individual into its canonical identity",
                actor_id=user.id, actor_name=user.username,
                detail={"source_iri": source, "canonical_iri": canonical,
                        "reason": reason, "triples_rewritten": removed},
                added=added_nt, removed=removed_nt, graph=abox_iri, commit=False,
            )
            for provenance in session.exec(select(AboxProvenance).where(
                AboxProvenance.knowledge_system_id == ks.id
            )).all():
                rewritten = _merged_fact_key(provenance.fact_key, source, canonical)
                if rewritten != provenance.fact_key:
                    provenance.fact_key = rewritten
                    review = dict(provenance.review_record or {})
                    review.setdefault("identity_merges", []).append({
                        "source_iri": source,
                        "canonical_iri": canonical,
                        "audit_event_id": event.id,
                    })
                    provenance.review_record = review
                    session.add(provenance)
            for resolution in session.exec(select(EntityResolution).where(
                EntityResolution.knowledge_system_id == ks.id,
                EntityResolution.individual_iri == source,
            )).all():
                resolution.individual_iri = canonical
                resolution.updated_at = utcnow()
                resolution.context = {
                    **(resolution.context or {}),
                    "merged_from_iri": source,
                    "canonical_iri": canonical,
                    "merge_audit_event_id": event.id,
                }
                session.add(resolution)
            if queue_row is not None:
                queue_row.status = "matched"
                queue_row.individual_iri = canonical
                queue_row.confidence = 1.0
                queue_row.resolved_by = user.username
                queue_row.resolved_at = utcnow()
                queue_row.updated_at = queue_row.resolved_at
                queue_row.context = {
                    **(queue_row.context or {}),
                    "decision_action": "merge",
                    "reason": reason,
                    "merged_from_iri": source,
                    "canonical_iri": canonical,
                    "merge_audit_event_id": event.id,
                }
                session.add(queue_row)
            _record_resolution_fact(
                session, ks_id=ks.id, fact_key=abox_provenance.ind_key(canonical),
                event=event, chunk_id=None,
            )
            session.commit()
    except Exception:
        session.rollback()
        raise

    try:
        retrieval.invalidate(ks.graph_iri)
    except Exception:  # noqa: BLE001
        logger.exception("ontology retrieval cache invalidation failed for %s", ks.graph_iri)
    return {"source_iri": source, "canonical_iri": canonical, "idempotent": False}
