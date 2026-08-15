"""Manual ontology editing operations.

A single ``apply_edit(graph_iri, base_iri, op)`` dispatcher applies a structured edit
to a knowledge system's named graph. The same operations power conflict resolution
(each detected conflict carries suggested ops). References to existing entities are
passed as IRIs; new entities are created from a label.
"""
from __future__ import annotations

import hashlib
import re

from pyoxigraph import BlankNode, Literal, NamedNode

from app.ontology import schema, store
from app.ontology.vocab import (
    OWL_CLASS,
    OWL_DATATYPE_PROPERTY,
    OWL_DISJOINT_WITH,
    OWL_EQUIVALENT_CLASS,
    OWL_NAMED_INDIVIDUAL,
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
    RDFS_SUBPROPERTYOF,
    class_local_name,
    norm_label,
    property_local_name,
)


class EditError(ValueError):
    pass


def _abox_iri(graph_iri: str) -> str:
    """The instance graph paired with a TBox graph (mirrors abox_iri_for in the API)."""
    return graph_iri.rstrip("/") + "/abox"


def _is_iri(ref: str) -> bool:
    # RDF IRIs are not limited to HTTP (tests, imported vocabularies, and enterprise
    # schemas commonly use urn:).  Require a URI scheme so ordinary labels that happen
    # to contain punctuation are still treated as labels.
    return bool(re.match(r"^[A-Za-z][A-Za-z0-9+.-]*:", ref))


def _class_node(graph_iri: str, base_iri: str, ref: str, *, create: bool = True) -> NamedNode:
    if _is_iri(ref):
        node = NamedNode(ref)
        if not create and not store.has_triple(graph_iri, node, RDF_TYPE, OWL_CLASS):
            raise EditError(f"Class not found: {ref}")
        return node
    idx = schema.read_index(graph_iri)
    existing = idx.class_by_norm.get(norm_label(ref))
    if existing:
        return NamedNode(existing)
    if not create:
        raise EditError(f"Class not found: {ref}")
    return NamedNode(base_iri + class_local_name(ref))


def _ensure_labeled_class(
    graph_iri: str,
    base_iri: str,
    ref: str,
    *,
    allow_new_label: bool = True,
) -> NamedNode:
    # Labels are the deliberate class-creation syntax used by the editor.  An IRI is
    # always a reference and must already be declared; otherwise stale agent payloads
    # could silently resurrect a deleted class as an unlabeled shell.
    node = _class_node(graph_iri, base_iri, ref, create=allow_new_label and not _is_iri(ref))
    store.add_triples(graph_iri, [(node, RDF_TYPE, OWL_CLASS)])
    if not store.has_triple(graph_iri, node, RDFS_LABEL, None) and not _is_iri(ref):
        store.add_triples(graph_iri, [(node, RDFS_LABEL, Literal(ref))])
    return node


def _prop_is_object(graph_iri: str, node: NamedNode) -> bool:
    return store.has_triple(graph_iri, node, RDF_TYPE, OWL_OBJECT_PROPERTY)


def _set_single(graph_iri: str, subj: NamedNode, pred, obj) -> None:
    """Replace all values of a single-valued predicate."""
    for old in store.object_terms(graph_iri, subj, pred):
        if isinstance(old, BlankNode):
            _gc_blank(graph_iri, old)
    store.remove_pattern(graph_iri, subj, pred, None)
    if obj is not None:
        store.add_triples(graph_iri, [(subj, pred, obj)])


# --------------------------------------------------------------------------- #
# Operations
# --------------------------------------------------------------------------- #
def _add_class(graph_iri, base_iri, p):
    label = (p.get("label") or "").strip()
    if not label:
        raise EditError("label required")
    node = _ensure_labeled_class(graph_iri, base_iri, label)
    if "comment" in p:
        _set_single(graph_iri, node, RDFS_COMMENT, Literal(str(p["comment"])) if p["comment"] else None)
    return node.value


