"""ABox validation: lint individuals against the TBox's semantic constraints.

Checks (computed on demand by scanning the ABox graph against the TBox):
  - placeholder: a non-identifying label such as "Untitled" was stored as an individual (error)
  - type_count : one individual accumulated an implausible number of direct types (error)
  - role       : one individual was merged across incompatible semantic roles (error)
  - unrelated_types: one individual has unrelated direct types with no shared identity role (warning)
  - disjoint  : an individual typed by two classes declared owl:disjointWith (error)
  - domain    : an individual uses a property whose domain class it isn't typed as (warning)
  - range     : an object-property target isn't typed as the property's range class (warning)
  - datatype  : a data-property value doesn't parse as the property's declared XSD type (warning)

Each violation carries one-click fixes (ABox operations: remove/add a type, drop an assertion).
Computed fresh each call (no persistence) so it never goes stale as the graph changes; for very
large ABoxes this should move to SPARQL and/or incremental checking (tracked as follow-up).
"""
from __future__ import annotations

import hashlib
import re

from pyoxigraph import Literal, NamedNode

from app.ontology import entity_roles, schema, store, vocab
from app.ontology.abox_extract import _is_non_identifying_label

MAX_VIOLATIONS = 500
MAX_DIRECT_TYPES = 12


def _sig(*parts: str) -> str:
    return hashlib.sha256("|".join(parts).encode()).hexdigest()[:16]


def _ancestors(supers: dict[str, list[str]], iri: str) -> set[str]:
    """Class iri + all its (transitive) superclasses."""
    out, stack = set(), [iri]
    while stack:
        cur = stack.pop()
        if cur in out:
            continue
        out.add(cur)
        stack.extend(supers.get(cur, []))
    return out


def _valid_xsd(value: str, xsd_local: str) -> bool:
    v = value.strip()
    if xsd_local == "integer":
        return re.fullmatch(r"[-+]?\d+", v) is not None
    if xsd_local in ("decimal", "float", "double"):
        try:
            float(v)
            return True
        except ValueError:
            return False
    if xsd_local == "boolean":
        return v.lower() in ("true", "false", "0", "1")
    return True  # string/date/dateTime — don't flag (avoid false positives)


