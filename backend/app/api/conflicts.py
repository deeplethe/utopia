"""Conflict queue: detect, list, resolve, dismiss."""
from __future__ import annotations

import asyncio

from fastapi import APIRouter, Depends, HTTPException, Query
from pydantic import BaseModel
from sqlalchemy import func, or_
from sqlmodel import Session, select

from app import audit, prompt_config
from app.api.knowledge import refresh_ks_stats
from app.db.database import get_session
from app.db.models import (
    AxiomProvenance,
    Chunk,
    Conflict,
    Document,
    KnowledgeSystem,
    TboxReconciliation,
    User,
    utcnow,
)
from app.permissions import extraction_active, ks_reader, ks_writer
from app.security import current_user
from app.ontology import conflicts as detector
from app.ontology import editor, provenance, retrieval, schema, statement_provenance, store, workbench

router = APIRouter(prefix="/api/knowledge", tags=["conflicts"])

def _iri_local(iri: str) -> str:
    return iri.rsplit("#", 1)[-1].rsplit("/", 1)[-1].rsplit(":", 1)[-1]


def _pair_key(kind: str, left: str, right: str) -> str:
    return kind + "|" + "|".join(sorted((_iri_local(left), _iri_local(right))))


def _conflict_axiom_keys(conflict: Conflict) -> set[str]:
    """Canonical provenance keys directly involved in a detected conflict."""
    payload = conflict.payload or {}
    entities = payload.get("entities", [])
    entity_iris = [entity.get("iri") for entity in entities if entity.get("iri")]
    keys: set[str] = set()

    if conflict.ctype == "duplicate":
        keys.update(f"class|{_iri_local(iri)}" for iri in entity_iris)
    elif conflict.ctype == "predicate_specialization":
        for iri in entity_iris:
            local = _iri_local(iri)
            keys.update((f"objprop|{local}", f"dataprop|{local}"))

    if conflict.ctype in ("domain_multi", "range_multi") and entity_iris:
        slot = "domain" if conflict.ctype == "domain_multi" else "range"
        prop_local = _iri_local(entity_iris[0])
        keys.update(f"{slot}|{prop_local}|{_iri_local(value)}" for value in entity_iris[1:])

    if conflict.ctype in ("disjoint_subclass", "disjoint_common") and len(entity_iris) >= 2:
        disjoint = entity_iris[-2:]
        keys.add(_pair_key("disjointWith", disjoint[0], disjoint[1]))
    elif conflict.ctype == "equiv_disjoint" and len(entity_iris) >= 2:
        keys.add(_pair_key("disjointWith", entity_iris[0], entity_iris[1]))
        keys.add(_pair_key("equivalentClass", entity_iris[0], entity_iris[1]))

    for resolution in payload.get("resolutions", []):
        operation = resolution.get("op") or {}
        if operation.get("op") != "delete_axiom":
            continue
        axiom_type = operation.get("type")
        if axiom_type == "subclass" and operation.get("sub") and operation.get("super"):
            keys.add(f"subClassOf|{_iri_local(operation['sub'])}|{_iri_local(operation['super'])}")
        elif axiom_type in ("disjoint", "equivalent") and operation.get("a") and operation.get("b"):
            kind = "disjointWith" if axiom_type == "disjoint" else "equivalentClass"
            keys.add(_pair_key(kind, operation["a"], operation["b"]))
    return keys


def _evidence_text(text: str) -> str:
    return text.strip()


