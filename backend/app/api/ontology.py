"""Read the ontology (curated JSON view + Turtle export), provenance, and manual edits."""
from __future__ import annotations

import json
import logging
import secrets
from dataclasses import asdict

from fastapi import APIRouter, Depends, HTTPException, Query, Response
from pydantic import BaseModel, ConfigDict
from sqlalchemy import delete, func
from sqlmodel import Session, select

from app import audit
from app import model_config, prompt_config
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
from app.ontology import conflicts as conflict_detector
from app.ontology import abox_validate, editor, modeling_assistant, retrieval, schema, skos, statement_provenance, store, workbench

router = APIRouter(prefix="/api/knowledge", tags=["ontology"])
logger = logging.getLogger(__name__)


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
    with store.read_lock(ks.graph_iri), store.read_lock(workbench.abox_iri_for(ks.graph_iri)):
        view = schema.build_view(ks.graph_iri)
        view["knowledge_system"] = {"id": ks.id, "name": ks.name, "base_iri": ks.base_iri}
        view["revision"] = workbench.ontology_revision(ks.graph_iri)
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
    # Optional metadata used by the workbench.  Existing callers can continue sending
    # only the edit operation exactly as before.
    expected_revision: str | None = None
    confirm_destructive: bool = False


class ChangeSetRequest(BaseModel):
    """An atomic ontology change set.

    ``ops`` and ``summary`` are accepted as compatibility aliases for early workbench
    clients; new clients should use ``operations`` and ``reason``.
    """

    operations: list[dict] | None = None
    ops: list[dict] | None = None
    expected_revision: str | None = None
    dry_run: bool = False
    reason: str = ""
    summary: str = ""
    confirm_destructive: bool = False
    include_rdf_diff: bool = True

    def edits(self) -> list[dict]:
        if self.operations is not None and self.ops is not None:
            raise ValueError("use operations or ops, not both")
        return self.operations if self.operations is not None else (self.ops or [])


class SuggestOntologyRequest(BaseModel):
    instruction: str
    expected_revision: str | None = None


_DESTRUCTIVE_OPS = {
    "delete_class", "delete_property", "delete_axiom", "merge_classes", "merge_properties",
}


def _operations(value: list[dict]) -> list[dict]:
    if not value or len(value) > 50:
        raise HTTPException(status_code=400, detail="operations must contain between 1 and 50 edits")
    if len(json.dumps(value, ensure_ascii=False)) > 200_000:
        raise HTTPException(status_code=400, detail="operations payload is too large")
    for index, operation in enumerate(value):
        if not isinstance(operation, dict) or not operation.get("op"):
            raise HTTPException(status_code=400, detail=f"operations[{index}] must contain an op")
    return value


def _revision_conflict(expected: str, current: str) -> HTTPException:
    return HTTPException(
        status_code=409,
        detail={
            "code": "ontology_revision_conflict",
            "message": "The ontology changed after this edit was prepared. Refresh and review it again.",
            "expected_revision": expected,
            "current_revision": current,
        },
    )


def _serialize_conflicts(items) -> list[dict]:
    return [asdict(item) for item in items]


