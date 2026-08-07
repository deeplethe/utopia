"""Ontology-level semantics: map a structured ontology description (from the LLM or
from manual edits) to RDF triples, and read the stored named graph back into a curated
JSON view that the frontend consumes (it never sees raw triples).
"""
from __future__ import annotations

from dataclasses import dataclass, field

from pyoxigraph import BlankNode, Literal, NamedNode

from app.ontology import store, vocab
from app.ontology.vocab import (
    OWL_CLASS,
    OWL_DATATYPE_PROPERTY,
    OWL_DISJOINT_WITH,
    OWL_EQUIVALENT_CLASS,
    OWL_OBJECT_PROPERTY,
    OWL_UNION_OF,
    RDF_FIRST,
    RDF_NIL,
    RDF_REST,
    RDF_TYPE,
    RDFS_COMMENT,
    RDFS_DOMAIN,
    RDFS_LABEL,
    RDFS_RANGE,
    RDFS_SUBCLASSOF,
    class_local_name,
    norm_label,
    property_local_name,
)

Triple = tuple[object, object, object]

_XSD_MAP = {
    "string": "string", "text": "string", "str": "string",
    "integer": "integer", "int": "integer", "long": "integer",
    "float": "decimal", "double": "decimal", "decimal": "decimal", "number": "decimal",
    "boolean": "boolean", "bool": "boolean",
    "date": "date", "datetime": "dateTime", "time": "time",
    "uri": "anyURI", "url": "anyURI",
}


def datatype_node(name: str | None) -> NamedNode:
    key = (name or "string").strip().lower()
    return NamedNode(vocab.XSD + _XSD_MAP.get(key, "string"))


@dataclass
class Mutation:
    triples: list[Triple] = field(default_factory=list)
    axiom_keys: list[str] = field(default_factory=list)


@dataclass
class MergeIndex:
    """Existing-graph state used to merge extractions onto entities that already exist
    (including ones the user renamed), and to avoid adding duplicate labels."""

    class_by_norm: dict[str, str] = field(default_factory=dict)   # norm_label -> iri
    prop_by_norm: dict[str, str] = field(default_factory=dict)    # norm_label -> iri
    labeled: set[str] = field(default_factory=set)                # iris that already have rdfs:label


def read_index(graph_iri: str) -> MergeIndex:
    idx = MergeIndex()
    types: dict[str, str] = {}
    labels: dict[str, str] = {}
    for s, p, o in store.read_triples(graph_iri):
        if p.value == RDF_TYPE.value:
            if o.value == OWL_CLASS.value:
                types[s.value] = "class"
            elif o.value in (OWL_OBJECT_PROPERTY.value, OWL_DATATYPE_PROPERTY.value):
                types[s.value] = "prop"
        elif p.value == RDFS_LABEL.value:
            labels[s.value] = o.value
            idx.labeled.add(s.value)
    from app.ontology.vocab import norm_label

    for iri, kind in types.items():
        label = labels.get(iri, "")
        if not label:
            continue
        norm = norm_label(label)
        if kind == "class":
            idx.class_by_norm.setdefault(norm, iri)
        else:
            idx.prop_by_norm.setdefault(norm, iri)
    return idx