def sync_conflicts(
    session: Session,
    ks: KnowledgeSystem,
    *,
    semantic: bool = True,
    commit: bool = True,
) -> list[Conflict]:
    """Re-detect conflicts and reconcile with stored ones (upsert + auto-clear stale).

    - new signature            -> create as open
    - existing open            -> refresh detail/payload
    - existing resolved + still detected -> re-open (it came back)
    - existing dismissed       -> stay dismissed (user judged it a non-issue)
    - open but no longer detected -> auto-resolve (the issue is gone)

    ``semantic=False`` skips the costly embedding+LLM duplicate pass (snappy edits). When
    skipped, existing open 'duplicate' conflicts are left untouched (not auto-cleared),
    since this pass didn't actually re-check them.
    """
    detected = detector.detect(ks.graph_iri, semantic=semantic)
    by_sig = {d.signature: d for d in detected}
    existing = session.exec(
        select(Conflict).where(Conflict.knowledge_system_id == ks.id)
    ).all()
    existing_by_sig = {c.signature: c for c in existing}

    for sig, d in by_sig.items():
        c = existing_by_sig.get(sig)
        payload = {"entities": d.entities, "resolutions": d.resolutions}
        if c is None:
            session.add(Conflict(
                knowledge_system_id=ks.id, signature=sig, ctype=d.ctype,
                severity=d.severity, status="open", title=d.title, detail=d.detail,
                payload=payload,
            ))
        elif c.status == "dismissed":
            continue
        else:  # open or resolved -> ensure open + refresh
            c.status = "open"
            c.resolved_at = None
            c.resolution = None
            c.title, c.detail, c.severity, c.payload = d.title, d.detail, d.severity, payload
            session.add(c)

    for c in existing:
        if c.status == "open" and c.signature not in by_sig:
            if not semantic and c.ctype == "duplicate":
                continue  # duplicates weren't re-checked this pass; don't auto-clear them
            c.status = "resolved"
            c.resolved_at = utcnow()
            c.resolution = "auto-cleared"
            session.add(c)

    if commit:
        session.commit()
    else:
        session.flush()
    return list(session.exec(
        select(Conflict)
        .where(Conflict.knowledge_system_id == ks.id, Conflict.status == "open")
        .order_by(Conflict.severity.desc(), Conflict.id)
    ).all())


@router.post("/{ks_id}/conflicts/detect", response_model=list[Conflict])
async def detect_conflicts(
    ks: KnowledgeSystem = Depends(ks_writer), session: Session = Depends(get_session)
) -> list[Conflict]:
    from app.ontology import conflict_agent, structure_agent

    from app import model_config
    with model_config.use_ks_connections(session, ks), prompt_config.use_ks_prompts(session, ks.id):
        sync_conflicts(session, ks)
        # Triage the freshly detected auto-resolvable conflicts right here — the agent runs at
        # discovery time (extraction does the same), so there's no separate "run the agent" step.
        if not extraction_active(session, ks.id):
            await asyncio.to_thread(conflict_agent.resolve_open_conflicts_bg, ks.id, None)
            await asyncio.to_thread(structure_agent.attach_isolated_bg, ks.id, None)
            session.expire_all()
    return list(session.exec(
        select(Conflict).where(Conflict.knowledge_system_id == ks.id, Conflict.status == "open")
        .order_by(Conflict.severity.desc(), Conflict.id)
    ).all())


@router.get("/{ks_id}/conflicts", response_model=list[Conflict])
def list_conflicts(
    status: str = "open",
    ctype: str | None = None,
    ks: KnowledgeSystem = Depends(ks_reader),
    session: Session = Depends(get_session),
) -> list[Conflict]:
    q = select(Conflict).where(Conflict.knowledge_system_id == ks.id)
    if status != "all":
        q = q.where(Conflict.status == status)
    if ctype:  # e.g. "duplicate" — the class-dedup decision history
        q = q.where(Conflict.ctype == ctype)
    return list(session.exec(q.order_by(Conflict.severity.desc(), Conflict.id)).all())