def _update_class(graph_iri, base_iri, p):
    iri = p["iri"]
    node = NamedNode(iri)
    if not store.has_triple(graph_iri, node, RDF_TYPE, OWL_CLASS):
        raise EditError(f"Class not found: {iri}")
    if p.get("label"):
        _set_single(graph_iri, node, RDFS_LABEL, Literal(str(p["label"])))
    if "comment" in p:
        _set_single(graph_iri, node, RDFS_COMMENT, Literal(str(p["comment"])) if p["comment"] else None)
    return iri


def _cascade_class_delete(graph_iri, cls_iri: str) -> None:
    """Deleting a class cascades into the ABox: drop each instance's rdf:type to this class, and
    if that was the individual's only type, delete the whole individual (with its assertions).
    Never leaves an individual dangling on a class that no longer exists."""
    abox_iri = _abox_iri(graph_iri)
    types_by_ind: dict[str, set[str]] = {}
    for s, pr, o in store.read_triples(abox_iri):
        if pr.value == RDF_TYPE.value and isinstance(o, NamedNode):
            types_by_ind.setdefault(s.value, set()).add(o.value)
    for ind, types in types_by_ind.items():
        if cls_iri not in types:
            continue
        remaining = types - {cls_iri, OWL_NAMED_INDIVIDUAL.value}
        if remaining:  # still an instance of another class → just drop the one type
            store.remove_triples(abox_iri, [(NamedNode(ind), RDF_TYPE, NamedNode(cls_iri))])
        else:  # its only type is gone → remove the individual entirely
            store.remove_entity(abox_iri, ind)


def _delete_class(graph_iri, base_iri, p):
    iri = p["iri"]
    node = NamedNode(iri)
    if not store.has_triple(graph_iri, node, RDF_TYPE, OWL_CLASS):
        raise EditError(f"Class not found: {iri}")

    # A class can be referenced from an anonymous owl:unionOf.  Removing just the
    # rdf:first statement would leave a malformed list behind.  Normalize every
    # affected slot first: keep a smaller union, collapse a singleton to the named
    # class, or clear an empty slot.
    view = schema.build_view(graph_iri)
    for key in ("object_properties", "data_properties"):
        for prop in view[key]:
            for slot, pred in (("domain", RDFS_DOMAIN), ("range", RDFS_RANGE)):
                members = prop.get(f"{slot}_members", [])
                if iri not in members or not isinstance(prop.get(slot), str):
                    continue
                remaining = [member for member in members if member != iri]
                if len(remaining) >= 2:
                    _set_property_union(graph_iri, base_iri, {
                        "iri": prop["iri"], "slot": slot, "members": remaining,
                    })
                elif remaining:
                    _set_single(graph_iri, NamedNode(prop["iri"]), pred, NamedNode(remaining[0]))
                else:
                    _set_single(graph_iri, NamedNode(prop["iri"]), pred, None)

    store.remove_entity(graph_iri, iri)
    _cascade_class_delete(graph_iri, iri)
    return iri


def _member_values(p: dict, key: str) -> list[str] | None:
    if key not in p:
        return None
    value = p[key]
    if not isinstance(value, list):
        raise EditError(f"{key} must be an array")
    members: list[str] = []
    for item in value:
        text = str(item).strip()
        if not text:
            raise EditError(f"{key} cannot contain an empty value")
        if text not in members:
            members.append(text)
    return members


def _apply_class_members(graph_iri: str, base_iri: str, node: NamedNode, slot: str,
                         members: list[str]) -> None:
    pred = RDFS_DOMAIN if slot == "domain" else RDFS_RANGE
    if not members:
        _set_single(graph_iri, node, pred, None)
    elif len(members) == 1:
        _set_single(
            graph_iri, node, pred,
            _ensure_labeled_class(graph_iri, base_iri, members[0]),
        )
    else:
        resolved = [
            _ensure_labeled_class(graph_iri, base_iri, member).value for member in members
        ]
        _set_property_union(graph_iri, base_iri, {
            "iri": node.value, "slot": slot, "members": resolved,
        })