def validate(graph_iri: str, abox_iri: str) -> dict:
    view = schema.build_view(graph_iri)
    labels = view["labels"]
    clabel = lambda iri: labels.get(iri, iri.rsplit("#", 1)[-1].rsplit("/", 1)[-1])  # noqa: E731

    supers = {c["iri"]: c["superclasses"] for c in view["classes"]}
    roles_by_class = entity_roles.class_role_map(view)
    disjoint = [(r["a"], r["b"]) for r in view["axioms"]["disjoint_with"]]
    prop_dr: dict[str, dict] = {}
    for kind, props in (("object", view["object_properties"]), ("data", view["data_properties"])):
        for p in props:
            prop_dr[p["iri"]] = {
                "kind": kind, "domain": p["domain"], "range": p["range"],
                "domain_members": p["domain_members"], "range_members": p["range_members"],
                "label": p["label"],
            }

    # Scan the ABox graph once.
    types: dict[str, set[str]] = {}          # individual iri -> direct class iris
    ind_labels: dict[str, str] = {}
    obj_assert: list[tuple[str, str, str]] = []
    data_assert: list[tuple[str, str, str, str | None]] = []
    for s, p, o in store.read_triples(abox_iri):
        if not isinstance(s, NamedNode):
            continue
        if p.value == vocab.RDF_TYPE.value and isinstance(o, NamedNode):
            if o.value != vocab.OWL_NAMED_INDIVIDUAL.value:
                types.setdefault(s.value, set()).add(o.value)
        elif p.value == vocab.RDFS_LABEL.value and isinstance(o, Literal):
            ind_labels[s.value] = o.value
        elif isinstance(o, NamedNode):
            obj_assert.append((s.value, p.value, o.value))
        elif isinstance(o, Literal):
            data_assert.append((s.value, p.value, o.value, o.datatype.value if o.datatype else None))

    ilabel = lambda iri: ind_labels.get(iri, iri.rsplit("ind-", 1)[-1])  # noqa: E731
    closure_cache: dict[str, set[str]] = {}

    def closure(ind: str) -> set[str]:
        if ind not in closure_cache:
            c: set[str] = set()
            for t in types.get(ind, ()):
                c |= _ancestors(supers, t)
            closure_cache[ind] = c
        return closure_cache[ind]

    violations: list[dict] = []
    seen_ids: set[str] = set()

    def add(v: dict) -> None:
        # De-dup by id: a subject using the same property for several values yields one
        # violation, not one per assertion (ids omit the object on purpose).
        if v["id"] in seen_ids or len(violations) >= MAX_VIOLATIONS:
            return
        seen_ids.add(v["id"])
        violations.append(v)

    # 1) identity-quality violations
    for ind, direct in types.items():
        label = ilabel(ind)
        if _is_non_identifying_label(label):
            add({
                "id": _sig("placeholder", ind),
                "type": "placeholder", "severity": "error",
                "individual": {"iri": ind, "label": label},
                "summary": f'"{label}" is a placeholder, not a stable individual identity.',
                "fixes": [{
                    "id": "delete", "label": "Delete this placeholder individual",
                    "op": {"kind": "delete_individual", "iri": ind},
                }],
            })
        if len(direct) > MAX_DIRECT_TYPES:
            add({
                "id": _sig("type_count", ind),
                "type": "type_count", "severity": "error",
                "individual": {"iri": ind, "label": label},
                "summary": (
                    f'"{label}" has {len(direct)} direct types, indicating that unrelated mentions '
                    "were probably merged into one individual."
                ),
                "fixes": [{
                    "id": "delete", "label": "Delete this over-merged individual",
                    "op": {"kind": "delete_individual", "iri": ind},
                }],
            })

        direct_roles: dict[str, list[str]] = {}
        if len(direct) > 1:
            for class_iri in direct:
                for role in roles_by_class.get(class_iri, ()):
                    direct_roles.setdefault(role, []).append(class_iri)
        if len(direct_roles) > 1:
            role_names = " / ".join(sorted(direct_roles))
            add({
                "id": _sig("role", ind, *sorted(direct_roles)),
                "type": "role", "severity": "error",
                "individual": {"iri": ind, "label": label},
                "summary": (
                    f'"{label}" is typed across incompatible roles ({role_names}); '
                    "same-name entities were probably merged."
                ),
                "fixes": [],
            })

        if len(direct) > 1 and len(direct_roles) <= 1:
            ordered = sorted(direct, key=clabel)
            unrelated: set[str] = set()
            for index, left in enumerate(ordered):
                left_closure = _ancestors(supers, left)
                left_roles = roles_by_class.get(left, frozenset())
                for right in ordered[index + 1:]:
                    right_closure = _ancestors(supers, right)
                    right_roles = roles_by_class.get(right, frozenset())
                    hierarchy_related = left in right_closure or right in left_closure
                    shared_role = bool(left_roles & right_roles)
                    if not hierarchy_related and not shared_role:
                        unrelated.update((left, right))
            if unrelated:
                names = ", ".join(f'"{clabel(class_iri)}"' for class_iri in sorted(unrelated, key=clabel))
                add({
                    "id": _sig("unrelated_types", ind, *sorted(unrelated)),
                    "type": "unrelated_types", "severity": "warning",
                    "individual": {"iri": ind, "label": label},
                    "summary": (
                        f'"{label}" has unrelated direct types {names}; same-name mentions may '
                        "have been merged into one individual."
                    ),
                    "fixes": [
                        {
                            "id": f"rm_{_sig(class_iri)}",
                            "label": f'Remove type "{clabel(class_iri)}"',
                            "op": {"kind": "remove_type", "iri": ind, "class_iri": class_iri},
                        }
                        for class_iri in sorted(unrelated, key=clabel)
                    ],
                })

    # 2) disjoint-type violations
    for ind, direct in types.items():
        cl = closure(ind)
        for a, b in disjoint:
            if a in cl and b in cl:
                ta = a if a in direct else next((t for t in direct if a in _ancestors(supers, t)), a)
                tb = b if b in direct else next((t for t in direct if b in _ancestors(supers, t)), b)
                add({
                    "id": _sig("disjoint", ind, a, b),
                    "type": "disjoint", "severity": "error",
                    "individual": {"iri": ind, "label": ilabel(ind)},
                    "summary": f'"{ilabel(ind)}" is typed as both "{clabel(a)}" and "{clabel(b)}", which are disjoint.',
                    "fixes": [
                        {"id": "rm_a", "label": f'Remove type "{clabel(ta)}"',
                         "op": {"kind": "remove_type", "iri": ind, "class_iri": ta}},
                        {"id": "rm_b", "label": f'Remove type "{clabel(tb)}"',
                         "op": {"kind": "remove_type", "iri": ind, "class_iri": tb}},
                    ],
                })

    # RDFS domain/range are INFERENCES, not hard constraints: "X uses P but isn't typed as P's
    # domain" only means a reasoner would infer that type — it is NOT a violation, and auto-
    # retyping X to the domain is usually wrong (and dangerous when the domain is over-narrow,
    # as concurrent TBox extraction tends to produce). We flag ONLY genuine contradictions: the
    # individual's actual type is DISJOINT with the property's domain/range. Fix = drop the
    # assertion (retyping would just create a disjoint-type conflict).
    def disjoint_types(a: str, b: str) -> bool:
        if a == b:
            return False
        aa, bb = _ancestors(supers, a), _ancestors(supers, b)
        return any((x in aa and y in bb) or (y in aa and x in bb) for x, y in disjoint)

    def conflicting_type(ind: str, target: str) -> str | None:
        return next((t for t in types.get(ind, ()) if disjoint_types(t, target)), None)

    def union_conflict(ind: str, members: list[str]) -> tuple[list[str], str] | None:
        """A property whose domain/range admits `members` (multiple rdfs:domain triples and/or an
        owl:unionOf) uses UNION semantics: the individual only contradicts it when it can be in
        NONE of the members — i.e. its type is disjoint from EVERY admissible class. Returns
        (class_members, a representative conflicting type) for the message, else None. This avoids
        false positives on multi-valued domains while still catching genuine contradictions that a
        single-value (last-writer) check would miss."""
        cls = [m for m in members if not m.startswith(vocab.XSD)]
        if not cls or any(m in closure(ind) for m in cls):
            return None
        conflicts = [conflicting_type(ind, m) for m in cls]
        if all(conflicts):
            return cls, conflicts[0]
        return None

    # 3) domain/range CONTRADICTIONS (object assertions)
    for s, p, o in obj_assert:
        dr = prop_dr.get(p)
        if not dr:
            continue
        if (dc := union_conflict(s, dr["domain_members"])):
            doms, ct = dc
            add({
                "id": _sig("domain", s, p), "type": "domain", "severity": "warning",
                "individual": {"iri": s, "label": ilabel(s)},
                "summary": f'"{ilabel(s)}" is typed "{clabel(ct)}" but uses "{dr["label"]}", whose domain ({" / ".join(clabel(d) for d in doms)}) is disjoint from "{clabel(ct)}".',
                "fixes": [{"id": "rm", "label": "Remove this relationship",
                           "op": {"kind": "remove_object_assertion", "subject": s, "prop": p, "target": o}}],
            })
        if (rc := union_conflict(o, dr["range_members"])):
            rngs, ct = rc
            add({
                "id": _sig("range", s, p, o), "type": "range", "severity": "warning",
                "individual": {"iri": o, "label": ilabel(o)},
                "summary": f'"{ilabel(o)}" is typed "{clabel(ct)}" but is the target of "{dr["label"]}", whose range ({" / ".join(clabel(r) for r in rngs)}) is disjoint from "{clabel(ct)}".',
                "fixes": [{"id": "rm", "label": "Remove this relationship",
                           "op": {"kind": "remove_object_assertion", "subject": s, "prop": p, "target": o}}],
            })

    # 4) domain CONTRADICTION + 5) datatype (data assertions)
    for s, p, value, dt in data_assert:
        dr = prop_dr.get(p)
        if not dr:
            continue
        if (dc := union_conflict(s, dr["domain_members"])):
            doms, ct = dc
            add({
                "id": _sig("domain", s, p), "type": "domain", "severity": "warning",
                "individual": {"iri": s, "label": ilabel(s)},
                "summary": f'"{ilabel(s)}" is typed "{clabel(ct)}" but uses "{dr["label"]}", whose domain ({" / ".join(clabel(d) for d in doms)}) is disjoint from "{clabel(ct)}".',
                "fixes": [{"id": "rm", "label": "Remove this attribute",
                           "op": {"kind": "remove_data_assertion", "subject": s, "prop": p, "value": value, "datatype": dt}}],
            })
        rng = dr["range"]
        if rng and rng.startswith(vocab.XSD):
            xsd_local = rng.rsplit("#", 1)[-1]
            if not _valid_xsd(value, xsd_local):
                add({
                    "id": _sig("datatype", s, p, value), "type": "datatype", "severity": "warning",
                    "individual": {"iri": s, "label": ilabel(s)},
                    "summary": f'"{ilabel(s)}": "{dr["label"]}" = "{value}" is not a valid {xsd_local}.',
                    "fixes": [
                        # If the value is a real qualitative descriptor, the property is mistyped —
                        # relaxing its range to text keeps the value and clears all its violations.
                        {"id": "relax", "label": f'Change "{dr["label"]}" to text',
                         "op": {"kind": "relax_range", "prop": p, "prop_label": dr["label"], "xsd": xsd_local}},
                        {"id": "rm", "label": "Remove this attribute",
                         "op": {"kind": "remove_data_assertion", "subject": s, "prop": p, "value": value, "datatype": dt}},
                    ],
                })

    order = {"error": 0, "warning": 1}
    violations.sort(key=lambda v: order.get(v["severity"], 2))
    counts = {"error": sum(1 for v in violations if v["severity"] == "error"),
              "warning": sum(1 for v in violations if v["severity"] == "warning")}
    return {"violations": violations, "counts": counts, "truncated": len(violations) >= MAX_VIOLATIONS}


