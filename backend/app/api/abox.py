"""ABox (instances) API: browse individuals by class, and manual CRUD of individuals and
their assertions. Instances live in a separate named graph per KS (``.../{id}/abox``); every
mutation is captured into the change history and is graph-scoped rollbackable.
"""
from __future__ import annotations

from fastapi import APIRouter, Depends, HTTPException, Query
from pydantic import BaseModel
from sqlalchemy import delete, func
from sqlmodel import Session, select

from app import audit
from app.api.knowledge import GRAPH_ROOT
from app.db.database import get_session
from app.db.models import AboxProvenance, EntityResolution, KnowledgeSystem, User, ValidationDecision
from app.permissions import extraction_active, ks_reader, ks_writer
from app.security import current_user
from app.ontology import (
    abox, abox_provenance, abox_validate, editor, schema, statement_provenance, store,
    validation_agent,
)

router = APIRouter(prefix="/api/knowledge", tags=["abox"])


def abox_iri_for(ks: KnowledgeSystem) -> str:
    # Derive from the KS id so it is stable regardless of graph_iri formatting.
    return f"{GRAPH_ROOT}/{ks.id}/abox"


def _labels(ks: KnowledgeSystem) -> tuple[dict[str, str], dict[str, str]]:
    """(class_iri -> label, property_iri -> label) from the TBox view."""
    view = schema.build_view(ks.graph_iri)
    class_labels = {c["iri"]: c.get("label") or c["iri"] for c in view["classes"]}
    prop_labels = {
        p["iri"]: p.get("label") or p["iri"]
        for p in view["object_properties"] + view["data_properties"]
    }
    return class_labels, prop_labels


def _guard(session: Session, ks: KnowledgeSystem) -> None:
    if extraction_active(session, ks.id):
        raise HTTPException(status_code=409, detail="An extraction is in progress; try again after it finishes.")


class ResetAboxRequest(BaseModel):
    confirm: bool = False