def _impact_for_operation(graph_iri: str, index: int, operation: dict) -> list[dict]:
    name = operation.get("op")
    refs: list[tuple[str, str]] = []
    if name == "delete_class":
        refs.append((operation.get("iri", ""), "class"))
    elif name == "delete_property":
        refs.append((operation.get("iri", ""), "property"))
    elif name == "merge_classes":
        refs.append((operation.get("source", ""), "class"))
    elif name == "merge_properties":
        refs.extend((iri, "property") for iri in operation.get("sources", []) if iri)
    elif name == "delete_axiom":
        # No entity is deleted, but this remains visible in the destructive-operation
        # list and the exact TBox removal appears in ``diff.counts``.
        return [{
            "index": index,
            "op": name,
            "destructive": True,
            "kind": "axiom",
            "entity_iri": None,
            "exists": True,
            "tbox_triples": 1,
            "referencing_axioms": 1,
            "subclasses": [],
            "superclasses": [],
            "properties_using_class": [],
            "abox_type_assertions": 0,
            "abox_property_assertions": 0,
            "abox_assertions": 0,
            "affected_individuals": [],
            "individuals_deleted": [],
            "individuals_retyped": [],
            "_affected_individual_iris": set(),
            "_deleted_individual_iris": set(),
            "_retyped_individual_iris": set(),
        }]
    elif name not in _DESTRUCTIVE_OPS:
        return [{
            "index": index,
            "op": name,
            "destructive": False,
            "kind": None,
            "entity_iri": operation.get("iri"),
            "exists": True,
            "tbox_triples": 0,
            "referencing_axioms": 0,
            "subclasses": [],
            "superclasses": [],
            "properties_using_class": [],
            "abox_type_assertions": 0,
            "abox_property_assertions": 0,
            "abox_assertions": 0,
            "affected_individuals": [],
            "individuals_deleted": [],
            "individuals_retyped": [],
            "_affected_individual_iris": set(),
            "_deleted_individual_iris": set(),
            "_retyped_individual_iris": set(),
        }]
    details = []
    for iri, kind in refs:
        if not iri:
            continue
        item = workbench.analyze_entity_impact(graph_iri, iri, kind)
        item.update({"index": index, "op": name, "destructive": True})
        if name == "merge_classes":
            # Merge re-types every source instance onto the target; it never deletes the
            # individual even when the source was its sole domain type.
            affected = set(item.get("_affected_individual_iris", set()))
            item["individuals_deleted"] = []
            item["individuals_deleted_count"] = 0
            item["individuals_deleted_truncated"] = False
            item["individuals_retyped"] = sorted(affected)[:100]
            item["individuals_retyped_count"] = len(affected)
            item["individuals_retyped_truncated"] = len(affected) > 100
            item["_deleted_individual_iris"] = set()
            item["_retyped_individual_iris"] = affected
        details.append(item)
    return details


