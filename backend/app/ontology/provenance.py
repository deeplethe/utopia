"""Provenance-driven ontology retraction.

When a source document is deleted, the user may choose to retract the axioms that came
*solely* from it. An axiom is identified by its canonical ``axiom_key`` (the same key
stored in ``AxiomProvenance``). This module turns those keys back into the triples to
remove, and safely garbage-collects entity declarations that become unreferenced.
"""
from __future__ import annotations

from typing import Callable

from pyoxigraph import NamedNode

from app.ontology import store
from app.ontology.vocab import (
    OWL_DISJOINT_WITH,
    OWL_EQUIVALENT_CLASS,
    RDF_TYPE,
    RDFS_COMMENT,
    RDFS_DOMAIN,
    RDFS_LABEL,
    RDFS_RANGE,
    RDFS_SUBCLASSOF,
    XSD,
)


def describe_axiom(key: str, label: Callable[[str], str]) -> str:
    """Human-readable description of an axiom_key, using a local-name -> label lookup."""
    parts = key.split("|")
    t = parts[0]
    if t == "class":
        return f'Class "{label(parts[1])}"'
    if t == "objprop":
        return f'Object property "{label(parts[1])}"'
    if t == "dataprop":
        return f'Data property "{label(parts[1])}"'
    if t == "subClassOf" and len(parts) == 3:
        return f"{label(parts[1])} ⊑ {label(parts[2])}"
    if t == "domain" and len(parts) == 3:
        return f"Domain of {label(parts[1])} = {label(parts[2])}"
    if t == "range" and len(parts) == 3:
        rng = parts[2]
        return f"Range of {label(parts[1])} = {('xsd:' + rng) if rng.islower() else label(rng)}"
    if t == "disjointWith" and len(parts) == 3:
        return f"{label(parts[1])} ⟂ {label(parts[2])}"
    if t == "equivalentClass" and len(parts) == 3:
        return f"{label(parts[1])} ≡ {label(parts[2])}"
    return key


_DECL_PREDS = {RDF_TYPE.value, RDFS_LABEL.value, RDFS_COMMENT.value}


def _is_referenced(graph_iri: str, node: NamedNode) -> bool:
    """True if the node participates in any relation (i.e. is used beyond its own
    declaration): as a subject of a non-declaration predicate, or as any object."""
    for s, p, o in store.read_triples(graph_iri):
        if s.value == node.value and p.value not in _DECL_PREDS:
            return True
        if getattr(o, "value", None) == node.value and s.value != node.value:
            return True
    return False


def retract_axioms(graph_iri: str, base_iri: str, keys: list[str]) -> None:
    """Remove the triples backing each axiom_key; GC chosen entity declarations that end
    up unreferenced (so no dangling class/property is left behind, and none in use is
    accidentally removed)."""
    def n(local: str) -> NamedNode:
        return NamedNode(base_iri + local)

    entity_locals: list[str] = []
    for key in keys:
        p = key.split("|")
        t = p[0]
        if t in ("class", "objprop", "dataprop"):
            entity_locals.append(p[1])
        elif t == "subClassOf" and len(p) == 3:
            store.remove_pattern(graph_iri, n(p[1]), RDFS_SUBCLASSOF, n(p[2]))
        elif t == "domain" and len(p) == 3:
            store.remove_pattern(graph_iri, n(p[1]), RDFS_DOMAIN, n(p[2]))
        elif t == "range" and len(p) == 3:
            # object-property range (a class) or data-property range (an XSD datatype)
            store.remove_pattern(graph_iri, n(p[1]), RDFS_RANGE, n(p[2]))
            store.remove_pattern(graph_iri, n(p[1]), RDFS_RANGE, NamedNode(XSD + p[2]))
        elif t == "disjointWith" and len(p) == 3:
            a, b = n(p[1]), n(p[2])
            store.remove_pattern(graph_iri, a, OWL_DISJOINT_WITH, b)
            store.remove_pattern(graph_iri, b, OWL_DISJOINT_WITH, a)
        elif t == "equivalentClass" and len(p) == 3:
            a, b = n(p[1]), n(p[2])
            store.remove_pattern(graph_iri, a, OWL_EQUIVALENT_CLASS, b)
            store.remove_pattern(graph_iri, b, OWL_EQUIVALENT_CLASS, a)

    # Remove chosen class/property declarations only if nothing references them anymore.
    for local in entity_locals:
        node = n(local)
        if not _is_referenced(graph_iri, node):
            store.remove_entity(graph_iri, node.value)
