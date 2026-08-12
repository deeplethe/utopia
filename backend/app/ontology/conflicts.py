"""Ontology conflict / contradiction detection.

Reads a knowledge system's named graph and flags issues an LLM extraction (or manual
editing) may have introduced. Each detected conflict carries a stable ``signature``
(so resolved/dismissed ones don't re-open) and a set of suggested ``resolutions`` —
each resolution is an editor op that ``editor.apply_edit`` can execute directly.

Detected types:
- cycle             : subclass cycle (A ⊑ … ⊑ A)
- disjoint_subclass : A ⊑ B while A ⟂ B (A unsatisfiable)
- disjoint_common   : X ⊑ A and X ⊑ B while A ⟂ B (X unsatisfiable)
- domain_multi      : a property has conflicting rdfs:domain values
- range_multi       : a property has conflicting rdfs:range values
- equiv_disjoint    : A ≡ B and A ⟂ B (direct contradiction)
- duplicate         : two classes with near-identical labels (likely the same concept)
"""
from __future__ import annotations

import difflib
import logging
from dataclasses import dataclass, field

from pyoxigraph import BlankNode

from app import prompt_config
from app.config import settings
from app.llm import openrouter
from app.ontology import store
from app.ontology.schema import _local

logger = logging.getLogger(__name__)
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
    RDFS_DOMAIN,
    RDFS_LABEL,
    RDFS_RANGE,
    RDFS_SUBCLASSOF,
    norm_label,
)

_DUPLICATE_SYSTEM = (
    "You compare pairs of class labels from ONE ontology. For each pair decide whether "
    "the two labels are SYNONYMS naming the SAME class (should be merged) or DIFFERENT "
    "classes. Treat siblings, part-of, general-vs-specific and merely-related terms as "
    "DIFFERENT. Be conservative: answer SAME only for genuinely interchangeable names."
)

prompt_config.register(
    key="conflict.duplicate_judge",
    category="review",
    title="Semantic duplicate judge",
    description="Judge whether similar class labels are true synonyms that should be merged.",
    default=_DUPLICATE_SYSTEM,
    order=40,
)

DUP_THRESHOLD = 0.86


def _compositional_distinct(la: str, lb: str) -> bool:
    """True when one label is a compound noun whose head differs from the other label.

    'Pump' vs 'Pump Station' → head 'Station' ≠ 'Pump' → distinct concepts (skip).
    'Pump' vs 'Water Pump'  → head 'Pump'   == 'Pump' → likely a refinement (keep).
    Only affects multi-word (space-tokenized) labels; single tokens fall through to the
    plain cosine threshold.
    """
    ta, tb = norm_label(la).split(), norm_label(lb).split()
    if not ta or not tb:
        return False
    short, long = (ta, tb) if len(ta) < len(tb) else (tb, ta)
    if len(short) < len(long) and set(short) <= set(long):
        return long[-1] not in short  # shorter lacks the head word → different concept
    return False


def _common_suffix(a: str, b: str) -> str:
    """Longest common trailing substring of a and b (the shared 'head noun' for CJK compounds)."""
    i = 0
    while i < len(a) and i < len(b) and a[-1 - i] == b[-1 - i]:
        i += 1
    return a[len(a) - i:] if i else ""


_UNINFORMATIVE_LATIN_STEMS = {
    "be", "is", "are", "was", "were", "has", "have", "had", "with",
}


def _specialization_stem(property_label: str, range_label: str) -> str | None:
    """Return a meaningful relation stem when the range name is a complete suffix.

    Latin labels are matched by normalized tokens rather than raw characters.  The old
    character suffix check treated ``has taint`` / ``Taint`` as stem ``has t`` because the
    initial letter differed in case, and then grouped unrelated properties by that fragment.
    CJK compounds keep the character-based behavior because a one-character range such as
    ``井`` can legitimately be baked into a predicate such as ``拥有井``.
    """
    property_norm = norm_label(property_label)
    range_norm = norm_label(range_label)
    if not property_norm or not range_norm:
        return None

    property_tokens = property_norm.split()
    range_tokens = range_norm.split()
    latin_like = any(char.isascii() and char.isalpha() for char in property_norm + range_norm)
    if latin_like:
        if len(property_tokens) <= len(range_tokens):
            return None
        if property_tokens[-len(range_tokens):] != range_tokens:
            return None
        stem = " ".join(property_tokens[:-len(range_tokens)]).strip()
        if stem in _UNINFORMATIVE_LATIN_STEMS:
            return None
        return stem or None

    suffix = _common_suffix(property_norm, range_norm)
    if not suffix or suffix != range_norm:
        return None
    stem = property_norm[:-len(suffix)].strip()
    return stem if len(stem) >= 2 else None