def _add_property(graph_iri, base_iri, p):
    label = (p.get("label") or "").strip()
    if not label:
        raise EditError("label required")
    kind = p.get("kind", "object")
    if kind not in {"object", "data"}:
        raise EditError("kind must be object or data")
    is_object = kind == "object"
    domain_members = _member_values(p, "domain_members")
    range_members = _member_values(p, "range_members")
    if domain_members is not None and p.get("domain"):
        raise EditError("use either domain or domain_members, not both")
    if range_members is not None and p.get("range"):
        raise EditError("use either range or range_members, not both")
    if not is_object and range_members is not None:
        raise EditError("range_members is only valid for object properties")
    idx = schema.read_index(graph_iri)
    existing = idx.prop_by_norm.get(norm_label(label))
    node = NamedNode(existing) if existing else NamedNode(base_iri + property_local_name(label))
    ptype = OWL_OBJECT_PROPERTY if is_object else OWL_DATATYPE_PROPERTY
    opposite_type = OWL_DATATYPE_PROPERTY if is_object else OWL_OBJECT_PROPERTY
    if store.has_triple(graph_iri, node, RDF_TYPE, opposite_type):
        opposite_kind = "data" if is_object else "object"
        raise EditError(
            f"Property already exists as a {opposite_kind} property: {node.value}"
        )
    store.add_triples(graph_iri, [(node, RDF_TYPE, ptype)])
    if not store.has_triple(graph_iri, node, RDFS_LABEL, None):
        store.add_triples(graph_iri, [(node, RDFS_LABEL, Literal(label))])
    if p.get("comment"):
        _set_single(graph_iri, node, RDFS_COMMENT, Literal(str(p["comment"])))
    if p.get("domain"):
        _set_single(graph_iri, node, RDFS_DOMAIN, _ensure_labeled_class(graph_iri, base_iri, str(p["domain"])))
    if p.get("range"):
        rng = _ensure_labeled_class(graph_iri, base_iri, str(p["range"])) if is_object else schema.datatype_node(str(p["range"]))
        _set_single(graph_iri, node, RDFS_RANGE, rng)
    if domain_members is not None:
        _apply_class_members(graph_iri, base_iri, node, "domain", domain_members)
    if range_members is not None:
        _apply_class_members(graph_iri, base_iri, node, "range", range_members)
    return node.value


def _update_property(graph_iri, base_iri, p):
    iri = p["iri"]
    node = NamedNode(iri)
    if not (store.has_triple(graph_iri, node, RDF_TYPE, OWL_OBJECT_PROPERTY)
            or store.has_triple(graph_iri, node, RDF_TYPE, OWL_DATATYPE_PROPERTY)):
        raise EditError(f"Property not found: {iri}")
    is_object = _prop_is_object(graph_iri, node)
    domain_members = _member_values(p, "domain_members")
    range_members = _member_values(p, "range_members")
    if domain_members is not None and (p.get("domain") or p.get("clear_domain")):
        raise EditError("use domain_members instead of domain/clear_domain")
    if range_members is not None and (p.get("range") or p.get("clear_range")):
        raise EditError("use range_members instead of range/clear_range")
    if not is_object and range_members is not None:
        raise EditError("range_members is only valid for object properties")
    if p.get("label"):
        _set_single(graph_iri, node, RDFS_LABEL, Literal(str(p["label"])))
    if "comment" in p:
        _set_single(graph_iri, node, RDFS_COMMENT, Literal(str(p["comment"])) if p["comment"] else None)
    if p.get("clear_domain"):
        _set_single(graph_iri, node, RDFS_DOMAIN, None)
    elif p.get("domain"):
        _set_single(graph_iri, node, RDFS_DOMAIN, _ensure_labeled_class(graph_iri, base_iri, str(p["domain"])))
    if p.get("clear_range"):
        _set_single(graph_iri, node, RDFS_RANGE, None)
    elif p.get("range"):
        rng = _ensure_labeled_class(graph_iri, base_iri, str(p["range"])) if is_object else schema.datatype_node(str(p["range"]))
        _set_single(graph_iri, node, RDFS_RANGE, rng)
    if domain_members is not None:
        _apply_class_members(graph_iri, base_iri, node, "domain", domain_members)
    if range_members is not None:
        _apply_class_members(graph_iri, base_iri, node, "range", range_members)
    return iri