def build_mutation(base_iri: str, onto: dict, index: MergeIndex | None = None) -> Mutation:
    """Translate an LLM/manual ontology dict into triples + provenance keys.

    Referenced-but-undeclared classes are auto-declared. Idempotent at the triple level
    (Oxigraph collapses identical quads), so re-running across chunks is safe.
    """
    mut = Mutation()
    idx = index or MergeIndex()
    seen_classes: set[str] = set()    # locals handled this run
    seen_props: set[str] = set()
    labeled_run: set[str] = set()     # iris we already gave a label this run

    def _add_label(node: NamedNode, label: str) -> None:
        if node.value in idx.labeled or node.value in labeled_run:
            return
        mut.triples.append((node, RDFS_LABEL, Literal(label)))
        labeled_run.add(node.value)

    def ensure_class(label: str) -> tuple[NamedNode, str]:
        existing = idx.class_by_norm.get(norm_label(label))
        node = NamedNode(existing) if existing else NamedNode(base_iri + class_local_name(label))
        local = _local(node.value)
        if local not in seen_classes:
            seen_classes.add(local)
            mut.triples.append((node, RDF_TYPE, OWL_CLASS))
            _add_label(node, label)
            mut.axiom_keys.append(f"class|{local}")
        return node, local

    def declare_prop(label: str, is_object: bool) -> tuple[NamedNode, str]:
        existing = idx.prop_by_norm.get(norm_label(label))
        node = NamedNode(existing) if existing else NamedNode(base_iri + property_local_name(label))
        local = _local(node.value)
        if local not in seen_props:
            seen_props.add(local)
            ptype = OWL_OBJECT_PROPERTY if is_object else OWL_DATATYPE_PROPERTY
            mut.triples.append((node, RDF_TYPE, ptype))
            _add_label(node, label)
            mut.axiom_keys.append(f"{'objprop' if is_object else 'dataprop'}|{local}")
        return node, local

    def _label(item: dict) -> str | None:
        return item.get("label") or item.get("name")

    # Classes (explicit first so their comments/labels take precedence)
    for c in onto.get("classes", []) or []:
        label = _label(c)
        if not label:
            continue
        node, _ = ensure_class(label)
        comment = c.get("comment") or c.get("description")
        if comment:
            mut.triples.append((node, RDFS_COMMENT, Literal(str(comment))))

    # Object properties
    for p in onto.get("object_properties", []) or []:
        label = _label(p)
        if not label:
            continue
        node, local = declare_prop(label, is_object=True)
        if p.get("comment"):
            mut.triples.append((node, RDFS_COMMENT, Literal(str(p["comment"]))))
        if p.get("domain"):
            dnode, dlocal = ensure_class(str(p["domain"]))
            mut.triples.append((node, RDFS_DOMAIN, dnode))
            mut.axiom_keys.append(f"domain|{local}|{dlocal}")
        if p.get("range"):
            rnode, rlocal = ensure_class(str(p["range"]))
            mut.triples.append((node, RDFS_RANGE, rnode))
            mut.axiom_keys.append(f"range|{local}|{rlocal}")

    # Data properties
    for p in onto.get("data_properties", []) or []:
        label = _label(p)
        if not label:
            continue
        node, local = declare_prop(label, is_object=False)
        if p.get("comment"):
            mut.triples.append((node, RDFS_COMMENT, Literal(str(p["comment"]))))
        if p.get("domain"):
            dnode, dlocal = ensure_class(str(p["domain"]))
            mut.triples.append((node, RDFS_DOMAIN, dnode))
            mut.axiom_keys.append(f"domain|{local}|{dlocal}")
        rng = datatype_node(p.get("range"))
        mut.triples.append((node, RDFS_RANGE, rng))
        mut.axiom_keys.append(f"range|{local}|{rng.value.rsplit('#', 1)[-1]}")

    # subClassOf
    for r in onto.get("subclass_of", []) or []:
        sub = r.get("sub") or r.get("child") or r.get("subclass")
        sup = r.get("super") or r.get("parent") or r.get("superclass")
        if not sub or not sup:
            continue
        snode, slocal = ensure_class(str(sub))
        pnode, plocal = ensure_class(str(sup))
        if slocal == plocal:
            continue  # ignore self-subclass noise
        mut.triples.append((snode, RDFS_SUBCLASSOF, pnode))
        mut.axiom_keys.append(f"subClassOf|{slocal}|{plocal}")

    # disjointWith
    for r in onto.get("disjoint_with", []) or []:
        a, b = r.get("a"), r.get("b")
        if not a or not b:
            continue
        anode, al = ensure_class(str(a))
        bnode, bl = ensure_class(str(b))
        if al == bl:
            continue
        mut.triples.append((anode, OWL_DISJOINT_WITH, bnode))
        mut.axiom_keys.append("disjointWith|" + "|".join(sorted([al, bl])))

    # equivalentClass
    for r in onto.get("equivalent_class", []) or []:
        a, b = r.get("a"), r.get("b")
        if not a or not b:
            continue
        anode, al = ensure_class(str(a))
        bnode, bl = ensure_class(str(b))
        if al == bl:
            continue
        mut.triples.append((anode, OWL_EQUIVALENT_CLASS, bnode))
        mut.axiom_keys.append("equivalentClass|" + "|".join(sorted([al, bl])))

    return mut


# --------------------------------------------------------------------------- #
# Reading the graph back into a curated view
# --------------------------------------------------------------------------- #
def _local(iri: str) -> str:
    if "#" in iri:
        return iri.rsplit("#", 1)[-1]
    return iri.rstrip("/").rsplit("/", 1)[-1]


def _is_xsd(iri: str) -> bool:
    return iri.startswith(vocab.XSD)