def _diff_payload(
    added: bytes,
    removed: bytes,
    abox_added: bytes,
    abox_removed: bytes,
    *,
    include_rdf_diff: bool = True,
) -> dict:
    return {
        "tbox_added": added.decode("utf-8") if include_rdf_diff else "",
        "tbox_removed": removed.decode("utf-8") if include_rdf_diff else "",
        "abox_added": abox_added.decode("utf-8") if include_rdf_diff else "",
        "abox_removed": abox_removed.decode("utf-8") if include_rdf_diff else "",
        "counts": {
            "tbox_added": len(store.load_triples(added)),
            "tbox_removed": len(store.load_triples(removed)),
            "abox_added": len(store.load_triples(abox_added)),
            "abox_removed": len(store.load_triples(abox_removed)),
        },
    }


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
    changed = False
    removed_rows: dict[str, int] = {}
    removed_triples: dict[str, int] = {
        "ontology": 0,
        "instances": 0,
        "vocabulary": 0,
    }
    documents_reset = 0

    try:
        # Keep every graph locked until the one SQL commit succeeds. If that commit
        # fails, the captures compensate all three immediate RDF mutations before the
        # locks are released.
        with store.capture(graph_iris[0], revert_on_error=True) as tbox_capture, \
                store.capture(graph_iris[1], revert_on_error=True) as abox_capture, \
                store.capture(graph_iris[2], revert_on_error=True) as vocabulary_capture:
            removed_rows = {
                model.__tablename__: session.exec(
                    select(func.count(model.id)).where(model.knowledge_system_id == ks.id)
                ).one()
                for model in row_models
            }
            documents = session.exec(
                select(Document).where(Document.knowledge_system_id == ks.id)
            ).all()
            documents_reset = sum(
                document.tbox_extracted_at is not None or document.abox_extracted_at is not None
                for document in documents
            )
            stale_stats = bool(ks.class_count or ks.property_count or ks.axiom_count)

            for graph_iri in graph_iris:
                store.clear_graph(graph_iri)

            graph_diffs = (
                ("ontology", graph_iris[0], tbox_capture.diff()),
                ("instances", graph_iris[1], abox_capture.diff()),
                ("vocabulary", graph_iris[2], vocabulary_capture.diff()),
            )
            for layer, _graph_iri, (_added_nt, removed_nt) in graph_diffs:
                removed_triples[layer] = (
                    len(store.load_triples(removed_nt)) if removed_nt else 0
                )

            changed = bool(
                any(removed_triples.values())
                or any(removed_rows.values())
                or documents_reset
                or stale_stats
            )
            if changed:
                for model in row_models:
                    session.exec(delete(model).where(model.knowledge_system_id == ks.id))
                for document in documents:
                    if document.tbox_extracted_at is None and document.abox_extracted_at is None:
                        continue
                    document.tbox_extracted_at = None
                    document.abox_extracted_at = None
                    session.add(document)

                refresh_ks_stats(session, ks, commit=False)
                group_id = secrets.token_hex(8)
                audited_graph = False
                for layer, graph_iri, (_added_nt, removed_nt) in graph_diffs:
                    if not removed_nt:
                        continue
                    audited_graph = True
                    audit.record(
                        session,
                        ks_id=ks.id,
                        action="ontology.reset",
                        summary=f"Reset extracted {layer} for clean re-extraction",
                        actor_id=user.id,
                        actor_name=user.username,
                        detail={
                            "layer": layer,
                            "removed_rows": removed_rows,
                            "documents_reset": documents_reset,
                        },
                        removed=removed_nt,
                        graph=graph_iri,
                        group_id=group_id,
                        commit=False,
                    )
                if not audited_graph:
                    # A reset may only clear relational extraction state (for example,
                    # after an earlier partial cleanup). Keep that non-no-op visible in
                    # history without fabricating an RDF diff.
                    audit.record(
                        session,
                        ks_id=ks.id,
                        action="ontology.reset",
                        summary="Reset extracted metadata for clean re-extraction",
                        actor_id=user.id,
                        actor_name=user.username,
                        detail={
                            "layer": "metadata",
                            "removed_rows": removed_rows,
                            "documents_reset": documents_reset,
                        },
                        group_id=group_id,
                        commit=False,
                    )
                session.commit()
    except Exception:
        session.rollback()
        raise

    if changed:
        try:
            retrieval.invalidate(ks.graph_iri)
        except Exception:  # noqa: BLE001
            # Retrieval vectors are derived cache state and can be rebuilt lazily.
            logger.exception("Failed to invalidate retrieval cache after ontology reset")

    return {
        "removed_triples": removed_triples,
        "removed_rows": removed_rows,
        "documents_reset": documents_reset,
    }