def _delete_property(graph_iri, base_iri, p):
    node = NamedNode(p["iri"])
    if not (
        store.has_triple(graph_iri, node, RDF_TYPE, OWL_OBJECT_PROPERTY)
        or store.has_triple(graph_iri, node, RDF_TYPE, OWL_DATATYPE_PROPERTY)
    ):
        raise EditError(f"Property not found: {p['iri']}")
    # Remember anonymous domain/range expressions before dropping the property's
    # incoming links.  ``remove_entity`` does not recursively remove those blank-node
    # subgraphs, so without this cleanup every deleted union would remain orphaned.
    blank_slots = {
        value
        for predicate in (RDFS_DOMAIN, RDFS_RANGE)
        for value in store.object_terms(graph_iri, node, predicate)
        if isinstance(value, BlankNode)
    }
    store.remove_entity(graph_iri, p["iri"])
    for blank in blank_slots:
        if not any(
            isinstance(obj, BlankNode) and obj.value == blank.value
            for _, _, obj in store.read_triples(graph_iri)
        ):
            _gc_blank(graph_iri, blank)
    # Cascade into the ABox: drop the instance assertions that used this property as a predicate,
    # so they don't dangle on a property that no longer exists.
    abox_iri = _abox_iri(graph_iri)
    used = [(s, pr, o) for s, pr, o in store.read_triples(abox_iri) if pr.value == p["iri"]]
    if used:
        store.remove_triples(abox_iri, used)
    return p["iri"]


def _add_axiom(graph_iri, base_iri, p):
    t = p["type"]
    if t == "subclass":
        sub = _ensure_labeled_class(graph_iri, base_iri, str(p["sub"]))
        sup = _ensure_labeled_class(graph_iri, base_iri, str(p["super"]))
        if sub.value != sup.value:
            store.add_triples(graph_iri, [(sub, RDFS_SUBCLASSOF, sup)])
    elif t in ("disjoint", "equivalent"):
        a = _ensure_labeled_class(graph_iri, base_iri, str(p["a"]))
        b = _ensure_labeled_class(graph_iri, base_iri, str(p["b"]))
        pred = OWL_DISJOINT_WITH if t == "disjoint" else OWL_EQUIVALENT_CLASS
        if a.value != b.value:
            store.add_triples(graph_iri, [(a, pred, b)])
    else:
        raise EditError(f"Unknown axiom type: {t}")
    return t


def _delete_axiom(graph_iri, base_iri, p):
    t = p["type"]
    if t == "subclass":
        sub, sup = NamedNode(p["sub"]), NamedNode(p["super"])
        if not store.has_triple(graph_iri, sub, RDFS_SUBCLASSOF, sup):
            raise EditError("Axiom not found")
        store.remove_pattern(graph_iri, sub, RDFS_SUBCLASSOF, sup)
    elif t in ("disjoint", "equivalent"):
        pred = OWL_DISJOINT_WITH if t == "disjoint" else OWL_EQUIVALENT_CLASS
        a, b = NamedNode(p["a"]), NamedNode(p["b"])
        if not (
            store.has_triple(graph_iri, a, pred, b)
            or store.has_triple(graph_iri, b, pred, a)
        ):
            raise EditError("Axiom not found")
        store.remove_pattern(graph_iri, a, pred, b)
        store.remove_pattern(graph_iri, b, pred, a)  # symmetric: remove either direction
    else:
        raise EditError(f"Unknown axiom type: {t}")
    return t


def _gc_blank(graph_iri: str, bnode: BlankNode) -> None:
    """Remove a blank-node expression and every blank node reachable from it (e.g. an
    owl:unionOf class and its RDF list), so replacing a union range leaves no orphans."""
    seen: set[str] = set()
    stack = [bnode.value]
    to_remove = []
    while stack:
        cur = stack.pop()
        if cur in seen:
            continue
        seen.add(cur)
        for s, pr, o in store.read_triples(graph_iri):
            if isinstance(s, BlankNode) and s.value == cur:
                to_remove.append((s, pr, o))
                if isinstance(o, BlankNode):
                    stack.append(o.value)
    store.remove_triples(graph_iri, to_remove)


