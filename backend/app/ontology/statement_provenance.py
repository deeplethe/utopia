"""Helpers for statement-level provenance created outside model extraction."""
from __future__ import annotations

import hashlib
import json

from pyoxigraph import Literal, NamedNode
from sqlmodel import Session, select

from app.db.models import AboxProvenance, AuditEvent, AxiomProvenance
from app.ontology import abox_provenance, store
from app.ontology.vocab import (
    OWL_CLASS,
    OWL_DATATYPE_PROPERTY,
    OWL_DISJOINT_WITH,
    OWL_EQUIVALENT_CLASS,
    OWL_NAMED_INDIVIDUAL,
    OWL_OBJECT_PROPERTY,
    RDF_TYPE,
    RDFS_DOMAIN,
    RDFS_LABEL,
    RDFS_RANGE,
    RDFS_SUBCLASSOF,
)


def triple_key(subject, predicate, obj) -> str:
    line = store.dump_triples([(subject, predicate, obj)])
    return "triple|" + hashlib.sha256(line).hexdigest()


def _local(iri: str) -> str:
    return iri.rsplit("#", 1)[-1].rstrip("/").rsplit("/", 1)[-1]


def semantic_tbox_key(triple: tuple) -> str | None:
    """Return the extraction-era canonical key for a concrete TBox triple.

    Manual edits historically used a hash of the full N-Triples statement while
    extraction provenance uses semantic keys. Returning both representations from
    diff handling keeps existing databases compatible without a destructive migration.
    Labels/comments and RDF collection plumbing have no extraction provenance key.
    """

    subject, predicate, obj = triple
    if not isinstance(subject, NamedNode):
        return None
    subject_local = _local(subject.value)
    if predicate == RDF_TYPE and isinstance(obj, NamedNode):
        if obj == OWL_CLASS:
            return f"class|{subject_local}"
        if obj == OWL_OBJECT_PROPERTY:
            return f"objprop|{subject_local}"
        if obj == OWL_DATATYPE_PROPERTY:
            return f"dataprop|{subject_local}"
    if not isinstance(obj, NamedNode):
        return None
    object_local = _local(obj.value)
    if predicate == RDFS_SUBCLASSOF:
        return f"subClassOf|{subject_local}|{object_local}"
    if predicate == RDFS_DOMAIN:
        return f"domain|{subject_local}|{object_local}"
    if predicate == RDFS_RANGE:
        return f"range|{subject_local}|{object_local}"
    if predicate == OWL_DISJOINT_WITH:
        return "disjointWith|" + "|".join(sorted((subject_local, object_local)))
    if predicate == OWL_EQUIVALENT_CLASS:
        return "equivalentClass|" + "|".join(sorted((subject_local, object_local)))
    return None


def tbox_keys(triple: tuple) -> set[str]:
    keys = {triple_key(*triple)}
    if semantic := semantic_tbox_key(triple):
        keys.add(semantic)
    return keys


def record_tbox_diff(
    session: Session,
    ks_id: int,
    added_nt: bytes,
    removed_nt: bytes,
    event: AuditEvent,
    *,
    commit: bool = True,
) -> None:
    removed_keys = {
        key
        for triple in (store.load_triples(removed_nt) if removed_nt else [])
        for key in tbox_keys(triple)
    }
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
    if commit:
        session.commit()
    else:
        session.flush()


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


def _abox_fact_key(triple: tuple) -> str | None:
    """Return the public provenance key represented by one ABox triple.

    ``rdf:type`` is intentionally represented by the individual's ``ind|`` key in
    the existing provenance model, rather than as one key per class assertion.
    Labels are display metadata and likewise have no independent source key.
    """

    subject, predicate, obj = triple
    if not isinstance(subject, NamedNode):
        return None
    if predicate.value in {RDF_TYPE.value, RDFS_LABEL.value}:
        return None
    if isinstance(obj, NamedNode):
        return abox_provenance.obj_key(subject.value, predicate.value, obj.value)
    if isinstance(obj, Literal):
        return abox_provenance.data_key(subject.value, predicate.value, obj.value)
    return None


