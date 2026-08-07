"""ABox (instance) layer: individuals typed by TBox classes, with object/data property
assertions, stored in a *separate* named graph per knowledge system (``.../{id}/abox``) so
the TBox graph stays small and the (much larger) ABox scales independently.

Reads currently scan the graph in Python — fine for the demo; for tens of thousands of
individuals this should move to SPARQL / indexed pattern queries (tracked as follow-up).
"""
from __future__ import annotations

import uuid

from pyoxigraph import Literal, NamedNode

from app.ontology import store, vocab


def _n(iri: str) -> NamedNode:
    return NamedNode(iri)


def mint_iri(base_iri: str) -> str:
    return f"{base_iri}ind-{uuid.uuid4().hex[:12]}"


# --------------------------------------------------------------------------- #
# Mutations (call inside store.capture() so they land in history / rollback)
# --------------------------------------------------------------------------- #
def create_individual(abox_iri: str, base_iri: str, label: str, class_iri: str) -> str:
    iri = mint_iri(base_iri)
    triples = [
        (_n(iri), vocab.RDF_TYPE, vocab.OWL_NAMED_INDIVIDUAL),
        (_n(iri), vocab.RDF_TYPE, _n(class_iri)),
    ]
    if label and label.strip():
        triples.append((_n(iri), vocab.RDFS_LABEL, Literal(label.strip())))
    store.add_triples(abox_iri, triples)
    return iri


def delete_individual(abox_iri: str, iri: str) -> int:
    return store.remove_entity(abox_iri, iri)


def add_type(abox_iri: str, iri: str, class_iri: str) -> None:
    store.add_triples(abox_iri, [(_n(iri), vocab.RDF_TYPE, _n(class_iri))])


def remove_type(abox_iri: str, iri: str, class_iri: str) -> None:
    store.remove_triples(abox_iri, [(_n(iri), vocab.RDF_TYPE, _n(class_iri))])


def add_object_assertion(abox_iri: str, s: str, p: str, o: str) -> bool:
    """Add the triple; returns True only if it wasn't already present (for accurate counting)."""
    if store.has_triple(abox_iri, _n(s), _n(p), _n(o)):
        return False
    store.add_triples(abox_iri, [(_n(s), _n(p), _n(o))])
    return True


def remove_object_assertion(abox_iri: str, s: str, p: str, o: str) -> None:
    store.remove_triples(abox_iri, [(_n(s), _n(p), _n(o))])


def add_data_assertion(abox_iri: str, s: str, p: str, value: str, datatype: str | None = None) -> bool:
    lit = Literal(value, datatype=_n(datatype)) if datatype else Literal(value)
    if store.has_triple(abox_iri, _n(s), _n(p), lit):
        return False
    store.add_triples(abox_iri, [(_n(s), _n(p), lit)])
    return True


def remove_data_assertion(abox_iri: str, s: str, p: str, value: str, datatype: str | None = None) -> None:
    lit = Literal(value, datatype=_n(datatype)) if datatype else Literal(value)
    store.remove_triples(abox_iri, [(_n(s), _n(p), lit)])


# --------------------------------------------------------------------------- #
# Reads
# --------------------------------------------------------------------------- #
def _all(abox_iri: str):
    return store.read_triples(abox_iri)


def _label_index(abox_iri: str) -> dict[str, str]:
    """iri -> its rdfs:label, for rendering assertion targets."""
    out: dict[str, str] = {}
    for s, p, o in _all(abox_iri):
        if isinstance(s, NamedNode) and p.value == vocab.RDFS_LABEL.value and isinstance(o, Literal):
            out[s.value] = o.value
    return out


def label_index(abox_iri: str) -> dict[str, str]:
    """Public one-scan iri -> label map (callers that need many labels at once should use
    this instead of get_individual() per iri)."""
    return _label_index(abox_iri)


def list_individuals(
    abox_iri: str, class_labels: dict[str, str], *,
    class_iri: str | None = None, q: str | None = None, offset: int = 0, limit: int = 20,
) -> tuple[list[dict], int]:
    inds: dict[str, dict] = {}
    for s, p, o in _all(abox_iri):
        if not isinstance(s, NamedNode):
            continue
        d = inds.setdefault(s.value, {"types": set(), "label": None})
        if p.value == vocab.RDF_TYPE.value and isinstance(o, NamedNode):
            if o.value != vocab.OWL_NAMED_INDIVIDUAL.value:
                d["types"].add(o.value)
        elif p.value == vocab.RDFS_LABEL.value and isinstance(o, Literal):
            d["label"] = o.value

    items = []
    for iri, d in inds.items():
        if class_iri and class_iri not in d["types"]:
            continue
        label = d["label"] or iri.rsplit("ind-", 1)[-1]
        if q and q.strip().lower() not in label.lower():
            continue
        items.append({
            "iri": iri, "label": label,
            "types": [{"iri": t, "label": class_labels.get(t, t)} for t in sorted(d["types"])],
        })
    items.sort(key=lambda x: x["label"])
    return items[offset:offset + limit], len(items)


