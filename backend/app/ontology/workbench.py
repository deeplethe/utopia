"""Shared primitives for the interactive ontology editing workbench.

The workbench needs two pieces of information that are deliberately independent of
SQL history rows:

* a content-addressed revision used for optimistic concurrency; and
* a read-only impact report for destructive ontology operations.

Both are derived directly from the TBox and its paired ABox.  This keeps revisions
stable across process restarts and triple iteration order, and makes a preview safe to
run without creating conflict/audit records.
"""
from __future__ import annotations

import hashlib
from typing import Iterable

from pyoxigraph import NamedNode

from app.ontology import abox_validate, conflicts, schema, store
from app.ontology.vocab import (
    OWL_DATATYPE_PROPERTY,
    OWL_NAMED_INDIVIDUAL,
    OWL_OBJECT_PROPERTY,
    RDF_FIRST,
    RDF_TYPE,
    RDFS_DOMAIN,
    RDFS_COMMENT,
    RDFS_LABEL,
    RDFS_RANGE,
    RDFS_SUBCLASSOF,
)


def abox_iri_for(graph_iri: str) -> str:
    return graph_iri.rstrip("/") + "/abox"


def _canonical_lines(graph_iri: str) -> list[bytes]:
    """Return an order-independent representation of the graph's current triples.

    ``dump_triples`` emits one complete N-Triples statement.  Sorting those statements
    removes Oxigraph iteration order from the digest while preserving RDF term types,
    language tags, datatypes, and blank-node identities.
    """

    return sorted(store.dump_triples([triple]).rstrip(b"\r\n") for triple in store.read_triples(graph_iri))


def ontology_revision(graph_iri: str) -> str:
    """Content revision for the editable TBox plus its cascade-dependent ABox.

    ABox content is included because deleting a class/property can remove or re-type
    individuals.  A client that previewed such a deletion must not silently apply it
    after another user has added affected assertions.
    """

    digest = hashlib.sha256()
    for layer, current_iri in ((b"tbox", graph_iri), (b"abox", abox_iri_for(graph_iri))):
        digest.update(layer + b"\0")
        for line in _canonical_lines(current_iri):
            digest.update(len(line).to_bytes(8, "big"))
            digest.update(line)
    return "sha256:" + digest.hexdigest()


def structural_error_signatures(graph_iri: str) -> set[str]:
    """Return stable identifiers for every current error-level TBox/ABox issue.

    Mutation paths compare this set before and after a write.  Existing errors do not
    prevent incremental repairs, while any newly introduced error blocks the whole
    operation.  Callers that mutate RDF should invoke this while holding the paired
    TBox/ABox graph locks so the comparison is one consistent snapshot.
    """

    detected = conflicts.detect(graph_iri, semantic=False)
    validation = abox_validate.validate(graph_iri, abox_iri_for(graph_iri))
    errors = {
        f"tbox:{item.signature}"
        for item in detected
        if item.severity == "error"
    }
    errors.update(
        f"abox:{item['id']}"
        for item in validation["violations"]
        if item["severity"] == "error"
    )
    return errors


def new_structural_errors(graph_iri: str, baseline: set[str]) -> list[str]:
    """Return sorted error signatures introduced since ``baseline``."""

    return sorted(structural_error_signatures(graph_iri) - baseline)


def _sample(values: Iterable[str], limit: int = 100) -> tuple[list[str], bool]:
    items = sorted(set(values))
    return items[:limit], len(items) > limit


def _entity_tbox_triples(graph_iri: str, iri: str) -> list[tuple]:
    return [
        triple
        for triple in store.read_triples(graph_iri)
        if triple[0].value == iri or (isinstance(triple[2], NamedNode) and triple[2].value == iri)
    ]