def _resolve_rewrite(value: str, rewrites: dict[str, str]) -> str:
    """Follow a batch's rewrite chain (A -> B -> C) without looping forever."""

    seen: set[str] = set()
    while value in rewrites and value not in seen:
        seen.add(value)
        value = rewrites[value]
    return value


def _change_set_rewrites(
    operations: list[dict] | None,
    results: list[object] | None,
) -> tuple[dict[str, str], dict[str, str]]:
    """Build predicate/object rewrites caused by merge operations in one batch."""

    predicate_rewrites: dict[str, str] = {}
    object_rewrites: dict[str, str] = {}
    for operation, result in zip(operations or [], results or []):
        target = str(result or "").strip()
        if not target:
            continue
        if operation.get("op") == "merge_properties":
            for source in operation.get("sources", []):
                if source:
                    predicate_rewrites[str(source)] = target
        elif operation.get("op") == "merge_classes" and operation.get("source"):
            object_rewrites[str(operation["source"])] = target
    return predicate_rewrites, object_rewrites


def _source_identity(row: AboxProvenance) -> tuple:
    """Identity of one source attribution, used only to collapse exact duplicates."""

    return (
        row.chunk_id,
        row.job_id,
        row.audit_event_id,
        row.method,
        row.actor_name,
        json.dumps(row.review_record or {}, ensure_ascii=False, sort_keys=True, default=str),
    )


_MIGRATIONS_KEY = "_ontopilot_migrations"


def _migration_records(row: AboxProvenance) -> list[dict]:
    value = (row.review_record or {}).get(_MIGRATIONS_KEY, [])
    return [dict(item) for item in value if isinstance(item, dict)] if isinstance(value, list) else []


def _set_migration_records(row: AboxProvenance, records: list[dict]) -> None:
    review = dict(row.review_record or {})
    if records:
        review[_MIGRATIONS_KEY] = records
    else:
        review.pop(_MIGRATIONS_KEY, None)
    row.review_record = review


def _append_migration(
    row: AboxProvenance,
    *,
    event_id: int | None,
    source: str,
    target: str,
    mode: str,
) -> None:
    records = _migration_records(row)
    records.append({
        "event_id": event_id,
        "from": source,
        "to": target,
        "mode": mode,
    })
    _set_migration_records(row, records)


def _clone_abox_source(row: AboxProvenance, fact_key: str, records: list[dict]) -> AboxProvenance:
    clone = AboxProvenance(
        knowledge_system_id=row.knowledge_system_id,
        fact_key=fact_key,
        chunk_id=row.chunk_id,
        source_document_id=row.source_document_id,
        source_document_sha256=row.source_document_sha256,
        job_id=row.job_id,
        method=row.method,
        actor_name=row.actor_name,
        audit_event_id=row.audit_event_id,
        review_record=dict(row.review_record or {}),
    )
    _set_migration_records(clone, records)
    return clone


def _reverse_abox_migrations(
    session: Session,
    ks_id: int,
    event_ids: set[int],
) -> None:
    """Restore source attribution moved by change sets being rolled back.

    A marker is stored on the migrated source row when the forward merge happens.
    That makes provenance reversible even when the rewritten target assertion already
    existed and therefore did not appear in the graph diff. ``clone`` markers denote
    a source collapsed into an identical pre-existing target attribution; ``move``
    markers denote a row whose fact key itself was rewritten.
    """

    if not event_ids:
        return
    rows = list(session.exec(select(AboxProvenance).where(
        AboxProvenance.knowledge_system_id == ks_id,
    )).all())
    additions: list[AboxProvenance] = []
    for row in rows:
        records = _migration_records(row)
        if not records:
            # Provenance created by an undone mutation must not remain attached to
            # a fact which happened to survive the graph rollback.
            if row.audit_event_id in event_ids:
                session.delete(row)
            continue
        kept = list(records)
        for marker in reversed(records):
            try:
                marker_event_id = int(marker.get("event_id"))
            except (TypeError, ValueError):
                continue
            if marker_event_id not in event_ids:
                continue
            source = str(marker.get("from") or "")
            target = str(marker.get("to") or "")
            if not source or not target:
                continue
            marker_index = next(
                (index for index in range(len(kept) - 1, -1, -1) if kept[index] == marker),
                None,
            )
            if marker_index is not None:
                kept.pop(marker_index)
            if marker.get("mode") == "clone":
                additions.append(_clone_abox_source(row, source, kept))
            else:
                row.fact_key = source
        _set_migration_records(row, kept)
        session.add(row)
    session.add_all(additions)
    session.flush()


