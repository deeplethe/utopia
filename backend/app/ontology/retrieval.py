"""Ontology retrieval tools (shared by retrieval-augmented and agentic extraction).

- ``search_ontology`` — vector search over the KS's existing classes/properties so
  extraction sees only the *relevant* slice of a large ontology, not all of it.
- ``get_neighborhood`` — graph lookup of a class's structural context (parents,
  children, properties, disjoint/equivalent) so new concepts can be attached correctly.

Both return plain JSON-serializable data: retrieval-augmented extraction formats it into
the prompt; the agentic loop feeds it back as tool results.

Entity embeddings are cached in-memory per named graph and refreshed incrementally
(only new/changed entities are re-embedded), reusing the OpenRouter embedding backend.
"""
from __future__ import annotations

import threading

from app.ontology import embeddings, schema

# graph_iri -> { entity_iri: (embedding_text, vector) }
_cache: dict[str, dict[str, tuple[str, object]]] = {}
_cache_lock = threading.Lock()  # concurrent extraction workers touch the cache from threads


def _entity_text(label: str, comment: str) -> str:
    return f"{label} — {comment}" if comment else label


def _entities(view: dict) -> list[dict]:
    ents: list[dict] = []
    superof = {c["iri"]: [view["labels"].get(s, s) for s in c["superclasses"]] for c in view["classes"]}
    for c in view["classes"]:
        ents.append({
            "iri": c["iri"], "label": c["label"], "comment": c["comment"],
            "kind": "class", "superclasses": superof.get(c["iri"], []),
        })
    for p in view["object_properties"]:
        ents.append({
            "iri": p["iri"], "label": p["label"], "comment": p["comment"], "kind": "object_property",
            "domain": p["domain_label"], "range": p["range_label"],
        })
    for p in view["data_properties"]:
        ents.append({
            "iri": p["iri"], "label": p["label"], "comment": p["comment"], "kind": "data_property",
            "domain": p["domain_label"], "range": p["range_label"],
        })
    return ents


def _ensure_vecs(graph_iri: str, ents: list[dict]):
    """Embed any entity whose text isn't already cached; return a snapshot of the cache."""
    with _cache_lock:
        cache = _cache.setdefault(graph_iri, {})
        todo = [e for e in ents if cache.get(e["iri"], ("", None))[0] != _entity_text(e["label"], e["comment"])]
    if todo:
        vecs = embeddings.embed([_entity_text(e["label"], e["comment"]) for e in todo])
        if vecs is not None:
            with _cache_lock:
                for e, v in zip(todo, vecs):
                    _cache.setdefault(graph_iri, {})[e["iri"]] = (_entity_text(e["label"], e["comment"]), v)
    with _cache_lock:
        return dict(_cache.get(graph_iri, {}))


def search_ontology(graph_iri: str, query: str, k: int = 10) -> list[dict]:
    """Return up to k existing entities most semantically relevant to the query text."""
    view = schema.build_view(graph_iri)
    ents = _entities(view)
    if not ents:
        return []
    cache = _ensure_vecs(graph_iri, ents)
    qv = embeddings.embed([query])
    if qv is None or not cache:
        return []
    import numpy as np

    q = qv[0]
    scored = []
    for e in ents:
        entry = cache.get(e["iri"])
        if entry is None or entry[1] is None:
            continue
        scored.append((float(np.dot(q, entry[1])), e))
    scored.sort(key=lambda x: -x[0])
    out = []
    for score, e in scored[:k]:
        out.append({**{key: v for key, v in e.items() if key != "iri"}, "score": round(score, 3)})
    return out


def get_neighborhood(graph_iri: str, target: str) -> dict | None:
    """Structural context of a class, looked up by label (case-insensitive) or IRI."""
    view = schema.build_view(graph_iri)
    t = target.strip().lower()
    cls = next((c for c in view["classes"] if c["label"].lower() == t or c["iri"] == target), None)
    if not cls:
        return None
    iri = cls["iri"]
    lbl = view["labels"].get

    subclasses = [lbl(r["sub"], r["sub"]) for r in view["axioms"]["subclass_of"] if r["super"] == iri]
    disjoint = [lbl(r["b"] if r["a"] == iri else r["a"]) for r in view["axioms"]["disjoint_with"] if iri in (r["a"], r["b"])]
    equivalent = [lbl(r["b"] if r["a"] == iri else r["a"]) for r in view["axioms"]["equivalent_class"] if iri in (r["a"], r["b"])]
    props_out = [{"label": p["label"], "range": p["range_label"]}
                 for p in view["object_properties"] + view["data_properties"] if p["domain"] == iri]
    props_in = [{"label": p["label"], "domain": p["domain_label"]}
                for p in view["object_properties"] if p["range"] == iri]

    return {
        "label": cls["label"],
        "comment": cls["comment"],
        "superclasses": [lbl(s, s) for s in cls["superclasses"]],
        "subclasses": subclasses,
        "properties_out": props_out,
        "properties_in": props_in,
        "disjoint_with": disjoint,
        "equivalent_class": equivalent,
    }


def invalidate(graph_iri: str) -> None:
    _cache.pop(graph_iri, None)
