"""Parse RDF files and split them into ontology (TBox) and instance (ABox) triples."""
from __future__ import annotations

from collections import defaultdict, deque
from dataclasses import dataclass
from pathlib import Path
from uuid import uuid4

from pyoxigraph import BlankNode, NamedNode, RdfFormat, Triple as RdfStarTriple, parse

from app.ontology import store, vocab


class RdfImportError(ValueError):
    """Raised when an RDF payload cannot be safely imported."""


@dataclass(frozen=True)
class ParsedRdf:
    format: str
    triples: list[store.Triple]


@dataclass(frozen=True)
class RdfPartition:
    tbox: list[store.Triple]
    abox: list[store.Triple]


RDF_FORMATS: dict[str, RdfFormat] = {
    "turtle": RdfFormat.TURTLE,
    "rdfxml": RdfFormat.RDF_XML,
    "ntriples": RdfFormat.N_TRIPLES,
    "jsonld": RdfFormat.JSON_LD,
}

_FORMAT_ALIASES = {
    "ttl": "turtle",
    "turtle": "turtle",
    "rdf": "rdfxml",
    "rdf/xml": "rdfxml",
    "rdfxml": "rdfxml",
    "xml": "rdfxml",
    "nt": "ntriples",
    "n-triples": "ntriples",
    "ntriples": "ntriples",
    "json": "jsonld",
    "json-ld": "jsonld",
    "jsonld": "jsonld",
}

_EXTENSION_FORMATS = {
    ".ttl": "turtle",
    ".rdf": "rdfxml",
    ".xml": "rdfxml",
    ".nt": "ntriples",
    ".jsonld": "jsonld",
    ".json": "jsonld",
}


def normalize_format(value: str) -> str:
    key = value.strip().lower()
    if key == "auto":
        return key
    normalized = _FORMAT_ALIASES.get(key)
    if not normalized:
        raise RdfImportError(f"Unsupported RDF format: {value}")
    return normalized


def _sniff_format(data: bytes) -> str:
    head = data.lstrip()[:2048].lower()
    if head.startswith((b"{", b"[")):
        return "jsonld"
    if head.startswith(b"<?xml") or b"<rdf:rdf" in head:
        return "rdfxml"
    if head.startswith(b"@prefix") or head.startswith(b"prefix ") or b"@prefix " in head:
        return "turtle"
    return "ntriples" if head.startswith(b"<") else "turtle"


def _candidate_formats(data: bytes, filename: str, requested: str) -> list[str]:
    requested = normalize_format(requested)
    if requested != "auto":
        return [requested]

    extension = Path(filename or "").suffix.lower()
    first = _EXTENSION_FORMATS.get(extension)
    if extension == ".owl" or first is None:
        first = _sniff_format(data)
    return [first, *(fmt for fmt in RDF_FORMATS if fmt != first)]


def _scope_blank_nodes(triples, scope: str) -> list[store.Triple]:
    blank_nodes: dict[BlankNode, BlankNode] = {}

    def scoped(term):
        if isinstance(term, RdfStarTriple):
            return RdfStarTriple(scoped(term.subject), term.predicate, scoped(term.object))
        if not isinstance(term, BlankNode):
            return term
        if term not in blank_nodes:
            blank_nodes[term] = BlankNode(f"rdfimport_{scope}_{len(blank_nodes)}")
        return blank_nodes[term]

    out: list[store.Triple] = []
    seen: set[store.Triple] = set()
    for quad in triples:
        triple = (scoped(quad.subject), quad.predicate, scoped(quad.object))
        if triple not in seen:
            seen.add(triple)
            out.append(triple)
    return out


def parse_rdf(
    data: bytes,
    *,
    filename: str = "",
    requested_format: str = "auto",
    base_iri: str | None = None,
    max_triples: int | None = None,
    blank_node_scope: str | None = None,
) -> ParsedRdf:
    """Parse one RDF document and scope its blank nodes to this import operation."""
    if not data.strip():
        raise RdfImportError("The RDF file is empty")

    errors: list[str] = []
    for fmt in _candidate_formats(data, filename, requested_format):
        try:
            kwargs = {"format": RDF_FORMATS[fmt]}
            if base_iri:
                kwargs["base_iri"] = base_iri
            parsed = []
            for index, quad in enumerate(parse(data, **kwargs), start=1):
                if max_triples is not None and index > max_triples:
                    raise RdfImportError(f"RDF file exceeds the {max_triples:,}-triple import limit")
                parsed.append(quad)
            triples = _scope_blank_nodes(parsed, blank_node_scope or uuid4().hex)
            return ParsedRdf(format=fmt, triples=triples)
        except RdfImportError:
            raise
        except Exception as exc:  # noqa: BLE001 - parser exception types vary by format
            errors.append(f"{fmt}: {exc}")
            if normalize_format(requested_format) != "auto":
                break

    detail = errors[0] if errors else "unknown parser error"
    raise RdfImportError(f"Could not parse RDF ({detail})")