@dataclass
class DetectedConflict:
    signature: str
    ctype: str
    severity: str
    title: str
    detail: str
    entities: list[dict] = field(default_factory=list)
    resolutions: list[dict] = field(default_factory=list)


@dataclass
class _GraphModel:
    classes: set[str] = field(default_factory=set)
    labels: dict[str, str] = field(default_factory=dict)
    props: dict[str, dict] = field(default_factory=dict)  # iri -> {kind, domains:set, ranges:set}
    unions: dict[str, tuple[str, ...]] = field(default_factory=dict)
    subclass: list[tuple[str, str]] = field(default_factory=list)  # (sub, super)
    disjoint: set[frozenset] = field(default_factory=set)
    equivalent: set[frozenset] = field(default_factory=set)


def _read(graph_iri: str) -> _GraphModel:
    m = _GraphModel()
    union_heads: dict[str, str] = {}
    list_first: dict[str, str] = {}
    list_rest: dict[str, str] = {}
    for s, p, o in store.read_triples(graph_iri):
        si, pi = s.value, p.value
        if pi == RDF_TYPE.value:
            if o.value == OWL_CLASS.value:
                if not isinstance(s, BlankNode):  # skip anonymous union expressions
                    m.classes.add(si)
            elif o.value == OWL_OBJECT_PROPERTY.value:
                m.props.setdefault(si, {"kind": "object", "domains": set(), "ranges": set()})
            elif o.value == OWL_DATATYPE_PROPERTY.value:
                m.props.setdefault(si, {"kind": "data", "domains": set(), "ranges": set()})
        elif pi == RDFS_LABEL.value:
            m.labels[si] = o.value
        elif pi == RDFS_SUBCLASSOF.value:
            m.subclass.append((si, o.value))
        elif pi == RDFS_DOMAIN.value:
            m.props.setdefault(si, {"kind": "object", "domains": set(), "ranges": set()})["domains"].add(o.value)
        elif pi == RDFS_RANGE.value:
            m.props.setdefault(si, {"kind": "object", "domains": set(), "ranges": set()})["ranges"].add(o.value)
        elif pi == OWL_DISJOINT_WITH.value:
            m.disjoint.add(frozenset((si, o.value)))
        elif pi == OWL_EQUIVALENT_CLASS.value:
            m.equivalent.add(frozenset((si, o.value)))
        elif pi == OWL_UNION_OF.value:
            union_heads[si] = o.value
        elif pi == RDF_FIRST.value:
            list_first[si] = o.value
        elif pi == RDF_REST.value:
            list_rest[si] = o.value
    for union_iri, head in union_heads.items():
        members: list[str] = []
        current = head
        guard = 0
        while current and current != RDF_NIL.value and guard < 1000:
            member = list_first.get(current)
            if member:
                members.append(member)
            current = list_rest.get(current)
            guard += 1
        if members:
            m.unions[union_iri] = tuple(members)
    return m


def _lbl(m: _GraphModel, iri: str) -> str:
    if iri in m.unions:
        return " ∪ ".join(_lbl(m, member) for member in m.unions[iri])
    return m.labels.get(iri) or _local(iri)


def _ent(m: _GraphModel, iri: str) -> dict:
    return {"iri": iri, "label": _lbl(m, iri)}


def _concrete_values(m: _GraphModel, values: list[str] | set[str]) -> list[str]:
    concrete: list[str] = []
    seen: set[str] = set()
    for value in values:
        members = m.unions.get(value, (value,))
        for member in members:
            if member not in seen:
                seen.add(member)
                concrete.append(member)
    return concrete