def get_individual(
    abox_iri: str, iri: str, class_labels: dict[str, str], prop_labels: dict[str, str],
) -> dict | None:
    ind_labels = _label_index(abox_iri)
    label = ind_labels.get(iri)
    types, obj, data = [], [], []
    found = False
    for s, p, o in _all(abox_iri):
        if not (isinstance(s, NamedNode) and s.value == iri):
            continue
        found = True
        if p.value == vocab.RDF_TYPE.value and isinstance(o, NamedNode):
            if o.value != vocab.OWL_NAMED_INDIVIDUAL.value:
                types.append({"iri": o.value, "label": class_labels.get(o.value, o.value)})
        elif p.value == vocab.RDFS_LABEL.value:
            continue
        elif isinstance(o, NamedNode):
            obj.append({
                "prop": p.value, "prop_label": prop_labels.get(p.value, p.value),
                "target": o.value, "target_label": ind_labels.get(o.value, o.value),
            })
        elif isinstance(o, Literal):
            data.append({
                "prop": p.value, "prop_label": prop_labels.get(p.value, p.value),
                "value": o.value, "datatype": o.datatype.value if o.datatype else None,
            })
    if not found:
        return None
    return {"iri": iri, "label": label or iri.rsplit("ind-", 1)[-1],
            "types": types, "object_assertions": obj, "data_assertions": data}


def counts_by_class(abox_iri: str) -> dict[str, int]:
    """class_iri -> number of individuals typed with it (for the browse sidebar)."""
    out: dict[str, int] = {}
    for s, p, o in _all(abox_iri):
        if (p.value == vocab.RDF_TYPE.value and isinstance(o, NamedNode)
                and o.value != vocab.OWL_NAMED_INDIVIDUAL.value):
            out[o.value] = out.get(o.value, 0) + 1
    return out


def individuals_of_class(abox_iri: str, class_iri: str) -> list[tuple[str, str]]:
    """(iri, label) of every individual typed with ``class_iri`` — used by entity resolution
    to compare a new mention only against instances of the same class."""
    typed: set[str] = set()
    labels: dict[str, str] = {}
    for s, p, o in _all(abox_iri):
        if not isinstance(s, NamedNode):
            continue
        if p.value == vocab.RDF_TYPE.value and isinstance(o, NamedNode) and o.value == class_iri:
            typed.add(s.value)
        elif p.value == vocab.RDFS_LABEL.value and isinstance(o, Literal):
            labels[s.value] = o.value
    return [(iri, labels.get(iri, iri.rsplit("ind-", 1)[-1])) for iri in typed]


def individuals_of_classes(abox_iri: str, class_iris: set[str]) -> list[tuple[str, str]]:
    """(iri, label) of individuals typed with ANY of the given classes — used so entity
    resolution can consider candidates across a class hierarchy (parent/child), not just the
    exact class, and merge e.g. a person typed both 'Worker' and 'Senior Worker'."""
    if not class_iris:
        return []
    typed: set[str] = set()
    labels: dict[str, str] = {}
    for s, p, o in _all(abox_iri):
        if not isinstance(s, NamedNode):
            continue
        if p.value == vocab.RDF_TYPE.value and isinstance(o, NamedNode) and o.value in class_iris:
            typed.add(s.value)
        elif p.value == vocab.RDFS_LABEL.value and isinstance(o, Literal):
            labels[s.value] = o.value
    return [(iri, labels.get(iri, iri.rsplit("ind-", 1)[-1])) for iri in typed]


def exists(abox_iri: str, iri: str) -> bool:
    return bool(store.object_terms(abox_iri, _n(iri), vocab.RDF_TYPE))


class ResolutionIndex:
    """In-memory mirror of the ABox facts entity resolution queries (rdf:type + rdfs:label), built
    with ONE graph scan and kept in sync as resolution mints/retypes individuals — so a chunk of M
    mentions costs 1 scan + M lookups instead of ~3 full ABox scans PER mention.

    Faithful stand-in for individuals_of_class / individuals_of_classes / label_index / exists:
    each method returns exactly what its scanning counterpart would, and add_individual / add_type
    reflect the only two mutations resolution makes that affect those queries."""

    __slots__ = ("by_class", "labels", "known")

    def __init__(self) -> None:
        self.by_class: dict[str, set[str]] = {}  # class_iri -> individual iris typed with it
        self.labels: dict[str, str] = {}         # individual iri -> rdfs:label
        self.known: set[str] = set()             # every individual iri (has some rdf:type)

    def _lbl(self, iri: str) -> str:
        return self.labels.get(iri, iri.rsplit("ind-", 1)[-1])

    def add_individual(self, iri: str, label: str | None, class_iri: str) -> None:
        self.known.add(iri)
        if label:
            self.labels[iri] = label
        self.by_class.setdefault(class_iri, set()).add(iri)

    def add_type(self, iri: str, class_iri: str) -> None:
        self.known.add(iri)
        self.by_class.setdefault(class_iri, set()).add(iri)

    def individuals_of_class(self, class_iri: str) -> list[tuple[str, str]]:
        return [(iri, self._lbl(iri)) for iri in self.by_class.get(class_iri, ())]

    def individuals_of_classes(self, class_iris: set[str]) -> list[tuple[str, str]]:
        out: list[tuple[str, str]] = []
        seen: set[str] = set()
        for c in class_iris or ():
            for iri in self.by_class.get(c, ()):
                if iri not in seen:
                    seen.add(iri)
                    out.append((iri, self._lbl(iri)))
        return out

    def label_index(self) -> dict[str, str]:
        return self.labels

    def exists(self, iri: str) -> bool:
        return iri in self.known


def build_resolution_index(abox_iri: str) -> ResolutionIndex:
    """One-scan snapshot of the ABox for entity resolution (see ResolutionIndex)."""
    idx = ResolutionIndex()
    for s, p, o in _all(abox_iri):
        if not isinstance(s, NamedNode):
            continue
        if p.value == vocab.RDF_TYPE.value and isinstance(o, NamedNode):
            idx.known.add(s.value)
            if o.value != vocab.OWL_NAMED_INDIVIDUAL.value:
                idx.by_class.setdefault(o.value, set()).add(s.value)
        elif p.value == vocab.RDFS_LABEL.value and isinstance(o, Literal):
            idx.labels[s.value] = o.value
    return idx