_NUMERIC_XSD = {"integer", "decimal", "float", "double", "boolean"}


def datatype_stats(graph_iri: str, abox_iri: str) -> list[dict]:
    """For each numeric-typed data property that has any non-parsing value, return its value
    distribution: total count, the offending (subject, value) pairs, and a sample. The validation
    agent uses this to judge whether the property is really qualitative (relax its range to text)
    or just has a few noisy outliers (remove those)."""
    view = schema.build_view(graph_iri)
    numeric: dict[str, tuple[str, str]] = {}
    for p in view["data_properties"]:
        rng = p.get("range")
        if rng and rng.startswith(vocab.XSD):
            loc = rng.rsplit("#", 1)[-1]
            if loc in _NUMERIC_XSD:
                numeric[p["iri"]] = (p["label"], loc)

    vals: dict[str, list[tuple[str, str, str | None]]] = {}
    for s, pr, o in store.read_triples(abox_iri):
        if isinstance(o, Literal) and pr.value in numeric:
            vals.setdefault(pr.value, []).append((s.value, o.value, o.datatype.value if o.datatype else None))

    out: list[dict] = []
    for iri, (label, loc) in numeric.items():
        vs = vals.get(iri, [])
        bad = [{"subject": s, "value": v, "datatype": dt} for (s, v, dt) in vs if not _valid_xsd(v, loc)]
        if not bad:
            continue
        out.append({
            "prop": iri, "label": label, "xsd": loc, "total": len(vs), "bad": bad,
            "sample_values": [v for (_, v, _) in vs][:12],
            "bad_values": [b["value"] for b in bad][:12],
        })
    return out


def apply_fix(abox_iri: str, op: dict) -> None:
    """Apply one fix operation to the ABox graph (call inside store.capture for history)."""
    from app.ontology import abox

    kind = op.get("kind")
    if kind == "remove_type":
        abox.remove_type(abox_iri, op["iri"], op["class_iri"])
    elif kind == "delete_individual":
        abox.delete_individual(abox_iri, op["iri"])
    elif kind == "add_type":
        abox.add_type(abox_iri, op["iri"], op["class_iri"])
    elif kind == "remove_object_assertion":
        abox.remove_object_assertion(abox_iri, op["subject"], op["prop"], op["target"])
    elif kind == "remove_data_assertion":
        abox.remove_data_assertion(abox_iri, op["subject"], op["prop"], op["value"], op.get("datatype"))
    else:
        raise ValueError(f"Unknown fix op: {kind}")