def _ancestors(subclass: list[tuple[str, str]]) -> dict[str, set[str]]:
    adj: dict[str, set[str]] = {}
    for sub, sup in subclass:
        adj.setdefault(sub, set()).add(sup)
    anc: dict[str, set[str]] = {}

    def walk(x: str) -> set[str]:
        if x in anc:
            return anc[x]
        anc[x] = set()  # guard against cycles
        acc: set[str] = set()
        for sup in adj.get(x, ()):
            acc.add(sup)
            acc |= walk(sup)
        anc[x] = acc
        return acc

    for n in list(adj):
        walk(n)
    return anc


def _sccs(nodes: set[str], subclass: list[tuple[str, str]]) -> list[list[str]]:
    """Tarjan's SCC over the subclass digraph (small graphs; recursion is fine)."""
    adj: dict[str, list[str]] = {}
    for sub, sup in subclass:
        adj.setdefault(sub, []).append(sup)
    index: dict[str, int] = {}
    low: dict[str, int] = {}
    on_stack: set[str] = set()
    stack: list[str] = []
    counter = [0]
    out: list[list[str]] = []

    def strong(v: str):
        index[v] = low[v] = counter[0]
        counter[0] += 1
        stack.append(v)
        on_stack.add(v)
        for w in adj.get(v, ()):
            if w not in index:
                strong(w)
                low[v] = min(low[v], low[w])
            elif w in on_stack:
                low[v] = min(low[v], index[w])
        if low[v] == index[v]:
            comp = []
            while True:
                w = stack.pop()
                on_stack.discard(w)
                comp.append(w)
                if w == v:
                    break
            out.append(comp)

    for v in list(adj):
        if v not in index:
            strong(v)
    return out


def _llm_verify_duplicates(pairs: list[tuple[str, str]]) -> set[int]:
    """Given candidate (label_a, label_b) pairs, return indices the LLM judges to be the
    SAME concept. Fail-closed (empty set on any error) so a flaky call adds no noise."""
    if not pairs:
        return set()
    lines = "\n".join(f'{i}. "{a}" | "{b}"' for i, (a, b) in enumerate(pairs))
    user = f'Pairs:\n{lines}\n\nReturn ONLY JSON: {{"same": [indices that are synonyms]}}.'
    try:
        content = openrouter.chat_sync(
            [
                {"role": "system", "content": prompt_config.get("conflict.duplicate_judge")},
                {"role": "user", "content": user},
            ],
            temperature=0,
            max_tokens=500,
        )
        data = openrouter.extract_json(content)
        same = data.get("same", []) if isinstance(data, dict) else []
        return {int(i) for i in same if str(i).lstrip("-").isdigit()}
    except Exception as e:  # noqa: BLE001
        logger.warning("LLM duplicate verification failed (%s); skipping semantic flags", e)
        return set()