def _class_impact(graph_iri: str, iri: str) -> dict:
    abox_iri = abox_iri_for(graph_iri)
    tbox = store.read_triples(graph_iri)
    abox = store.read_triples(abox_iri)
    view = schema.build_view(graph_iri)
    class_by_iri = {item["iri"]: item for item in view["classes"]}
    exists = iri in class_by_iri

    subclasses = [
        s.value for s, predicate, obj in tbox
        if predicate.value == RDFS_SUBCLASSOF.value
        and isinstance(obj, NamedNode) and obj.value == iri
    ]
    superclasses = [
        obj.value for s, predicate, obj in tbox
        if s.value == iri and predicate.value == RDFS_SUBCLASSOF.value and isinstance(obj, NamedNode)
    ]
    property_uses: list[dict] = []
    for kind, key in (("object", "object_properties"), ("data", "data_properties")):
        for prop in view[key]:
            slots = []
            if iri in prop.get("domain_members", []):
                slots.append("domain")
            if iri in prop.get("range_members", []):
                slots.append("range")
            if slots:
                property_uses.append({
                    "iri": prop["iri"],
                    "label": prop.get("label") or prop["iri"],
                    "kind": kind,
                    "slots": slots,
                })

    types_by_individual: dict[str, set[str]] = {}
    for subject, predicate, obj in abox:
        if predicate.value == RDF_TYPE.value and isinstance(obj, NamedNode):
            types_by_individual.setdefault(subject.value, set()).add(obj.value)
    typed_affected = {subject for subject, types in types_by_individual.items() if iri in types}
    deleted = {
        subject
        for subject in typed_affected
        if not (types_by_individual[subject] - {iri, OWL_NAMED_INDIVIDUAL.value})
    }
    retyped = typed_affected - deleted

    # Exact ABox removals performed by editor._cascade_class_delete: multi-typed
    # individuals lose only this rdf:type; single-typed individuals are removed as
    # entities, including assertions in which they occur as an object.
    removed_abox = {
        (str(subject), predicate.value, str(obj))
        for subject, predicate, obj in abox
        if (
            subject.value in deleted
            or (isinstance(obj, NamedNode) and obj.value in deleted)
            or (
                subject.value in retyped
                and predicate.value == RDF_TYPE.value
                and isinstance(obj, NamedNode)
                and obj.value == iri
            )
        )
    }
    # Impact includes every individual whose assertions change, including a subject
    # that merely points at an individual deleted by the class cascade.
    affected = set(typed_affected)
    for subject, predicate, obj in abox:
        if predicate.value != RDF_TYPE.value and (
            subject.value in deleted or (isinstance(obj, NamedNode) and obj.value in deleted)
        ):
            affected.add(subject.value)
            if isinstance(obj, NamedNode) and obj.value in deleted:
                affected.add(obj.value)
    tbox_entity = _entity_tbox_triples(graph_iri, iri)
    references = [
        store.dump_triples([triple]).decode("utf-8").strip()
        for triple in tbox_entity
        if triple[1].value not in {RDF_TYPE.value, RDFS_LABEL.value, RDFS_COMMENT.value}
    ]
    affected_sample, affected_truncated = _sample(affected)
    deleted_sample, deleted_truncated = _sample(deleted)
    retyped_sample, retyped_truncated = _sample(retyped)
    return {
        "kind": "class",
        "entity_iri": iri,
        "label": class_by_iri.get(iri, {}).get("label") or schema._local(iri),
        "exists": exists,
        "tbox_triples": len(tbox_entity),
        "referencing_axioms": len(references),
        "references": references[:100],
        "references_truncated": len(references) > 100,
        "subclasses": sorted(set(subclasses)),
        "superclasses": sorted(set(superclasses)),
        "properties_using_class": property_uses,
        "abox_type_assertions": len(typed_affected),
        "abox_property_assertions": sum(
            predicate != RDF_TYPE.value for _, predicate, _ in removed_abox
        ),
        "abox_assertions": len(removed_abox),
        "affected_individuals": affected_sample,
        "affected_individual_count": len(affected),
        "affected_individuals_truncated": affected_truncated,
        "individuals_deleted": deleted_sample,
        "individuals_deleted_count": len(deleted),
        "individuals_deleted_truncated": deleted_truncated,
        "individuals_retyped": retyped_sample,
        "individuals_retyped_count": len(retyped),
        "individuals_retyped_truncated": retyped_truncated,
        # Private full sets are used to produce unique batch totals and stripped by
        # ``public_impact`` before a response is returned.
        "_affected_individual_iris": affected,
        "_deleted_individual_iris": deleted,
        "_retyped_individual_iris": retyped,
    }