@router.post("/{ks_id}/abox/reset")
def reset_abox(
    body: ResetAboxRequest,
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict:
    _guard(session, ks)
    if not body.confirm:
        raise HTTPException(status_code=400, detail="confirm=true is required to reset all instances")
    abox_iri = abox_iri_for(ks)
    provenance_rows = session.exec(
        select(func.count(AboxProvenance.id)).where(AboxProvenance.knowledge_system_id == ks.id)
    ).one()
    resolution_rows = session.exec(
        select(func.count(EntityResolution.id)).where(EntityResolution.knowledge_system_id == ks.id)
    ).one()
    with store.capture(abox_iri, revert_on_error=True) as cap:
        store.clear_graph(abox_iri)
    added_nt, removed_nt = cap.diff()
    session.exec(delete(AboxProvenance).where(AboxProvenance.knowledge_system_id == ks.id))
    session.exec(delete(EntityResolution).where(EntityResolution.knowledge_system_id == ks.id))
    audit.record(
        session, ks_id=ks.id, action="abox.reset", summary="Reset all instances for re-extraction",
        actor_id=user.id, actor_name=user.username,
        detail={"provenance_rows": provenance_rows, "resolution_rows": resolution_rows},
        added=added_nt, removed=removed_nt, graph=abox_iri,
    )
    session.commit()
    return {
        "removed_triples": len(store.load_triples(removed_nt)) if removed_nt else 0,
        "provenance_rows": provenance_rows,
        "resolution_rows": resolution_rows,
    }


# --------------------------------------------------------------------------- #
# Browse
# --------------------------------------------------------------------------- #
@router.get("/{ks_id}/abox/classes")
def abox_classes(ks: KnowledgeSystem = Depends(ks_reader)) -> dict:
    """TBox classes annotated with how many individuals each has (browse sidebar)."""
    class_labels, _ = _labels(ks)
    counts = abox.counts_by_class(abox_iri_for(ks))
    classes = [
        {"iri": iri, "label": label, "count": counts.get(iri, 0)}
        for iri, label in class_labels.items()
    ]
    classes.sort(key=lambda c: (-c["count"], c["label"]))
    # Individuals whose type isn't a known class (shouldn't normally happen) still counted in total.
    return {"classes": classes, "total": sum(counts.values())}


@router.get("/{ks_id}/abox/individuals")
def list_individuals(
    class_iri: str | None = None,
    q: str | None = None,
    limit: int = Query(default=20, le=200),
    offset: int = 0,
    ks: KnowledgeSystem = Depends(ks_reader),
) -> dict:
    class_labels, _ = _labels(ks)
    items, total = abox.list_individuals(
        abox_iri_for(ks), class_labels, class_iri=class_iri, q=q, offset=offset, limit=limit,
    )
    return {"items": items, "total": total}


@router.get("/{ks_id}/abox/individual")
def get_individual(
    iri: str, ks: KnowledgeSystem = Depends(ks_reader), session: Session = Depends(get_session)
) -> dict:
    class_labels, prop_labels = _labels(ks)
    ind = abox.get_individual(abox_iri_for(ks), iri, class_labels, prop_labels)
    if ind is None:
        raise HTTPException(status_code=404, detail="Individual not found")
    # Attach provenance (source chunk/document + snippet) to the individual and each assertion.
    keys = [abox_provenance.ind_key(iri)]
    keys += [abox_provenance.data_key(iri, a["prop"], a["value"]) for a in ind["data_assertions"]]
    keys += [abox_provenance.obj_key(iri, a["prop"], a["target"]) for a in ind["object_assertions"]]
    srcs = abox_provenance.sources_for(session, ks.id, keys)
    ind["sources"] = srcs.get(abox_provenance.ind_key(iri), [])
    for a in ind["data_assertions"]:
        a["sources"] = srcs.get(abox_provenance.data_key(iri, a["prop"], a["value"]), [])
    for a in ind["object_assertions"]:
        a["sources"] = srcs.get(abox_provenance.obj_key(iri, a["prop"], a["target"]), [])
    return ind


# --------------------------------------------------------------------------- #
# Mutations
# --------------------------------------------------------------------------- #
class CreateIndividual(BaseModel):
    label: str
    class_iri: str


@router.post("/{ks_id}/abox/individuals")
def create_individual(
    body: CreateIndividual,
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict:
    _guard(session, ks)
    class_labels, prop_labels = _labels(ks)
    if body.class_iri not in class_labels:
        raise HTTPException(status_code=400, detail="Unknown class")
    abox_iri = abox_iri_for(ks)
    with store.capture(abox_iri, revert_on_error=True) as cap:
        iri = abox.create_individual(abox_iri, ks.base_iri, body.label, body.class_iri)
    added_nt, removed_nt = cap.diff()
    event = audit.record(
        session, ks_id=ks.id, action="abox.add_individual",
        summary=f'Added individual "{body.label}" ({class_labels[body.class_iri]})',
        actor_id=user.id, actor_name=user.username,
        detail={"iri": iri, "class_iri": body.class_iri, "label": body.label},
        added=added_nt, removed=removed_nt, graph=abox_iri,
    )
    statement_provenance.record_abox_fact(
        session, ks.id, abox_provenance.ind_key(iri), event,
    )
    return abox.get_individual(abox_iri, iri, class_labels, prop_labels)


class IndividualRef(BaseModel):
    iri: str


@router.post("/{ks_id}/abox/individuals/delete")
def delete_individual(
    body: IndividualRef,
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict:
    _guard(session, ks)
    class_labels, prop_labels = _labels(ks)
    abox_iri = abox_iri_for(ks)
    existing = abox.get_individual(abox_iri, body.iri, class_labels, prop_labels)
    if existing is None:
        raise HTTPException(status_code=404, detail="Individual not found")
    with store.capture(abox_iri, revert_on_error=True) as cap:
        removed = abox.delete_individual(abox_iri, body.iri)
    added_nt, removed_nt = cap.diff()
    fact_keys = {abox_provenance.ind_key(body.iri)}
    fact_keys.update(
        abox_provenance.data_key(body.iri, item["prop"], item["value"])
        for item in existing["data_assertions"]
    )
    fact_keys.update(
        abox_provenance.obj_key(body.iri, item["prop"], item["target"])
        for item in existing["object_assertions"]
    )
    audit.record(
        session, ks_id=ks.id, action="abox.delete_individual",
        summary=f'Deleted individual "{existing["label"]}"',
        actor_id=user.id, actor_name=user.username,
        detail={"iri": body.iri, "label": existing["label"], "triples_removed": removed},
        added=added_nt, removed=removed_nt, graph=abox_iri,
    )
    statement_provenance.remove_abox_facts(session, ks.id, fact_keys)
    return {"removed": removed}


class Assertion(BaseModel):
    subject: str
    prop: str
    kind: str  # "object" | "data"
    target: str | None = None            # object property: individual IRI
    value: str | None = None             # data property: literal value
    datatype: str | None = None          # data property: optional XSD datatype IRI


def _assert_summary(a: Assertion, prop_labels: dict[str, str], subj_label: str, verb: str) -> str:
    pl = prop_labels.get(a.prop, a.prop.rsplit("#", 1)[-1].rsplit("/", 1)[-1])
    if a.kind == "object":
        return f'{verb} "{subj_label}" —{pl}→ (individual)'
    return f'{verb} {pl} = "{a.value}" on "{subj_label}"'


def _apply_assertion(abox_iri: str, a: Assertion, remove: bool) -> None:
    if a.kind == "object":
        if not a.target:
            raise HTTPException(status_code=400, detail="Object assertion needs a target individual")
        (abox.remove_object_assertion if remove else abox.add_object_assertion)(
            abox_iri, a.subject, a.prop, a.target)
    elif a.kind == "data":
        if a.value is None:
            raise HTTPException(status_code=400, detail="Data assertion needs a value")
        (abox.remove_data_assertion if remove else abox.add_data_assertion)(
            abox_iri, a.subject, a.prop, a.value, a.datatype)
    else:
        raise HTTPException(status_code=400, detail="kind must be 'object' or 'data'")


@router.post("/{ks_id}/abox/assertions")
def add_assertion(
    body: Assertion,
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict:
    _guard(session, ks)
    class_labels, prop_labels = _labels(ks)
    abox_iri = abox_iri_for(ks)
    subj = abox.get_individual(abox_iri, body.subject, class_labels, prop_labels)
    if subj is None:
        raise HTTPException(status_code=404, detail="Subject individual not found")
    if body.prop not in prop_labels:
        raise HTTPException(status_code=400, detail="Unknown property for this knowledge system")
    if body.kind == "object" and body.target and not abox.exists(abox_iri, body.target):
        raise HTTPException(status_code=404, detail="Target individual not found")
    with store.capture(abox_iri, revert_on_error=True) as cap:
        _apply_assertion(abox_iri, body, remove=False)
    added_nt, removed_nt = cap.diff()
    event = audit.record(
        session, ks_id=ks.id, action="abox.add_assertion",
        summary=_assert_summary(body, prop_labels, subj["label"], "Asserted"),
        actor_id=user.id, actor_name=user.username, detail=body.model_dump(),
        added=added_nt, removed=removed_nt, graph=abox_iri,
    )
    statement_provenance.record_abox_fact(
        session,
        ks.id,
        statement_provenance.assertion_key(
            body.subject, body.prop, body.kind, body.target, body.value,
        ),
        event,
    )
    return abox.get_individual(abox_iri, body.subject, class_labels, prop_labels)


@router.post("/{ks_id}/abox/assertions/delete")
def remove_assertion(
    body: Assertion,
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict:
    _guard(session, ks)
    class_labels, prop_labels = _labels(ks)
    abox_iri = abox_iri_for(ks)
    subj = abox.get_individual(abox_iri, body.subject, class_labels, prop_labels)
    if subj is None:
        raise HTTPException(status_code=404, detail="Subject individual not found")
    with store.capture(abox_iri, revert_on_error=True) as cap:
        _apply_assertion(abox_iri, body, remove=True)
    added_nt, removed_nt = cap.diff()
    key = statement_provenance.assertion_key(
        body.subject, body.prop, body.kind, body.target, body.value,
    )
    audit.record(
        session, ks_id=ks.id, action="abox.remove_assertion",
        summary=_assert_summary(body, prop_labels, subj["label"], "Removed"),
        actor_id=user.id, actor_name=user.username, detail=body.model_dump(),
        added=added_nt, removed=removed_nt, graph=abox_iri,
    )
    statement_provenance.remove_abox_facts(session, ks.id, {key})
    return abox.get_individual(abox_iri, body.subject, class_labels, prop_labels)


# --------------------------------------------------------------------------- #
# Validation (lint individuals against the TBox's semantic constraints)
# --------------------------------------------------------------------------- #
@router.get("/{ks_id}/abox/validate")
def validate_abox(ks: KnowledgeSystem = Depends(ks_reader)) -> dict:
    return abox_validate.validate(ks.graph_iri, abox_iri_for(ks))


class FixRequest(BaseModel):
    op: dict
    summary: str = ""  # human-readable, for the history entry


@router.post("/{ks_id}/abox/validate/fix")
def fix_violation(
    body: FixRequest,
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict:
    _guard(session, ks)
    abox_iri = abox_iri_for(ks)
    op = body.op
    try:
        if op.get("kind") == "relax_range":
            # A schema fix — relax a numeric data property's range to text. Lands on the TBox graph.
            with store.capture(ks.graph_iri, revert_on_error=True) as cap:
                editor.apply_edit(ks.graph_iri, ks.base_iri,
                                  {"op": "update_property", "iri": op["prop"], "range": "string"})
            graph = ks.graph_iri
        else:
            with store.capture(abox_iri, revert_on_error=True) as cap:
                abox_validate.apply_fix(abox_iri, op)
            graph = abox_iri
    except (KeyError, ValueError) as e:
        raise HTTPException(status_code=400, detail=f"Invalid fix: {e}") from e
    added_nt, removed_nt = cap.diff()
    audit.record(
        session, ks_id=ks.id, action="abox.fix_violation",
        summary=body.summary or f"Fixed instance violation ({op.get('kind')})",
        actor_id=user.id, actor_name=user.username, detail=op,
        added=added_nt, removed=removed_nt, graph=graph,
    )
    if op.get("kind") == "relax_range":  # a human said this property is qualitative — remember it
        validation_agent.record_decision(
            session, ks.id, op["prop"], op.get("prop_label", ""), op.get("xsd"),
            "relax", "human relaxed the range to text", user.username,
        )
        session.commit()
    return abox_validate.validate(ks.graph_iri, abox_iri)


@router.get("/{ks_id}/validation/decisions")
def list_validation_decisions(
    q: str | None = None,
    limit: int = Query(default=50, le=1000),
    offset: int = 0,
    ks: KnowledgeSystem = Depends(ks_reader),
    session: Session = Depends(get_session),
) -> dict:
    """The learned validation memory — per data property, the fix the agent will reuse (relax to
    text / remove noise), by agent or human."""
    conds = [ValidationDecision.knowledge_system_id == ks.id]
    if q and q.strip():
        conds.append(ValidationDecision.property_label.ilike(f"%{q.strip()}%"))
    total = session.exec(select(func.count(ValidationDecision.id)).where(*conds)).one()
    rows = session.exec(
        select(ValidationDecision).where(*conds)
        .order_by(ValidationDecision.id.desc()).limit(limit).offset(offset)
    ).all()
    items = [{
        "id": r.id, "property_label": r.property_label, "property_iri": r.property_iri,
        "xsd_type": r.xsd_type, "action": r.action, "reason": r.reason,
        "resolved_by": r.resolved_by, "created_at": r.created_at.isoformat(),
    } for r in rows]
    return {"items": items, "total": total}


@router.delete("/{ks_id}/validation/decisions/{did}")
def revoke_validation_decision(
    did: int,
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict:
    """Forget a validation fix decision so the agent re-judges that property next time (the schema
    change it already made is not undone — use History rollback for that)."""
    row = session.get(ValidationDecision, did)
    if not row or row.knowledge_system_id != ks.id:
        raise HTTPException(status_code=404, detail="Decision not found")
    label = row.property_label
    session.delete(row)
    audit.record(
        session, ks_id=ks.id, action="validation.revoke",
        summary=f'Forgot validation memory for "{label}"',
        actor_id=user.id, actor_name=user.username, detail={"property": label},
    )
    session.commit()
    return {"revoked": did}