def detect(graph_iri: str, *, semantic: bool = True) -> list[DetectedConflict]:
    """Detect conflicts. ``semantic=False`` skips the embedding+LLM duplicate pass (which
    costs API calls) — used to keep manual edits snappy; structural checks always run."""
    m = _read(graph_iri)
    found: list[DetectedConflict] = []
    seen_sig: set[str] = set()

    def push(c: DetectedConflict):
        if c.signature in seen_sig:
            return
        seen_sig.add(c.signature)
        found.append(c)

    # --- subclass cycles ---
    edge_set = set(m.subclass)
    for comp in _sccs(m.classes, m.subclass):
        is_cycle = len(comp) > 1 or (comp[0], comp[0]) in edge_set
        if not is_cycle:
            continue
        comp_set = set(comp)
        intra = [(a, b) for (a, b) in m.subclass if a in comp_set and b in comp_set]
        sig = "cycle|" + "|".join(sorted(comp))
        res = [
            {
                "id": f"rm-{_local(a)}-{_local(b)}",
                "label": f"Delete subclass relation: {_lbl(m, a)} ⊑ {_lbl(m, b)}",
                "op": {"op": "delete_axiom", "type": "subclass", "sub": a, "super": b},
            }
            for (a, b) in intra
        ]
        push(DetectedConflict(
            signature=sig, ctype="cycle", severity="error",
            title="Subclass cycle",
            detail="These classes form a subclass cycle (each is a subclass of another); at least one subclass relation must be removed: " + ", ".join(_lbl(m, x) for x in comp),
            entities=[_ent(m, x) for x in comp], resolutions=res,
        ))

    anc = _ancestors(m.subclass)
    direct = set(m.subclass)

    # --- disjoint violations ---
    for pair in m.disjoint:
        a, b = tuple(pair)
        # A subclass-of B (or reverse) while disjoint
        for x, y in ((a, b), (b, a)):
            if y in anc.get(x, set()):
                sig = "disjoint_subclass|" + "|".join(sorted((a, b)))
                res = [{
                    "id": "rm-disjoint",
                    "label": f"Delete disjoint declaration: {_lbl(m, a)} ⟂ {_lbl(m, b)}",
                    "op": {"op": "delete_axiom", "type": "disjoint", "a": a, "b": b},
                }]
                if (x, y) in direct:
                    res.append({
                        "id": "rm-subclass",
                        "label": f"Delete subclass relation: {_lbl(m, x)} ⊑ {_lbl(m, y)}",
                        "op": {"op": "delete_axiom", "type": "subclass", "sub": x, "super": y},
                    })
                push(DetectedConflict(
                    signature=sig, ctype="disjoint_subclass", severity="error",
                    title="Disjointness conflict (subclass)",
                    detail=f"{_lbl(m, x)} is a (transitive) subclass of {_lbl(m, y)}, yet the two are declared disjoint, making {_lbl(m, x)} unsatisfiable.",
                    entities=[_ent(m, a), _ent(m, b)], resolutions=res,
                ))
                break

    # common subclass of two disjoint classes
    for x in m.classes:
        ax = anc.get(x, set())
        for pair in m.disjoint:
            a, b = tuple(pair)
            if x in (a, b):
                continue
            if a in ax and b in ax:
                sig = f"disjoint_common|{x}|" + "|".join(sorted((a, b)))
                res = [{
                    "id": "rm-disjoint",
                    "label": f"Delete disjoint declaration: {_lbl(m, a)} ⟂ {_lbl(m, b)}",
                    "op": {"op": "delete_axiom", "type": "disjoint", "a": a, "b": b},
                }]
                for tgt in (a, b):
                    if (x, tgt) in direct:
                        res.append({
                            "id": f"rm-sub-{_local(tgt)}",
                            "label": f"Delete subclass relation: {_lbl(m, x)} ⊑ {_lbl(m, tgt)}",
                            "op": {"op": "delete_axiom", "type": "subclass", "sub": x, "super": tgt},
                        })
                push(DetectedConflict(
                    signature=sig, ctype="disjoint_common", severity="error",
                    title="Disjointness conflict (common subclass)",
                    detail=f"{_lbl(m, x)} is a subclass of both disjoint classes {_lbl(m, a)} and {_lbl(m, b)}, making {_lbl(m, x)} unsatisfiable.",
                    entities=[_ent(m, x), _ent(m, a), _ent(m, b)], resolutions=res,
                ))

    # --- equivalent + disjoint contradiction ---
    for pair in m.equivalent & m.disjoint:
        a, b = tuple(pair)
        sig = "equiv_disjoint|" + "|".join(sorted((a, b)))
        push(DetectedConflict(
            signature=sig, ctype="equiv_disjoint", severity="error",
            title="Equivalent vs. disjoint contradiction",
            detail=f"{_lbl(m, a)} and {_lbl(m, b)} are declared both equivalent and disjoint — a direct contradiction.",
            entities=[_ent(m, a), _ent(m, b)],
            resolutions=[
                {"id": "rm-equivalent", "label": "Delete equivalent declaration",
                 "op": {"op": "delete_axiom", "type": "equivalent", "a": a, "b": b}},
                {"id": "rm-disjoint", "label": "Delete disjoint declaration",
                 "op": {"op": "delete_axiom", "type": "disjoint", "a": a, "b": b}},
            ],
        ))

    # --- domain / range multiplicity ---
    for iri, info in m.props.items():
        for slot, pred_vals in (("domain", info["domains"]), ("range", info["ranges"])):
            if len(pred_vals) < 2:
                continue
            # Drop values subsumed by a more-general one in the set: {抽油机, 设备} with 抽油机⊑设备
            # is just 设备 (union semantics) — redundant, not a conflict. Only genuinely unrelated
            # classes remain a real multi-domain/range.
            vals = sorted(v for v in pred_vals if not any(w != v and w in anc.get(v, set()) for w in pred_vals))
            if len(vals) < 2:
                continue
            ctype = "domain_multi" if slot == "domain" else "range_multi"
            slot_cn = "domain" if slot == "domain" else "range"
            concrete_vals = _concrete_values(m, vals)
            res = []
            for value in vals:
                operation = (
                    {
                        "op": "set_property_union",
                        "iri": iri,
                        "slot": slot,
                        "members": list(m.unions[value]),
                    }
                    if value in m.unions
                    else {"op": "update_property", "iri": iri, slot: value}
                )
                res.append({
                    "id": f"keep-{_local(value)}",
                    "label": f"Keep only {slot_cn} = {_lbl(m, value)}",
                    "op": operation,
                })

            # For class-valued slots (not XSD datatypes), offer semantically better fixes:
            # collapse to a shared superclass if one exists, or an owl:unionOf.
            if all("XMLSchema#" not in value for value in concrete_vals):
                common = set.intersection(*[anc.get(value, set()) for value in concrete_vals])
                if common:
                    s_super = max(common, key=lambda s: len(anc.get(s, set())))
                    res.append({
                        "id": f"super-{_local(s_super)}",
                        "label": f"Use common superclass {_lbl(m, s_super)}",
                        "op": {"op": "update_property", "iri": iri, slot: s_super},
                    })
                res.append({
                    "id": "union",
                    "label": f"Use union {slot_cn} ({' ∪ '.join(_lbl(m, value) for value in concrete_vals)})",
                    "op": {
                        "op": "set_property_union",
                        "iri": iri,
                        "slot": slot,
                        "members": concrete_vals,
                    },
                })

            push(DetectedConflict(
                signature=f"{ctype}|{iri}", ctype=ctype, severity="warning",
                title=f"{slot_cn.capitalize()} conflict",
                detail=(f'Property "{_lbl(m, iri)}" has multiple {slot_cn}s: '
                        f"{', '.join(_lbl(m, v) for v in vals)}. Multiple {slot_cn}s mean their "
                        f"intersection (all must hold), which is usually not intended — use a union, a common superclass, or keep just one."),
                entities=[_ent(m, iri)] + [_ent(m, v) for v in vals], resolutions=res,
            ))

    if not semantic:
        return found

    # --- over-specialized object predicates (拥有井 / 拥有计量站 → 拥有) ---
    # Families of object properties that share a prefix (verb) whose remainder is the object's
    # noun baked into the predicate. The signal: the property's label ends with (the head of) one
    # of its own range classes. Suggest merging into the general relation or subordinating it.
    obj_props = {iri: d for iri, d in m.props.items() if d.get("kind") == "object"}
    stem_of: dict[str, str] = {}
    for iri, d in obj_props.items():
        label = _lbl(m, iri)
        best_stem, best_len = None, 0
        for r in d["ranges"]:
            range_label = _lbl(m, r)
            stem = _specialization_stem(label, range_label)
            if stem and len(norm_label(range_label)) > best_len:
                best_stem, best_len = stem, len(norm_label(range_label))
        if best_stem:
            stem_of[iri] = best_stem
    families: dict[str, list[str]] = {}
    for iri, stem in stem_of.items():
        families.setdefault(stem, []).append(iri)
    for stem, members in families.items():
        if len(members) < 2:
            continue
        members = sorted(members)
        labels = [_lbl(m, i) for i in members]
        target_iri = next((i for i, d in obj_props.items() if _lbl(m, i) == stem), None)
        tgt = {"target": target_iri} if target_iri else {"target_label": stem}
        push(DetectedConflict(
            signature="predspec|" + stem + "|" + "|".join(members),
            ctype="predicate_specialization", severity="warning",
            title="Over-specialized relations",
            detail=(f'{", ".join(chr(34) + x + chr(34) for x in labels)} look like the general '
                    f'relation "{stem}" specialized by object type — the object\'s class already '
                    f'carries that. Merge into "{stem}", make them sub-properties, or dismiss.'),
            entities=[_ent(m, i) for i in members],
            resolutions=[
                {"id": "merge", "label": f'Merge into "{stem}"',
                 "op": {"op": "merge_properties", "sources": members, **tgt}},
                {"id": "subprop", "label": f'Sub-properties of "{stem}"',
                 "op": {"op": "subordinate_properties", "sources": members, "target_label": stem}},
            ],
        ))

    # --- duplicate / near-duplicate class labels ---
    # Skip pairs already related by subclass/equivalent/disjoint: those are deliberately
    # distinct (e.g. "Submersible Pump" ⊑ "Pump"), not accidental duplicates.
    def _related(a: str, b: str) -> bool:
        if b in anc.get(a, set()) or a in anc.get(b, set()):
            return True
        pair = frozenset((a, b))
        return pair in m.disjoint or pair in m.equivalent

    def _dup(a: str, b: str, detail: str) -> DetectedConflict:
        return DetectedConflict(
            signature="duplicate|" + "|".join(sorted((a, b))),
            ctype="duplicate", severity="warning", title="Possible duplicate classes", detail=detail,
            entities=[_ent(m, a), _ent(m, b)],
            resolutions=[
                {"id": "merge-a-into-b", "label": f"Merge: {_lbl(m, a)} → {_lbl(m, b)}",
                 "op": {"op": "merge_classes", "source": a, "target": b}},
                {"id": "merge-b-into-a", "label": f"Merge: {_lbl(m, b)} → {_lbl(m, a)}",
                 "op": {"op": "merge_classes", "source": b, "target": a}},
            ],
        )

    cls = sorted(m.classes)
    labels_list = [_lbl(m, iri) for iri in cls]

    def _eligible(i: int, j: int) -> bool:
        return not (_related(cls[i], cls[j]) or _compositional_distinct(labels_list[i], labels_list[j]))

    # Gather candidate pairs. Two cheap generators feed one shared LLM judge:
    #  - string similarity (plurals / typos / spacing)
    #  - embedding cosine (semantic near-duplicates string matching misses, e.g.
    #    Person/Human, 泵/水泵). Cosine conflates "related" with "same", so it is only a
    #    candidate generator; the LLM decides which candidates are truly one concept.
    cand: dict[tuple[int, int], float | None] = {}
    norms = [norm_label(l) for l in labels_list]  # was recomputed O(n^2) times inside the loop
    for i in range(len(cls)):
        la = norms[i]
        if not la:
            continue
        for j in range(i + 1, len(cls)):
            lb = norms[j]
            if not lb or not _eligible(i, j):
                continue
            # real_quick_ratio() >= quick_ratio() >= ratio(), so the cheap upper bounds gate the
            # expensive full ratio() without ever dropping a real match — same set, fewer full calls.
            sm = difflib.SequenceMatcher(None, la, lb)
            if (sm.real_quick_ratio() >= DUP_THRESHOLD and sm.quick_ratio() >= DUP_THRESHOLD
                    and sm.ratio() >= DUP_THRESHOLD):
                cand[(i, j)] = None

    if settings.enable_semantic_conflicts and len(cls) >= 2:
        from app.ontology import embeddings

        vecs = embeddings.embed(labels_list)
        if vecs is not None:
            import numpy as np

            V = np.asarray(vecs)
            sims = V @ V.T  # all pairwise cosines in one BLAS call instead of n^2 Python dot products
            thr = settings.semantic_candidate_threshold
            for i in range(len(cls)):
                row = sims[i]
                for j in range(i + 1, len(cls)):
                    in_cand = (i, j) in cand
                    if not in_cand and not _eligible(i, j):
                        continue
                    cos = float(row[j])
                    if in_cand or cos >= thr:
                        cand[(i, j)] = cos

    keys = list(cand)
    if keys and settings.verify_duplicates_with_llm:
        keep = _llm_verify_duplicates([(labels_list[i], labels_list[j]) for (i, j) in keys])
        keys = [k for idx, k in enumerate(keys) if idx in keep]

    for (i, j) in keys:
        cos = cand[(i, j)]
        note = f" (cosine {cos:.2f})" if isinstance(cos, float) else ""
        push(_dup(cls[i], cls[j], f'"{labels_list[i]}" and "{labels_list[j]}" look like the same concept{note}. Merge them, or dismiss.'))

    return found