def _set_property_union(graph_iri, base_iri, p):
    """Set a property's domain/range to an anonymous owl:unionOf of the given classes."""
    node = NamedNode(p["iri"])
    if not (
        store.has_triple(graph_iri, node, RDF_TYPE, OWL_OBJECT_PROPERTY)
        or store.has_triple(graph_iri, node, RDF_TYPE, OWL_DATATYPE_PROPERTY)
    ):
        raise EditError(f"Property not found: {p['iri']}")
    slot = p.get("slot", "range")
    if slot not in {"domain", "range"}:
        raise EditError("slot must be domain or range")
    if slot == "range" and not _prop_is_object(graph_iri, node):
        raise EditError("data property ranges cannot be class unions")
    pred = RDFS_DOMAIN if slot == "domain" else RDFS_RANGE
    member_values = list(dict.fromkeys(str(m).strip() for m in p.get("members", []) if str(m).strip()))
    members = [
        _class_node(graph_iri, base_iri, value, create=False) for value in member_values
    ]
    if len(members) < 2:
        raise EditError("union needs at least two members")

    # Drop the old slot values, GC-ing any previous union expression.
    for old in store.object_terms(graph_iri, node, pred):
        if isinstance(old, BlankNode):
            _gc_blank(graph_iri, old)
    store.remove_pattern(graph_iri, node, pred, None)

    # Build:  _:u a owl:Class ; owl:unionOf ( C1 C2 ... ) .   node pred _:u .
    seed = "\0".join((node.value, slot, *sorted(member.value for member in members)))
    token = hashlib.sha256(seed.encode("utf-8")).hexdigest()
    union = BlankNode(f"union-{token}")
    triples = [(union, RDF_TYPE, OWL_CLASS)]
    cells = [BlankNode(f"union-{token}-{index}") for index in range(len(members))]
    for k, cell in enumerate(cells):
        triples.append((cell, RDF_FIRST, members[k]))
        triples.append((cell, RDF_REST, cells[k + 1] if k + 1 < len(cells) else RDF_NIL))
    triples.append((union, OWL_UNION_OF, cells[0]))
    triples.append((node, pred, union))
    store.add_triples(graph_iri, triples)
    return node.value


def _merge_classes(graph_iri, base_iri, p):
    """Merge `source` into `target`: repoint all references, drop source."""
    source = NamedNode(p["source"])
    target = NamedNode(p["target"])
    if source.value == target.value:
        raise EditError("Cannot merge a class into itself")
    if not store.has_triple(graph_iri, source, RDF_TYPE, OWL_CLASS):
        raise EditError(f"Class not found: {source.value}")
    if not store.has_triple(graph_iri, target, RDF_TYPE, OWL_CLASS):
        raise EditError(f"Class not found: {target.value}")
    keep_preds = {RDF_TYPE.value, RDFS_LABEL.value}
    symmetric = {RDFS_SUBCLASSOF.value, OWL_DISJOINT_WITH.value, OWL_EQUIVALENT_CLASS.value}

    new_triples = []
    for s, pr, o in store.read_triples(graph_iri):
        s2 = target if s.value == source.value else s
        o2 = target if isinstance(o, NamedNode) and o.value == source.value else o
        if s.value == source.value and pr.value in keep_preds:
            continue  # keep target's own type/label, drop source's
        if s.value == source.value or (isinstance(o, NamedNode) and o.value == source.value):
            # a triple that referenced source -> repoint to target
            if pr.value in symmetric and isinstance(o2, NamedNode) and s2.value == o2.value:
                continue  # avoid self-loop like target subClassOf target
            new_triples.append((s2, pr, o2))
    store.add_triples(graph_iri, new_triples)
    store.remove_entity(graph_iri, source.value)
    # ABox: a class appears there only as the object of rdf:type, so re-type the source class's
    # individuals onto the target — otherwise merging a duplicate class (incl. the conflict agent's
    # auto-merge) would strand its instances on a class that no longer exists.
    abox_iri = _abox_iri(graph_iri)
    a_add, a_remove = [], []
    for s, pr, o in store.read_triples(abox_iri):
        if isinstance(o, NamedNode) and o.value == source.value:
            a_remove.append((s, pr, o)); a_add.append((s, pr, target))
    if a_remove:
        store.remove_triples(abox_iri, a_remove)
        store.add_triples(abox_iri, a_add)
    return target.value