@router.post("/{ks_id}/ontology/edit")
def edit_ontology(
    body: EditRequest,
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict:
    # Compatibility endpoint: execute the single edit through the same atomic,
    # revision-aware, structurally validated path as the modeling workbench.
    operation = body.model_dump()
    expected_revision = operation.pop("expected_revision", None)
    confirm_destructive = bool(operation.pop("confirm_destructive", False))
    # Legacy callers cannot send a revision, so bind the request to the current graph
    # before entering the executor.  The executor re-checks it while both write locks
    # are held, preserving optimistic concurrency without silently bypassing it.
    if expected_revision is None:
        expected_revision = workbench.ontology_revision(ks.graph_iri)
    response = change_ontology(
        ChangeSetRequest(
            operations=[operation],
            expected_revision=expected_revision,
            reason=_edit_summary(operation),
            confirm_destructive=confirm_destructive,
        ),
        ks,
        user,
        session,
    )
    return {
        "result": response["results"][0],
        "view": response["view"],
        "open_conflicts": response.get("open_conflicts", 0),
        "base_revision": response["base_revision"],
        "revision": response["revision"],
    }


@router.get("/{ks_id}/ontology/impact")
def ontology_impact(
    iri: str = Query(min_length=1),
    kind: str | None = Query(default=None),
    ks: KnowledgeSystem = Depends(ks_reader),
) -> dict:
    """Read-only deletion impact analysis for a class or property."""

    try:
        with store.read_lock(ks.graph_iri), store.read_lock(workbench.abox_iri_for(ks.graph_iri)):
            impact = workbench.analyze_entity_impact(ks.graph_iri, iri, kind)
            revision = workbench.ontology_revision(ks.graph_iri)
    except ValueError as exc:
        raise HTTPException(status_code=400, detail=str(exc)) from exc
    if not impact["exists"]:
        raise HTTPException(status_code=404, detail="Ontology entity not found")
    return {
        "revision": revision,
        "impact": workbench.public_impact(impact),
    }


@router.post("/{ks_id}/ontology/changes")
def change_ontology(
    body: ChangeSetRequest,
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict:
    """Preview or atomically apply a set of ontology edits.

    Preview applies edits inside dual TBox/ABox captures, builds the exact diff and
    structural-conflict report, and then explicitly reverts both graphs.  Commit uses
    the same execution path but records one grouped audit change.  Any failed operation
    exits the nested captures with an exception, rolling every preceding operation back.
    """

    try:
        operations = _operations(body.edits())
    except ValueError as exc:
        raise HTTPException(status_code=400, detail=str(exc)) from exc
    if extraction_active(session, ks.id):
        raise HTTPException(status_code=409, detail="An extraction is in progress; try again after it finishes.")
    if not body.dry_run and body.expected_revision is None:
        raise HTTPException(
            status_code=400,
            detail="expected_revision is required when committing ontology changes",
        )

    # Optimistic concurrency is re-checked below while both graph write locks are held.
    # Avoid an unlocked check here, which can only be advisory and creates a TOCTOU gap.
    base_revision = ""
    destructive = [operation["op"] for operation in operations if operation["op"] in _DESTRUCTIVE_OPS]
    if destructive and not body.dry_run and not body.confirm_destructive:
        raise HTTPException(
            status_code=400,
            detail="confirm_destructive=true is required for delete or merge operations",
        )

    abox_iri = workbench.abox_iri_for(ks.graph_iri)
    try:
        # Keep both graph locks until the SQL transaction commits.  That closes the
        # revision-check/apply race and lets capture revert RDF automatically if an
        # audit, provenance, conflict, or metadata write fails.
        with store.capture(ks.graph_iri, revert_on_error=True) as cap, store.capture(
            abox_iri, revert_on_error=True,
        ) as acap:
            locked_revision = workbench.ontology_revision(ks.graph_iri)
            if body.expected_revision is not None and body.expected_revision != locked_revision:
                raise _revision_conflict(body.expected_revision, locked_revision)
            base_revision = locked_revision
            baseline_detected = conflict_detector.detect(ks.graph_iri, semantic=False)
            baseline_abox = abox_validate.validate(ks.graph_iri, abox_iri)
            impact_details = []
            for index, operation in enumerate(operations):
                impact_details.extend(_impact_for_operation(ks.graph_iri, index, operation))
            results = [editor.apply_edit(ks.graph_iri, ks.base_iri, operation) for operation in operations]
            added, removed = cap.diff()
            abox_added, abox_removed = acap.diff()
            resulting_view = schema.build_view(ks.graph_iri)
            resulting_revision = workbench.ontology_revision(ks.graph_iri)
            detected = conflict_detector.detect(ks.graph_iri, semantic=False)
            resulting_abox = abox_validate.validate(ks.graph_iri, abox_iri)
            baseline_errors = {
                f"tbox:{item.signature}" for item in baseline_detected if item.severity == "error"
            }
            baseline_errors.update(
                f"abox:{item['id']}"
                for item in baseline_abox["violations"]
                if item["severity"] == "error"
            )
            resulting_errors = {
                f"tbox:{item.signature}" for item in detected if item.severity == "error"
            }
            resulting_errors.update(
                f"abox:{item['id']}"
                for item in resulting_abox["violations"]
                if item["severity"] == "error"
            )
            new_errors = resulting_errors - baseline_errors
            resolved_errors = baseline_errors - resulting_errors

            impact = workbench.batch_impact(operations, impact_details)
            # Exact net counts include anonymous union/list cleanup and de-duplicate
            # overlap between several destructive operations in one batch.
            impact["totals"]["tbox_triples"] = len(store.load_triples(removed))
            impact["totals"]["abox_assertions"] = len(store.load_triples(abox_removed))
            response = {
                "dry_run": body.dry_run,
                "applied": 0 if body.dry_run else len(operations),
                "operations": operations,
                "results": results,
                "destructive_operations": destructive,
                "requires_confirmation": bool(destructive),
                "base_revision": base_revision,
                "revision": resulting_revision,
                "diff": _diff_payload(
                    added,
                    removed,
                    abox_added,
                    abox_removed,
                    include_rdf_diff=body.include_rdf_diff,
                ),
                "impact": impact,
                "conflicts": _serialize_conflicts(detected),
                "abox_validation": resulting_abox,
                "structural_validation": {
                    "valid": not resulting_errors,
                    "committable": not new_errors,
                    "error_count": len(resulting_errors),
                    "warning_count": (
                        sum(item.severity != "error" for item in detected)
                        + resulting_abox["counts"]["warning"]
                    ),
                    "new_error_count": len(new_errors),
                    "resolved_error_count": len(resolved_errors),
                    "new_error_signatures": sorted(new_errors),
                },
                "resulting_stats": resulting_view["stats"],
                "view": resulting_view,
            }
            if body.dry_run:
                # Successful previews must still leave both graphs untouched.
                acap.revert()
                cap.revert()
                return response

            # Existing errors may be repaired incrementally, but a change set must never
            # introduce a new error-level structural contradiction. Enforce this on the
            # server as well as in the UI so direct API clients cannot bypass preflight.
            if new_errors:
                raise HTTPException(
                    status_code=422,
                    detail={
                        "code": "ontology_structural_validation_failed",
                        "message": "The change set introduces structural ontology errors.",
                        "new_error_count": len(new_errors),
                        "new_error_signatures": sorted(new_errors),
                    },
                )

            if not (added or removed or abox_added or abox_removed):
                # A valid idempotent operation may resolve to the graph's existing
                # state.  It is not a committed change: do not fabricate history,
                # provenance, stats writes, or conflict transitions for it.
                response.update({
                    "applied": 0,
                    "no_op": True,
                    "open_conflicts": session.exec(
                        select(func.count(Conflict.id)).where(
                            Conflict.knowledge_system_id == ks.id,
                            Conflict.status == "open",
                        )
                    ).one(),
                })
                return response

            refresh_ks_stats(session, ks, commit=False)
            # A preview calls the detector directly and never writes SQL conflict rows;
            # a commit reconciles them within this same SQL transaction.
            open_conflicts = sync_conflicts(session, ks, semantic=False, commit=False)
            group_id = secrets.token_hex(8)
            reason = (body.reason or body.summary).strip()
            summary = reason or (
                _edit_summary(operations[0])
                if len(operations) == 1
                else f"Applied {len(operations)} ontology edits"
            )
            event = audit.record(
                session,
                ks_id=ks.id,
                action="ontology.change_set",
                summary=summary,
                actor_id=user.id,
                actor_name=user.username,
                detail={"reason": reason, "operations": operations, "base_revision": base_revision},
                added=added,
                removed=removed,
                group_id=group_id,
                commit=False,
            )
            statement_provenance.record_tbox_diff(
                session, ks.id, added, removed, event, commit=False,
            )
            if abox_added or abox_removed:
                abox_event = audit.record(
                    session,
                    ks_id=ks.id,
                    action="ontology.change_set",
                    summary=f"{summary} — cascaded to instances",
                    actor_id=user.id,
                    actor_name=user.username,
                    detail={"reason": reason, "operations": operations, "base_revision": base_revision},
                    added=abox_added,
                    removed=abox_removed,
                    graph=abox_iri,
                    group_id=group_id,
                    commit=False,
                )
                statement_provenance.record_abox_diff(
                    session,
                    ks.id,
                    abox_added,
                    abox_removed,
                    abox_event,
                    abox_iri=abox_iri,
                    operations=operations,
                    results=results,
                    commit=False,
                )
            session.commit()
            response.update({
                "audit_event_id": event.id,
                "open_conflicts": len(open_conflicts),
            })
    except editor.EditError as exc:
        session.rollback()
        raise HTTPException(status_code=400, detail=str(exc)) from exc
    except (KeyError, TypeError, ValueError) as exc:
        session.rollback()
        raise HTTPException(status_code=400, detail=f"Change set failed: {exc}") from exc
    except HTTPException:
        session.rollback()
        raise
    except Exception:
        session.rollback()
        raise

    # SQL and both RDF graphs are now durably aligned and the compensating captures
    # have closed successfully.  Cache eviction is deliberately best-effort: a cache
    # implementation failure must never turn a committed SQL transaction into an RDF
    # rollback (or report the durable change as failed to the client).
    try:
        retrieval.invalidate(ks.graph_iri)
    except Exception:  # noqa: BLE001
        logger.exception("ontology retrieval cache invalidation failed for %s", ks.graph_iri)
    return response


@router.post("/{ks_id}/ontology/suggest")
def suggest_ontology_changes(
    body: SuggestOntologyRequest,
    ks: KnowledgeSystem = Depends(ks_writer),
    session: Session = Depends(get_session),
) -> dict:
    """Generate and structurally preview an ontology change set; never apply it."""

    instruction = body.instruction.strip()
    if not instruction:
        raise HTTPException(status_code=400, detail="instruction is required")
    if len(instruction) > 20_000:
        raise HTTPException(status_code=400, detail="instruction cannot exceed 20,000 characters")
    if extraction_active(session, ks.id):
        raise HTTPException(status_code=409, detail="An extraction is in progress; try again after it finishes.")
    revision = workbench.ontology_revision(ks.graph_iri)
    if body.expected_revision is not None and body.expected_revision != revision:
        raise _revision_conflict(body.expected_revision, revision)

    try:
        with model_config.use_ks_connections(session, ks), prompt_config.use_ks_prompts(session, ks.id):
            suggestion = modeling_assistant.suggest(ks.graph_iri, instruction)
        # Use the exact same atomic preview executor as a human-authored change set.
        preview = change_ontology(
            ChangeSetRequest(
                operations=suggestion["operations"],
                expected_revision=revision,
                dry_run=True,
                reason=suggestion["reason"],
            ),
            ks,
            # Preview never reads the actor, but the signature remains one shared path.
            User(id=0, username="modeling-assistant", password_hash=""),
            session,
        )
    except modeling_assistant.SuggestionError as exc:
        raise HTTPException(status_code=422, detail=f"Model suggestion is invalid: {exc}") from exc
    except HTTPException:
        raise
    except Exception as exc:  # noqa: BLE001
        raise HTTPException(status_code=502, detail=f"Modeling assistant failed: {exc}") from exc
    return {
        "summary": suggestion["summary"],
        "reason": suggestion["reason"],
        "operations": suggestion["operations"],
        "revision": revision,
        "preview": preview,
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