def record_abox_diff(
    session: Session,
    ks_id: int,
    added_nt: bytes,
    removed_nt: bytes,
    event: AuditEvent,
    *,
    abox_iri: str | None = None,
    operations: list[dict] | None = None,
    results: list[object] | None = None,
    reverse_event_ids: set[int] | None = None,
    commit: bool = True,
) -> None:
    """Synchronize ABox statement provenance with an exact RDF diff.

    Removed facts lose every source attribution.  For class/property merge change
    sets, attributions are moved to the rewritten fact (including the case where
    that target triple already existed, so it is absent from the net ``added``
    diff).  Exact duplicate source rows at the target are collapsed, while rows
    from distinct chunks/jobs/reviews remain distinct.  Every added or rewritten
    public fact also gets one manual attribution to ``event`` so the governance
    action itself remains traceable.

    Passing ``commit=False`` keeps all deletes, migrations, and additions inside
    the caller's SQL transaction.  ``abox_iri`` should be supplied for merge change
    sets so a source is migrated only when its rewritten target really survives the
    complete batch.
    """

    if reverse_event_ids:
        _reverse_abox_migrations(session, ks_id, reverse_event_ids)

    added_triples = store.load_triples(added_nt) if added_nt else []
    removed_triples = store.load_triples(removed_nt) if removed_nt else []
    predicate_rewrites, object_rewrites = _change_set_rewrites(operations, results)

    removed_keys: set[str] = set()
    added_keys = {key for triple in added_triples if (key := _abox_fact_key(triple))}
    touched_identity_subjects: set[str] = set()
    # An individual identity is one provenance fact even though its class assertions
    # may change independently.  Only adding/removing owl:NamedIndividual changes it.
    for subject, predicate, obj in removed_triples:
        key = _abox_fact_key((subject, predicate, obj))
        if key:
            removed_keys.add(key)
        if (
            isinstance(subject, NamedNode)
            and predicate.value == RDF_TYPE.value
            and obj == OWL_NAMED_INDIVIDUAL
        ):
            removed_keys.add(abox_provenance.ind_key(subject.value))
        elif isinstance(subject, NamedNode) and predicate.value in {
            RDF_TYPE.value, RDFS_LABEL.value,
        }:
            # The current public provenance model has one ``ind|`` fact rather
            # than separate keys for type/label metadata. Attribute a surviving
            # metadata change to that identity without deleting earlier sources.
            touched_identity_subjects.add(subject.value)
    for subject, predicate, obj in added_triples:
        if (
            isinstance(subject, NamedNode)
            and predicate.value == RDF_TYPE.value
            and obj == OWL_NAMED_INDIVIDUAL
        ):
            added_keys.add(abox_provenance.ind_key(subject.value))
        elif isinstance(subject, NamedNode) and predicate.value in {
            RDF_TYPE.value, RDFS_LABEL.value,
        }:
            touched_identity_subjects.add(subject.value)

    touched_identity_keys: set[str] = set()
    for subject_iri in touched_identity_subjects:
        identity_key = abox_provenance.ind_key(subject_iri)
        # Generic ABox imports are not required to declare owl:NamedIndividual.
        # Use the final subject existence when available so replacing/removing
        # such an individual cannot leave a dangling ``ind|`` attribution.
        if abox_iri is not None and not store.has_triple(
            abox_iri, NamedNode(subject_iri), None, None,
        ):
            removed_keys.add(identity_key)
        else:
            touched_identity_keys.add(identity_key)

    migrations: dict[str, str] = {}
    for subject, predicate, obj in removed_triples:
        old_key = _abox_fact_key((subject, predicate, obj))
        if old_key is None or not isinstance(subject, NamedNode):
            continue
        rewritten_predicate = _resolve_rewrite(predicate.value, predicate_rewrites)
        rewritten_obj = obj
        if isinstance(obj, NamedNode):
            rewritten_obj_iri = _resolve_rewrite(obj.value, object_rewrites)
            rewritten_obj = NamedNode(rewritten_obj_iri)
        if rewritten_predicate == predicate.value and rewritten_obj == obj:
            continue
        rewritten = (subject, NamedNode(rewritten_predicate), rewritten_obj)
        new_key = _abox_fact_key(rewritten)
        if new_key is None:
            continue
        # A later operation in the same batch may delete the rewritten assertion.
        # Never preserve provenance for a fact that does not exist in the final graph.
        if abox_iri is not None and not store.has_triple(abox_iri, *rewritten):
            continue
        if abox_iri is None and rewritten not in added_triples:
            continue
        migrations[old_key] = new_key

    rows = list(session.exec(select(AboxProvenance).where(
        AboxProvenance.knowledge_system_id == ks_id,
        AboxProvenance.fact_key.in_(removed_keys),
    )).all()) if removed_keys else []
    migration_targets = set(migrations.values())
    target_rows = list(session.exec(select(AboxProvenance).where(
        AboxProvenance.knowledge_system_id == ks_id,
        AboxProvenance.fact_key.in_(migration_targets),
    )).all()) if migration_targets else []
    identities_by_target: dict[str, set[tuple]] = {}
    rows_by_target_identity: dict[str, dict[tuple, AboxProvenance]] = {}
    for row in target_rows:
        identity = _source_identity(row)
        identities_by_target.setdefault(row.fact_key, set()).add(identity)
        rows_by_target_identity.setdefault(row.fact_key, {}).setdefault(identity, row)

    for row in rows:
        target_key = migrations.get(row.fact_key)
        if target_key is None:
            session.delete(row)
            continue
        identities = identities_by_target.setdefault(target_key, set())
        identity = _source_identity(row)
        if identity in identities:
            target_row = rows_by_target_identity[target_key][identity]
            _append_migration(
                target_row,
                event_id=event.id,
                source=row.fact_key,
                target=target_key,
                mode="clone",
            )
            session.add(target_row)
            session.delete(row)
            continue
        old_key = row.fact_key
        row.fact_key = target_key
        _append_migration(
            row,
            event_id=event.id,
            source=old_key,
            target=target_key,
            mode="move",
        )
        session.add(row)
        identities.add(identity)
        rows_by_target_identity.setdefault(target_key, {})[identity] = row

    # Record the governance action once for each resulting public fact.  Existing
    # extraction/review rows are deliberately retained as separate source evidence.
    # Do not create identity provenance for an individual deleted by this same
    # batch; otherwise a removed type triple would resurrect a dangling ``ind|``
    # row after the owl:NamedIndividual removal above.
    provenance_targets = (
        added_keys | migration_targets | touched_identity_keys
    ) - removed_keys
    if reverse_event_ids and provenance_targets:
        # A rollback of a merge can restore the original source attribution from a
        # migration marker before this diff is processed.  In that case the restored
        # fact is already fully sourced; adding a new chunk-less "manual" attribution
        # would create a provenance record that did not exist before the merge.
        restored_source_keys = {
            row.fact_key
            for row in session.exec(select(AboxProvenance).where(
                AboxProvenance.knowledge_system_id == ks_id,
                AboxProvenance.fact_key.in_(provenance_targets),
            )).all()
        }
        provenance_targets -= restored_source_keys
    existing_manual = {
        row.fact_key
        for row in session.exec(select(AboxProvenance).where(
            AboxProvenance.knowledge_system_id == ks_id,
            AboxProvenance.fact_key.in_(provenance_targets),
            AboxProvenance.audit_event_id == event.id,
        )).all()
    } if provenance_targets else set()
    review_record = {
        "action": event.action,
        "summary": event.summary,
        "detail": event.detail,
    }
    for fact_key in sorted(provenance_targets - existing_manual):
        session.add(AboxProvenance(
            knowledge_system_id=ks_id,
            fact_key=fact_key,
            method="manual",
            actor_name=event.actor_name,
            audit_event_id=event.id,
            review_record=review_record,
        ))

    if commit:
        session.commit()
    else:
        session.flush()


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