@router.get("/{ks_id}/conflicts/{cid}")
def get_conflict_context(
    cid: int,
    ks: KnowledgeSystem = Depends(ks_reader),
    session: Session = Depends(get_session),
) -> dict:
    """Return one conflict with the source axioms and text evidence needed for human review."""
    conflict = session.get(Conflict, cid)
    if not conflict or conflict.knowledge_system_id != ks.id:
        raise HTTPException(status_code=404, detail="Conflict not found")

    entities = (conflict.payload or {}).get("entities", [])
    labels_by_local = {
        _iri_local(entity["iri"]): entity.get("label") or _iri_local(entity["iri"])
        for entity in entities if entity.get("iri")
    }
    entity_locals = list(labels_by_local)
    exact_keys = _conflict_axiom_keys(conflict)
    provenance_filters = []
    if exact_keys:
        provenance_filters.append(AxiomProvenance.axiom_key.in_(exact_keys))
    structural_context = conflict.ctype in ("disjoint_subclass", "disjoint_common")
    if structural_context:
        provenance_filters.extend(
            AxiomProvenance.axiom_key.contains(f"|{local}") for local in entity_locals
        )
    provenance_rows = list(session.exec(
        select(AxiomProvenance).where(
            AxiomProvenance.knowledge_system_id == ks.id,
            or_(*provenance_filters),
        )
    ).all()) if provenance_filters else []

    ranks: dict[str, int] = {}
    entity_local_set = set(entity_locals)
    for row in provenance_rows:
        parts = set(row.axiom_key.split("|")[1:])
        hits = parts & entity_local_set
        if row.axiom_key in exact_keys:
            ranks[row.axiom_key] = 0
        elif structural_context and hits and row.axiom_key.startswith("subClassOf|"):
            ranks.setdefault(row.axiom_key, 1)

    ranked_keys = sorted(ranks, key=lambda key: (ranks[key], key))
    ranked_key_set = set(ranked_keys)
    relevant_rows = [row for row in provenance_rows if row.axiom_key in ranked_key_set]

    chunk_ids = {row.chunk_id for row in relevant_rows if row.chunk_id is not None}
    chunks = {
        chunk.id: chunk
        for chunk in session.exec(select(Chunk).where(Chunk.id.in_(chunk_ids))).all()
    } if chunk_ids else {}
    document_ids = {chunk.document_id for chunk in chunks.values()}
    documents = {
        document.id: document
        for document in session.exec(select(Document).where(
            Document.id.in_(document_ids),
            Document.knowledge_system_id == ks.id,
        )).all()
    } if document_ids else {}

    rows_by_key: dict[str, list[AxiomProvenance]] = {}
    for row in relevant_rows:
        rows_by_key.setdefault(row.axiom_key, []).append(row)

    evidence = []
    for axiom_key in ranked_keys:
        axiom_rows = rows_by_key.get(axiom_key, [])
        source_count = len({
            row.chunk_id for row in axiom_rows
            if row.chunk_id is not None and row.chunk_id in chunks
        })
        sources = []
        seen_chunks: set[int] = set()
        for row in axiom_rows:
            if row.chunk_id is None or row.chunk_id in seen_chunks:
                continue
            chunk = chunks.get(row.chunk_id)
            if not chunk:
                continue
            seen_chunks.add(row.chunk_id)
            document = documents.get(chunk.document_id)
            sources.append({
                "chunk_id": chunk.id,
                "chunk_index": chunk.idx,
                "document_id": document.id if document else None,
                "document": document.original_filename if document else None,
                "folder": document.folder if document else None,
                "job_id": row.job_id,
                "snippet": _evidence_text(chunk.text),
            })
        if not sources:
            continue
        evidence.append({
            "axiom_key": axiom_key,
            "description": provenance.describe_axiom(
                axiom_key, lambda local: labels_by_local.get(local, local)
            ),
            "source_count": source_count,
            "sources": sources,
        })

    return {"conflict": conflict, "evidence": evidence}


class ResolveRequest(BaseModel):
    resolution_id: str


