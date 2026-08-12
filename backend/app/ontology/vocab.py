"""RDF/OWL/RDFS vocabulary terms and IRI/label helpers (pyoxigraph terms)."""
from __future__ import annotations

import re

from pyoxigraph import NamedNode

RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#"
RDFS = "http://www.w3.org/2000/01/rdf-schema#"
OWL = "http://www.w3.org/2002/07/owl#"
XSD = "http://www.w3.org/2001/XMLSchema#"

RDF_TYPE = NamedNode(RDF + "type")
RDF_FIRST = NamedNode(RDF + "first")
RDF_REST = NamedNode(RDF + "rest")
RDF_NIL = NamedNode(RDF + "nil")
RDFS_LABEL = NamedNode(RDFS + "label")
RDFS_COMMENT = NamedNode(RDFS + "comment")
RDFS_SUBCLASSOF = NamedNode(RDFS + "subClassOf")
RDFS_SUBPROPERTYOF = NamedNode(RDFS + "subPropertyOf")
RDFS_DOMAIN = NamedNode(RDFS + "domain")
RDFS_RANGE = NamedNode(RDFS + "range")

OWL_CLASS = NamedNode(OWL + "Class")
OWL_NAMED_INDIVIDUAL = NamedNode(OWL + "NamedIndividual")
OWL_OBJECT_PROPERTY = NamedNode(OWL + "ObjectProperty")
OWL_DATATYPE_PROPERTY = NamedNode(OWL + "DatatypeProperty")
OWL_DISJOINT_WITH = NamedNode(OWL + "disjointWith")
OWL_EQUIVALENT_CLASS = NamedNode(OWL + "equivalentClass")
OWL_UNION_OF = NamedNode(OWL + "unionOf")

XSD_STRING = NamedNode(XSD + "string")


# --------------------------------------------------------------------------- #
# Label normalization & local-name generation
# --------------------------------------------------------------------------- #
# Split on ASCII punctuation/space but KEEP non-ASCII letters (CJK, accented Latin, …),
# so non-Latin labels (e.g. Chinese "油田") get distinct, stable local names instead of
# collapsing to an empty/"Class" token.
_WORD_SPLIT = re.compile(r"[^0-9A-Za-z\u0080-\uffff]+")
_CAMEL_BOUNDARY = re.compile(r"(?<=[a-z0-9])(?=[A-Z])")


def _words(label: str) -> list[str]:
    """Split a label into words, honoring both separators and camelCase boundaries."""
    pieces: list[str] = []
    for token in _WORD_SPLIT.split(label.strip()):
        if not token:
            continue
        pieces.extend(w for w in _CAMEL_BOUNDARY.split(token) if w)
    return pieces


def norm_label(label: str) -> str:
    """Case/separator-insensitive key used for exact-duplicate detection."""
    return " ".join(w.lower() for w in _words(label))


def class_local_name(label: str) -> str:
    """PascalCase local name for a class IRI (also acts as dedup key)."""
    words = _words(label)
    if not words:
        return "Class"
    return "".join(w[:1].upper() + w[1:].lower() for w in words)


def property_local_name(label: str) -> str:
    """camelCase local name for a property IRI (also acts as dedup key)."""
    words = _words(label)
    if not words:
        return "property"
    head, *tail = words
    return head.lower() + "".join(w[:1].upper() + w[1:].lower() for w in tail)


def class_node(base_iri: str, label: str) -> NamedNode:
    return NamedNode(base_iri + class_local_name(label))


def property_node(base_iri: str, label: str) -> NamedNode:
    return NamedNode(base_iri + property_local_name(label))