def build_view(graph_iri: str) -> dict:
    """Read the named graph and return the curated {classes, properties, axioms} view."""
    classes: dict[str, dict] = {}
    obj_props: dict[str, dict] = {}
    data_props: dict[str, dict] = {}
    labels: dict[str, str] = {}
    comments: dict[str, str] = {}
    subclass_of: list[dict] = []
    disjoint_with: list[dict] = []
    equivalent_class: list[dict] = []
    domains: dict[str, str] = {}
    ranges: dict[str, str] = {}
    domain_all: dict[str, list[str]] = {}  # ALL rdfs:domain values (a prop may have several)
    range_all: dict[str, list[str]] = {}
    union_head: dict[str, str] = {}   # union bnode -> first list cell
    list_first: dict[str, str] = {}   # list cell -> member iri
    list_rest: dict[str, str] = {}    # list cell -> next cell

    for s, p, o in store.read_triples(graph_iri):
        siri = s.value
        piri = p.value
        if piri == RDF_TYPE.value:
            oiri = o.value
            # Blank-node classes are anonymous expressions (owl:unionOf) — not named classes.
            if oiri == OWL_CLASS.value and not isinstance(s, BlankNode):
                classes.setdefault(siri, {"iri": siri})
            elif oiri == OWL_OBJECT_PROPERTY.value:
                obj_props.setdefault(siri, {"iri": siri})
            elif oiri == OWL_DATATYPE_PROPERTY.value:
                data_props.setdefault(siri, {"iri": siri})
        elif piri == RDFS_LABEL.value:
            labels[siri] = o.value
        elif piri == RDFS_COMMENT.value:
            comments[siri] = o.value
        elif piri == RDFS_SUBCLASSOF.value:
            subclass_of.append({"sub": siri, "super": o.value})
        elif piri == RDFS_DOMAIN.value:
            domains[siri] = o.value
            domain_all.setdefault(siri, []).append(o.value)
        elif piri == RDFS_RANGE.value:
            ranges[siri] = o.value
            range_all.setdefault(siri, []).append(o.value)
        elif piri == OWL_DISJOINT_WITH.value:
            disjoint_with.append({"a": siri, "b": o.value})
        elif piri == OWL_EQUIVALENT_CLASS.value:
            equivalent_class.append({"a": siri, "b": o.value})
        elif piri == OWL_UNION_OF.value:
            union_head[siri] = o.value
        elif piri == RDF_FIRST.value:
            list_first[siri] = o.value
        elif piri == RDF_REST.value:
            list_rest[siri] = o.value

    def _union_members(bval: str) -> list[str]:
        cur, out, guard = union_head.get(bval), [], 0
        while cur and cur != RDF_NIL.value and guard < 1000:
            if cur in list_first:
                out.append(list_first[cur])
            cur = list_rest.get(cur)
            guard += 1
        return out

    def _concrete_members(vals: list[str]) -> list[str]:
        """Flatten a property's domain/range value(s) to the concrete class/datatype iris it
        admits: every rdfs:domain/range triple, with any owl:unionOf expanded to its members.
        De-duped, order-preserving. Used by validation to reason over ALL admitted classes rather
        than one arbitrary last-writer value."""
        out, seen = [], set()
        for v in vals:
            for m in (_union_members(v) if v in union_head else [v]):
                if m not in seen:
                    seen.add(m)
                    out.append(m)
        return out

    def label_of(iri: str) -> str:
        if iri in union_head:  # anonymous union expression
            return " ∪ ".join(label_of(m) for m in _union_members(iri))
        return labels.get(iri) or _local(iri)

    superclasses: dict[str, list[str]] = {}
    for r in subclass_of:
        superclasses.setdefault(r["sub"], []).append(r["super"])

    class_list = [
        {
            "iri": iri,
            "local": _local(iri),
            "label": label_of(iri),
            "comment": comments.get(iri, ""),
            "superclasses": superclasses.get(iri, []),
        }
        for iri in sorted(classes, key=label_of)
    ]

    def prop_entry(iri: str, datatype_range: bool) -> dict:
        rng = ranges.get(iri)
        return {
            "iri": iri,
            "local": _local(iri),
            "label": label_of(iri),
            "comment": comments.get(iri, ""),
            "domain": domains.get(iri),
            "domain_label": label_of(domains[iri]) if iri in domains else None,
            "range": rng,
            "range_label": ("xsd:" + _local(rng)) if (rng and _is_xsd(rng)) else (label_of(rng) if rng else None),
            # ALL admitted classes (every domain/range triple, unions expanded) — validation reasons
            # over these so a multi-valued or union domain/range isn't collapsed to one arbitrary value.
            "domain_members": _concrete_members(domain_all.get(iri, [])),
            "range_members": _concrete_members(range_all.get(iri, [])),
        }

    obj_list = [prop_entry(iri, False) for iri in sorted(obj_props, key=label_of)]
    data_list = [prop_entry(iri, True) for iri in sorted(data_props, key=label_of)]

    return {
        "classes": class_list,
        "object_properties": obj_list,
        "data_properties": data_list,
        "axioms": {
            "subclass_of": subclass_of,
            "disjoint_with": disjoint_with,
            "equivalent_class": equivalent_class,
        },
        "labels": {iri: label_of(iri) for iri in set(classes) | set(obj_props) | set(data_props)},
        "stats": {
            "class_count": len(classes),
            "property_count": len(obj_props) + len(data_props),
            "axiom_count": len(subclass_of) + len(disjoint_with) + len(equivalent_class),
        },
    }


def graph_stats(graph_iri: str) -> dict:
    view = build_view(graph_iri)
    return view["stats"]