def _prop_target(graph_iri, base_iri, p, forbidden: set[str] | None = None):
    """Resolve/create the general target object property (by iri or label).

    ``forbidden`` (the source IRIs) is checked BEFORE any write, so a rejected merge/subordinate
    (target == a source) never mutates the graph — otherwise the target's owl:ObjectProperty
    declaration would already have landed, mistyping e.g. a data property as both data and object."""
    label = (p.get("target_label") or "").strip()
    if p.get("target"):
        target = NamedNode(p["target"])
        if not store.has_triple(graph_iri, target, RDF_TYPE, OWL_OBJECT_PROPERTY):
            if store.has_triple(graph_iri, target, RDF_TYPE, OWL_DATATYPE_PROPERTY):
                raise EditError(f"Object property required, found data property: {target.value}")
            raise EditError(f"Object property not found: {target.value}")
    elif label:
        existing = schema.read_index(graph_iri).prop_by_norm.get(norm_label(label))
        target = NamedNode(existing) if existing else NamedNode(base_iri + property_local_name(label))
        if existing and not store.has_triple(graph_iri, target, RDF_TYPE, OWL_OBJECT_PROPERTY):
            raise EditError(f"Object property required, found data property: {target.value}")
    else:
        raise EditError("needs target or target_label")
    if forbidden and target.value in forbidden:
        raise EditError("target cannot be one of the sources")
    store.add_triples(graph_iri, [(target, RDF_TYPE, OWL_OBJECT_PROPERTY)])
    if label and not store.has_triple(graph_iri, target, RDFS_LABEL, None):
        store.add_triples(graph_iri, [(target, RDFS_LABEL, Literal(label))])
    return target


def _union_slot(graph_iri, base_iri, target, pred, slot, collected):
    """Set target's domain/range to target's current values ∪ `collected` (union if 2+)."""
    cur = {o.value for o in store.object_terms(graph_iri, target, pred) if isinstance(o, NamedNode)}
    # ``object_terms`` sees an anonymous union only as its blank root.  Include the
    # flattened members exposed by the curated view so extending an existing union
    # does not silently discard its current classes.
    for key in ("object_properties", "data_properties"):
        entry = next(
            (item for item in schema.build_view(graph_iri)[key] if item["iri"] == target.value),
            None,
        )
        if entry is not None:
            cur.update(entry.get(f"{slot}_members", []))
            break
    members = sorted(cur | collected)
    if len(members) == 1:
        _set_single(graph_iri, target, pred, NamedNode(members[0]))
    elif len(members) >= 2:
        _set_property_union(graph_iri, base_iri, {"iri": target.value, "slot": slot, "members": members})