def _iri(local: str, namespace: str = vocab.OWL) -> str:
    return namespace + local


_SCHEMA_TYPES = {
    _iri("Property", vocab.RDF),
    _iri("Class", vocab.RDFS),
    _iri("Datatype", vocab.RDFS),
    _iri("Class"),
    _iri("Restriction"),
    _iri("Ontology"),
    _iri("ObjectProperty"),
    _iri("DatatypeProperty"),
    _iri("AnnotationProperty"),
    _iri("OntologyProperty"),
    _iri("FunctionalProperty"),
    _iri("InverseFunctionalProperty"),
    _iri("TransitiveProperty"),
    _iri("SymmetricProperty"),
    _iri("AsymmetricProperty"),
    _iri("ReflexiveProperty"),
    _iri("IrreflexiveProperty"),
    _iri("DeprecatedClass"),
    _iri("DeprecatedProperty"),
    _iri("AllDisjointClasses"),
    _iri("AllDisjointProperties"),
    "http://www.w3.org/ns/shacl#NodeShape",
    "http://www.w3.org/ns/shacl#PropertyShape",
}

_CLASS_LINK_PREDICATES = {
    vocab.RDFS_SUBCLASSOF.value,
    vocab.RDFS_DOMAIN.value,
    vocab.RDFS_RANGE.value,
    _iri("equivalentClass"),
    _iri("disjointWith"),
    _iri("complementOf"),
    _iri("onClass"),
    _iri("onDataRange"),
    _iri("someValuesFrom"),
    _iri("allValuesFrom"),
    "http://www.w3.org/ns/shacl#class",
    "http://www.w3.org/ns/shacl#targetClass",
    "http://www.w3.org/ns/shacl#datatype",
}

_PROPERTY_LINK_PREDICATES = {
    vocab.RDFS_SUBPROPERTYOF.value,
    _iri("equivalentProperty"),
    _iri("propertyDisjointWith"),
    _iri("inverseOf"),
    _iri("onProperty"),
    "http://www.w3.org/ns/shacl#path",
}

_SCHEMA_SUBJECT_PREDICATES = {
    *_CLASS_LINK_PREDICATES,
    *_PROPERTY_LINK_PREDICATES,
    _iri("imports"),
    _iri("versionIRI"),
    _iri("priorVersion"),
    _iri("backwardCompatibleWith"),
    _iri("incompatibleWith"),
    _iri("versionInfo"),
    _iri("hasValue"),
    _iri("hasSelf"),
    _iri("cardinality"),
    _iri("minCardinality"),
    _iri("maxCardinality"),
    _iri("qualifiedCardinality"),
    _iri("minQualifiedCardinality"),
    _iri("maxQualifiedCardinality"),
}

_LIST_PREDICATES = {
    _iri("unionOf"): "class",
    _iri("intersectionOf"): "class",
    _iri("disjointUnionOf"): "class",
    _iri("propertyChainAxiom"): "property",
    _iri("hasKey"): "property",
    _iri("oneOf"): "individual",
}

_ANNOTATED_SOURCE = _iri("annotatedSource")
_ANNOTATED_PROPERTY = _iri("annotatedProperty")
_OWL_AXIOM = _iri("Axiom")
_OWL_MEMBERS = _iri("members")
_OWL_DISTINCT_MEMBERS = _iri("distinctMembers")
_OWL_ALL_DISJOINT_CLASSES = _iri("AllDisjointClasses")
_OWL_ALL_DISJOINT_PROPERTIES = _iri("AllDisjointProperties")


def _resource(term) -> bool:
    return isinstance(term, (NamedNode, BlankNode))


def _value(term) -> str | None:
    return term.value if isinstance(term, NamedNode) else None