def _property_impact(graph_iri: str, iri: str) -> dict:
    view = schema.build_view(graph_iri)
    props = {
        item["iri"]: ("object", item)
        for item in view["object_properties"]
    } | {
        item["iri"]: ("data", item)
        for item in view["data_properties"]
    }
    kind, item = props.get(iri, ("property", {}))
    tbox_entity = _entity_tbox_triples(graph_iri, iri)
    assertions = [
        triple for triple in store.read_triples(abox_iri_for(graph_iri)) if triple[1].value == iri
    ]
    affected = {triple[0].value for triple in assertions}
    affected.update(
        triple[2].value for triple in assertions if isinstance(triple[2], NamedNode)
    )
    affected_sample, affected_truncated = _sample(affected)
    references = [
        store.dump_triples([triple]).decode("utf-8").strip()
        for triple in tbox_entity
        if triple[1].value not in {
            RDF_TYPE.value, RDFS_DOMAIN.value, RDFS_RANGE.value,
            RDFS_LABEL.value, RDFS_COMMENT.value,
        }
    ]
    return {
        "kind": "property",
        "property_kind": kind,
        "entity_iri": iri,
        "label": item.get("label") or schema._local(iri),
        "exists": iri in props,
        "tbox_triples": len(tbox_entity),
        "referencing_axioms": len(references),
        "references": references[:100],
        "references_truncated": len(references) > 100,
        "subclasses": [],
        "superclasses": [],
        "properties_using_class": [],
        "abox_type_assertions": 0,
        "abox_property_assertions": len(assertions),
        "abox_assertions": len(assertions),
        "affected_individuals": affected_sample,
        "affected_individual_count": len(affected),
        "affected_individuals_truncated": affected_truncated,
        "individuals_deleted": [],
        "individuals_deleted_count": 0,
        "individuals_deleted_truncated": False,
        "individuals_retyped": [],
        "individuals_retyped_count": 0,
        "individuals_retyped_truncated": False,
        "_affected_individual_iris": affected,
        "_deleted_individual_iris": set(),
        "_retyped_individual_iris": set(),
    }


def analyze_entity_impact(graph_iri: str, iri: str, kind: str | None = None) -> dict:
    """Analyze a class/property deletion without changing either graph."""

    if kind not in {None, "class", "property"}:
        raise ValueError("kind must be class or property")
    if kind is None:
        node = NamedNode(iri)
        if store.has_triple(graph_iri, node, RDF_TYPE, OWL_OBJECT_PROPERTY) or store.has_triple(
            graph_iri, node, RDF_TYPE, OWL_DATATYPE_PROPERTY
        ):
            kind = "property"
        else:
            kind = "class"
    return _class_impact(graph_iri, iri) if kind == "class" else _property_impact(graph_iri, iri)


def public_impact(value: dict) -> dict:
    return {key: item for key, item in value.items() if not key.startswith("_")}


def batch_impact(operations: list[dict], per_operation: list[dict]) -> dict:
    """Return per-operation details and de-duplicated affected-entity totals."""

    affected: set[str] = set()
    deleted: set[str] = set()
    retyped: set[str] = set()
    totals = {
        "destructive_operations": 0,
        "tbox_triples": 0,
        "referencing_axioms": 0,
        "subclasses": 0,
        "properties_using_class": 0,
        "abox_type_assertions": 0,
        "abox_property_assertions": 0,
        "abox_assertions": 0,
        "affected_individuals": 0,
        "individuals_deleted": 0,
        "individuals_retyped": 0,
    }
    public_items = []
    destructive_indices: set[int] = set()
    for raw in per_operation:
        if raw.get("destructive"):
            destructive_indices.add(raw["index"])
        for key in (
            "tbox_triples", "referencing_axioms", "abox_type_assertions",
            "abox_property_assertions", "abox_assertions",
        ):
            totals[key] += int(raw.get(key, 0))
        totals["subclasses"] += len(raw.get("subclasses", []))
        totals["properties_using_class"] += len(raw.get("properties_using_class", []))
        affected.update(raw.get("_affected_individual_iris", set()))
        deleted.update(raw.get("_deleted_individual_iris", set()))
        retyped.update(raw.get("_retyped_individual_iris", set()))
        public_items.append(public_impact(raw))
    totals["destructive_operations"] = len(destructive_indices)
    totals["affected_individuals"] = len(affected)
    totals["individuals_deleted"] = len(deleted)
    totals["individuals_retyped"] = len(retyped)
    return {"operations": public_items, "totals": totals}