def _merge_properties(graph_iri, base_iri, p):
    """Collapse over-specialized object properties (e.g. 拥有井/拥有计量站) into one general property
    (拥有): repoint every triple that uses a source AS ITS PREDICATE to the target, union the
    sources' domains/ranges onto the target, then drop the sources."""
    src_vals = {s for s in p.get("sources", []) if s}
    if not src_vals:
        raise EditError("merge_properties needs sources")
    for source in src_vals:
        node = NamedNode(source)
        if not store.has_triple(graph_iri, node, RDF_TYPE, OWL_OBJECT_PROPERTY):
            if store.has_triple(graph_iri, node, RDF_TYPE, OWL_DATATYPE_PROPERTY):
                raise EditError(f"Object property required, found data property: {source}")
            raise EditError(f"Object property not found: {source}")
    target = _prop_target(graph_iri, base_iri, p, forbidden=src_vals)

    # Capture concrete members before removing source definitions.  A union-valued
    # slot points to a BlankNode, so harvesting only NamedNode objects loses its
    # domain/range semantics during a merge.
    source_slots: dict[str, dict[str, set[str]]] = {}
    current_view = schema.build_view(graph_iri)
    for key in ("object_properties", "data_properties"):
        for item in current_view[key]:
            if item["iri"] in src_vals:
                source_slots[item["iri"]] = {
                    "domain": set(item.get("domain_members", [])),
                    "range": set(item.get("range_members", [])),
                }

    to_add, to_remove = [], []
    domains, ranges = set(), set()
    blank_slots: set[BlankNode] = set()
    for s, pr, o in store.read_triples(graph_iri):
        if pr.value in src_vals:  # a USAGE triple → repoint its predicate to target
            to_remove.append((s, pr, o)); to_add.append((s, target, o))
        elif s.value in src_vals:  # a source property's own definition triple → drop, harvest d/r
            to_remove.append((s, pr, o))
            if pr.value == RDFS_DOMAIN.value:
                domains.update(source_slots.get(s.value, {}).get("domain", set()))
                if isinstance(o, BlankNode):
                    blank_slots.add(o)
            elif pr.value == RDFS_RANGE.value:
                ranges.update(source_slots.get(s.value, {}).get("range", set()))
                if isinstance(o, BlankNode):
                    blank_slots.add(o)
        elif isinstance(o, NamedNode) and o.value in src_vals:  # rare: source referenced as object
            to_remove.append((s, pr, o))
    store.remove_triples(graph_iri, to_remove)
    store.add_triples(graph_iri, to_add)
    for blank in blank_slots:
        if not any(
            isinstance(obj, BlankNode) and obj.value == blank.value
            for _, _, obj in store.read_triples(graph_iri)
        ):
            _gc_blank(graph_iri, blank)

    # Object properties are USED as predicates in the ABox graph — repoint those too, or the
    # instance assertions would dangle on a deleted property.
    abox_iri = graph_iri.rstrip("/") + "/abox"
    a_add, a_remove = [], []
    for s, pr, o in store.read_triples(abox_iri):
        if pr.value in src_vals:
            a_remove.append((s, pr, o)); a_add.append((s, target, o))
    if a_remove:
        store.remove_triples(abox_iri, a_remove)
        store.add_triples(abox_iri, a_add)

    _union_slot(graph_iri, base_iri, target, RDFS_DOMAIN, "domain", domains)
    _union_slot(graph_iri, base_iri, target, RDFS_RANGE, "range", ranges)
    return target.value


def _subordinate_properties(graph_iri, base_iri, p):
    """Keep the specialized object properties but declare each rdfs:subPropertyOf a general one
    (created if needed), whose domain/range is the union of the sources'."""
    sources = [s for s in p.get("sources", []) if s]
    if not sources:
        raise EditError("subordinate_properties needs sources")
    for source in sources:
        node = NamedNode(source)
        if not store.has_triple(graph_iri, node, RDF_TYPE, OWL_OBJECT_PROPERTY):
            if store.has_triple(graph_iri, node, RDF_TYPE, OWL_DATATYPE_PROPERTY):
                raise EditError(f"Object property required, found data property: {source}")
            raise EditError(f"Object property not found: {source}")
    target = _prop_target(graph_iri, base_iri, p, forbidden=set(sources))
    domains, ranges = set(), set()
    for s in sources:
        node = NamedNode(s)
        domains |= {o.value for o in store.object_terms(graph_iri, node, RDFS_DOMAIN) if isinstance(o, NamedNode)}
        ranges |= {o.value for o in store.object_terms(graph_iri, node, RDFS_RANGE) if isinstance(o, NamedNode)}
        store.add_triples(graph_iri, [(node, RDFS_SUBPROPERTYOF, target)])
    _union_slot(graph_iri, base_iri, target, RDFS_DOMAIN, "domain", domains)
    _union_slot(graph_iri, base_iri, target, RDFS_RANGE, "range", ranges)
    return target.value


_OPS = {
    "add_class": _add_class,
    "update_class": _update_class,
    "delete_class": _delete_class,
    "add_property": _add_property,
    "update_property": _update_property,
    "delete_property": _delete_property,
    "add_axiom": _add_axiom,
    "delete_axiom": _delete_axiom,
    "merge_classes": _merge_classes,
    "set_property_union": _set_property_union,
    "merge_properties": _merge_properties,
    "subordinate_properties": _subordinate_properties,
}


def apply_edit(graph_iri: str, base_iri: str, op: dict) -> str:
    name = op.get("op")
    fn = _OPS.get(name)
    if not fn:
        raise EditError(f"Unknown edit op: {name}")
    return fn(graph_iri, base_iri, op)