def split_tbox_abox(triples: list[store.Triple]) -> RdfPartition:
    """Heuristically split a mixed RDF graph while preserving OWL expression structure."""
    by_subject: dict[object, list[store.Triple]] = defaultdict(list)
    declared_types: dict[object, set[str]] = defaultdict(set)
    for triple in triples:
        subject, predicate, obj = triple
        by_subject[subject].append(triple)
        if predicate.value == vocab.RDF_TYPE.value and isinstance(obj, NamedNode):
            declared_types[subject].add(obj.value)

    schema_nodes: set[object] = set()
    list_heads: deque[tuple[object, str]] = deque()

    for subject, predicate, obj in triples:
        predicate_iri = predicate.value
        object_iri = _value(obj)
        if predicate_iri == vocab.RDF_TYPE.value and object_iri in _SCHEMA_TYPES:
            schema_nodes.add(subject)
        if predicate_iri in _SCHEMA_SUBJECT_PREDICATES:
            schema_nodes.add(subject)
        if predicate_iri in _CLASS_LINK_PREDICATES | _PROPERTY_LINK_PREDICATES and _resource(obj):
            schema_nodes.add(obj)
        if role := _LIST_PREDICATES.get(predicate_iri):
            schema_nodes.add(subject)
            if _resource(obj):
                list_heads.append((obj, role))

        if predicate_iri in {_OWL_MEMBERS, _OWL_DISTINCT_MEMBERS} and _resource(obj):
            subject_types = declared_types.get(subject, set())
            if _OWL_ALL_DISJOINT_CLASSES in subject_types:
                list_heads.append((obj, "class"))
                schema_nodes.add(subject)
            elif _OWL_ALL_DISJOINT_PROPERTIES in subject_types:
                list_heads.append((obj, "property"))
                schema_nodes.add(subject)

    visited_lists: set[tuple[object, str]] = set()
    while list_heads:
        cell, role = list_heads.popleft()
        if (cell, role) in visited_lists or cell == vocab.RDF_NIL:
            continue
        visited_lists.add((cell, role))
        schema_nodes.add(cell)
        for _, predicate, obj in by_subject.get(cell, []):
            if predicate == vocab.RDF_REST and _resource(obj):
                list_heads.append((obj, role))
            elif predicate == vocab.RDF_FIRST and role != "individual" and _resource(obj):
                schema_nodes.add(obj)

    changed = True
    while changed:
        changed = False
        for subject, predicate, obj in triples:
            predicate_iri = predicate.value
            if subject in schema_nodes:
                if (
                    isinstance(obj, BlankNode)
                    and predicate_iri not in {vocab.RDF_FIRST.value, _iri("hasValue")}
                    and obj not in schema_nodes
                ):
                    schema_nodes.add(obj)
                    changed = True
                if (
                    predicate_iri in _CLASS_LINK_PREDICATES | _PROPERTY_LINK_PREDICATES
                    and _resource(obj)
                    and obj not in schema_nodes
                ):
                    schema_nodes.add(obj)
                    changed = True

        for axiom_node, types in declared_types.items():
            if _OWL_AXIOM not in types or axiom_node in schema_nodes:
                continue
            source_is_schema = any(
                predicate.value == _ANNOTATED_SOURCE and obj in schema_nodes
                for _, predicate, obj in by_subject.get(axiom_node, [])
            )
            property_is_schema = any(
                predicate.value == _ANNOTATED_PROPERTY
                and isinstance(obj, NamedNode)
                and obj.value in _SCHEMA_SUBJECT_PREDICATES | set(_LIST_PREDICATES)
                for _, predicate, obj in by_subject.get(axiom_node, [])
            )
            if source_is_schema or property_is_schema:
                schema_nodes.add(axiom_node)
                changed = True

    tbox, abox = [], []
    for triple in triples:
        (tbox if triple[0] in schema_nodes else abox).append(triple)
    return RdfPartition(tbox=tbox, abox=abox)


def partition_rdf(triples: list[store.Triple], target: str) -> RdfPartition:
    if target == "tbox":
        return RdfPartition(tbox=triples, abox=[])
    if target == "abox":
        return RdfPartition(tbox=[], abox=triples)
    if target == "auto":
        return split_tbox_abox(triples)
    raise RdfImportError(f"Unsupported RDF import target: {target}")