@router.post("/{ks_id}/conflicts/{cid}/resolve")
def resolve_conflict(
    cid: int,
    body: ResolveRequest,
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict:
    # A resolution can cascade into the ABox (merge_classes retypes instances, merge_properties
    # repoints assertion predicates) — capture that graph too so the change is audited/rollbackable.
    abox_iri = f"{ks.graph_iri.rstrip('/')}/abox"
    try:
        with store.capture(ks.graph_iri, revert_on_error=True) as cap, \
                store.capture(abox_iri, revert_on_error=True) as acap:
            # The request may have waited for another graph writer. Lock and repopulate
            # the row only now, then derive the chosen operation from that fresh payload;
            # otherwise a concurrent resolve/dismiss or conflict refresh could make this
            # decision stale while it was queued for the graph lock.
            c = session.exec(
                select(Conflict)
                .where(Conflict.id == cid, Conflict.knowledge_system_id == ks.id)
                .with_for_update()
                .execution_options(populate_existing=True)
            ).one_or_none()
            if not c:
                raise HTTPException(status_code=404, detail="Conflict not found")
            if c.status != "open":
                raise HTTPException(status_code=409, detail=f"Conflict already {c.status}")
            resolutions = (c.payload or {}).get("resolutions", [])
            chosen = next(
                (
                    resolution
                    for resolution in resolutions
                    if resolution.get("id") == body.resolution_id
                ),
                None,
            )
            if not chosen:
                raise HTTPException(status_code=400, detail="Unknown resolution id")
            if extraction_active(session, ks.id):
                raise HTTPException(
                    status_code=409,
                    detail="An extraction is in progress; try again after it finishes.",
                )

            baseline_errors = workbench.structural_error_signatures(ks.graph_iri)
            edit_result = editor.apply_edit(ks.graph_iri, ks.base_iri, chosen["op"])
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
            a_added, a_removed = acap.diff()
            if not (added_nt or removed_nt or a_added or a_removed):
                raise HTTPException(
                    status_code=409,
                    detail="The selected resolution no longer changes the ontology.",
                )

            import secrets
            gid = secrets.token_hex(8) if (a_added or a_removed) else None
            c.status = "resolved"
            c.resolved_at = utcnow()
            c.resolution = body.resolution_id
            session.add(c)
            refresh_ks_stats(session, ks, commit=False)
            open_conflicts = sync_conflicts(session, ks, semantic=False, commit=False)
            detail = {"conflict_id": cid, "resolution": body.resolution_id, "op": chosen["op"]}
            event = audit.record(
                session, ks_id=ks.id, action="conflict.resolve",
                summary=f'Resolved conflict "{c.title}" ({chosen["label"]})',
                actor_id=user.id, actor_name=user.username, detail=detail,
                added=added_nt, removed=removed_nt, group_id=gid, commit=False,
            )
            statement_provenance.record_tbox_diff(
                session, ks.id, added_nt, removed_nt, event, commit=False,
            )
            if a_added or a_removed:
                abox_event = audit.record(
                    session, ks_id=ks.id, action="conflict.resolve",
                    summary=f'Resolved conflict "{c.title}" - cascaded to instances',
                    actor_id=user.id, actor_name=user.username, detail=detail,
                    added=a_added, removed=a_removed, graph=abox_iri, group_id=gid,
                    commit=False,
                )
                statement_provenance.record_abox_diff(
                    session,
                    ks.id,
                    a_added,
                    a_removed,
                    abox_event,
                    abox_iri=abox_iri,
                    operations=[chosen["op"]],
                    results=[edit_result],
                    commit=False,
                )
            session.commit()
    except HTTPException:
        session.rollback()
        raise
    except Exception as e:  # noqa: BLE001
        session.rollback()
        raise HTTPException(status_code=400, detail=f"Resolution failed: {e}") from e

    try:
        retrieval.invalidate(ks.graph_iri)
    except Exception:  # noqa: BLE001
        # Retrieval vectors are derived cache state; the next lookup can rebuild them.
        pass

    # Record a domain/range reconciliation into the learned memory the TBox agent consults,
    # so a human's decision here teaches future automatic reconciliations.
    if c.ctype in ("domain_multi", "range_multi"):
        from app.ontology import tbox_reconcile

        slot = "domain" if c.ctype == "domain_multi" else "range"
        ents = c.payload.get("entities", [])
        prop = ents[0] if ents else {}
        rid = body.resolution_id
        choice = ("union" if rid == "union"
                  else "common_super" if rid.startswith("super-")
                  else "keep")
        if choice == "union":
            chosen_label = "union"
        else:
            cls_iri = chosen["op"].get(slot)
            chosen_label = schema.build_view(ks.graph_iri)["labels"].get(cls_iri, cls_iri) if cls_iri else None
        tbox_reconcile.record_manual(
            ks.id, slot, prop.get("label", ""), prop.get("iri"),
            [e.get("label") for e in ents[1:]], choice, chosen_label, user.username,
        )

    return {
        "resolved": cid,
        "open_conflicts": open_conflicts,
        "view": schema.build_view(ks.graph_iri),
    }


@router.post("/{ks_id}/conflicts/{cid}/dismiss", response_model=Conflict)
def dismiss_conflict(
    cid: int,
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> Conflict:
    c = session.exec(
        select(Conflict)
        .where(Conflict.id == cid, Conflict.knowledge_system_id == ks.id)
        .with_for_update()
        .execution_options(populate_existing=True)
    ).one_or_none()
    if not c or c.knowledge_system_id != ks.id:
        raise HTTPException(status_code=404, detail="Conflict not found")
    if c.status == "dismissed":
        # Idempotent retry: preserve the original timestamp and do not fabricate a
        # second audit event or SQL commit.
        return c
    c.status = "dismissed"
    c.resolved_at = utcnow()
    c.resolution = "dismissed"
    session.add(c)
    try:
        audit.record(
            session, ks_id=ks.id, action="conflict.dismiss",
            summary=f'Dismissed conflict "{c.title}"',
            actor_id=user.id, actor_name=user.username, detail={"conflict_id": cid},
            commit=False,
        )
        session.commit()
    except Exception:
        session.rollback()
        raise
    session.refresh(c)
    return c


@router.post("/{ks_id}/conflicts/{cid}/reopen", response_model=Conflict)
def reopen_conflict(
    cid: int,
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> Conflict:
    """Reconsider a resolved/dismissed conflict — the learned-memory 'revoke' for duplicate-class
    decisions: flip it back to open so the judge re-evaluates the pair. Doesn't undo any graph
    change a prior resolution made (that's a History rollback)."""
    c = session.get(Conflict, cid)
    if not c or c.knowledge_system_id != ks.id:
        raise HTTPException(status_code=404, detail="Conflict not found")
    if c.status == "open":
        raise HTTPException(status_code=400, detail="Conflict is already open")
    c.status = "open"
    c.resolved_at = None
    c.resolution = None
    session.add(c)
    audit.record(
        session, ks_id=ks.id, action="conflict.reopen",
        summary=f'Reopened conflict "{c.title}"',
        actor_id=user.id, actor_name=user.username, detail={"conflict_id": cid},
    )
    session.commit()
    session.refresh(c)
    return c


@router.get("/{ks_id}/reconciliations")
def list_reconciliations(
    q: str | None = None,
    limit: int = Query(default=50, le=1000),
    offset: int = 0,
    ks: KnowledgeSystem = Depends(ks_reader),
    session: Session = Depends(get_session),
) -> dict:
    """The learned TBox domain/range reconciliation memory — decisions the reconcile agent
    consults. One row per property/slot decision (union | common_super | keep), by agent or human."""
    conds = [TboxReconciliation.knowledge_system_id == ks.id]
    if q and q.strip():
        conds.append(TboxReconciliation.property_label.ilike(f"%{q.strip()}%"))
    total = session.exec(select(func.count(TboxReconciliation.id)).where(*conds)).one()
    rows = session.exec(
        select(TboxReconciliation).where(*conds)
        .order_by(TboxReconciliation.id.desc()).limit(limit).offset(offset)
    ).all()
    items = [{
        "id": r.id, "slot": r.slot, "property_label": r.property_label, "property_iri": r.property_iri,
        "candidates": r.candidates, "choice": r.choice, "chosen_label": r.chosen_label,
        "reason": r.reason, "resolved_by": r.resolved_by, "created_at": r.created_at.isoformat(),
    } for r in rows]
    return {"items": items, "total": total}


@router.delete("/{ks_id}/reconciliations/{rid}")
def revoke_reconciliation(
    rid: int,
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict:
    """Forget one domain/range reconciliation decision so the agent re-decides that property next
    time. Edits the agent's MEMORY only — the schema edit it made stays (undo via History)."""
    row = session.get(TboxReconciliation, rid)
    if not row or row.knowledge_system_id != ks.id:
        raise HTTPException(status_code=404, detail="Reconciliation not found")
    label = row.property_label
    session.delete(row)
    audit.record(
        session, ks_id=ks.id, action="reconciliation.revoke",
        summary=f'Forgot reconciliation memory for "{label}"',
        actor_id=user.id, actor_name=user.username, detail={"property": label, "slot": row.slot},
    )
    session.commit()
    return {"revoked": rid}


class ReasonUpdate(BaseModel):
    reason: str = ""


@router.patch("/{ks_id}/reconciliations/{rid}")
def edit_reconciliation_reason(
    rid: int,
    body: ReasonUpdate,
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict:
    """Edit the rationale for a domain/range reconciliation decision. Shown here and fed back to
    the reconcile agent's prompt (via lookup_experience) as experience. Kept short."""
    row = session.get(TboxReconciliation, rid)
    if not row or row.knowledge_system_id != ks.id:
        raise HTTPException(status_code=404, detail="Reconciliation not found")
    row.reason = (body.reason or "").strip()[:200]
    session.add(row)
    audit.record(
        session, ks_id=ks.id, action="reconciliation.edit_reason",
        summary=f'Edited reconciliation reason for "{row.property_label}"',
        actor_id=user.id, actor_name=user.username, detail={"property": row.property_label, "reason": row.reason},
    )
    session.commit()
    return {"id": rid, "reason": row.reason}
