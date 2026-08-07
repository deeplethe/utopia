"""Oxigraph-backed RDF store wrapper.

One persistent embedded store lives under ``data/oxigraph``. Each knowledge system is
one *named graph* (identified by its ``graph_iri``). This module only does low-level
quad add/remove/read/serialize; ontology-level semantics live in ``schema.py``.
"""
from __future__ import annotations

import contextlib
import logging
import threading
from typing import Iterable

from pyoxigraph import BlankNode, Literal, NamedNode, Quad, RdfFormat, Store, parse, serialize

from app.config import settings

logger = logging.getLogger(__name__)

Triple = tuple[object, object, object]  # (subject, predicate, object) pyoxigraph terms

_store: Store | None = None
_lock = threading.Lock()

# --------------------------------------------------------------------------- #
# Change capture — records the exact triples added/removed to a graph during a
# `with capture(graph_iri) as cap:` block, so an operation can be reversed for rollback.
# A global (not thread-local) registry: extraction merges run in worker threads but are
# serialized by a write lock, and other graph edits are blocked while an extraction runs,
# so at most one logical operation writes to a given graph at a time.
# --------------------------------------------------------------------------- #
class _Recorder:
    def __init__(self, graph_iri: str) -> None:
        self.graph_iri = graph_iri
        self._orig: dict[str, tuple] = {}  # key -> (s, p, o, was_present_before)

    def _touch(self, s, p, o) -> None:
        key = f"{s}\t{p}\t{o}"
        if key not in self._orig:
            self._orig[key] = (s, p, o, has_triple(self.graph_iri, s, p, o))

    def _net(self) -> tuple[list, list]:
        added, removed = [], []
        for (s, p, o, was) in self._orig.values():
            now = has_triple(self.graph_iri, s, p, o)
            if was and not now:
                removed.append((s, p, o))
            elif not was and now:
                added.append((s, p, o))
        return added, removed

    def diff(self) -> tuple[bytes, bytes]:
        """Net added / removed triples (N-Triples bytes) vs. the pre-block state."""
        added, removed = self._net()
        return dump_triples(added), dump_triples(removed)

    def revert(self) -> None:
        """Undo this block's net changes in the store: drop what it added, restore what it removed.
        Used to leave a clean graph when an operation fails mid-way (writes here are immediate and
        non-transactional, so a SQL rollback alone would strand them)."""
        added, removed = self._net()
        if added:
            remove_triples(self.graph_iri, added)
        if removed:
            add_triples(self.graph_iri, removed)


_recorders: dict[str, _Recorder] = {}
_rec_lock = threading.Lock()
_graph_locks: dict[str, threading.Lock] = {}
_CAPTURE_TIMEOUT_S = 15.0


class CaptureBusy(RuntimeError):
    """Raised when another operation is already capturing writes to the same graph. Two
    concurrent captures on one graph would interleave their change records and corrupt the
    rollback diff, so the second writer is rejected (mapped to HTTP 409) instead of clobbering."""


def _graph_lock(graph_iri: str) -> threading.Lock:
    with _rec_lock:
        lk = _graph_locks.get(graph_iri)
        if lk is None:
            lk = _graph_locks[graph_iri] = threading.Lock()
        return lk


def _notify(graph_iri: str, s, p, o) -> None:
    rec = _recorders.get(graph_iri)
    if rec is not None:
        rec._touch(s, p, o)


@contextlib.contextmanager
def capture(graph_iri: str, *, revert_on_error: bool = False):
    """Serialize writers per graph for the whole ``with`` block. The first acquirer (e.g. a
    long extraction) proceeds immediately; a contender (a manual edit, resolve, rollback) waits
    briefly then raises :class:`CaptureBusy` rather than hang or clobber. Not re-entrant on one
    graph — nothing inside a capture opens another capture on the same graph.

    With ``revert_on_error=True``, if the block raises, the triples it wrote are undone before the
    lock is released — so a failed extraction job leaves a clean graph instead of orphaned,
    unrollbackable writes (graph writes are immediate/non-transactional; a SQL rollback won't touch
    them)."""
    rec = _Recorder(graph_iri)
    lock = _graph_lock(graph_iri)
    if not lock.acquire(timeout=_CAPTURE_TIMEOUT_S):
        raise CaptureBusy(f"another operation is writing to {graph_iri}")
    ok = False
    try:
        with _rec_lock:
            _recorders[graph_iri] = rec
        yield rec
        ok = True
    finally:
        # Deregister first so revert()'s own writes aren't re-recorded, then undo under the lock.
        with _rec_lock:
            if _recorders.get(graph_iri) is rec:
                del _recorders[graph_iri]
        if not ok and revert_on_error:
            try:
                rec.revert()
            except Exception:  # noqa: BLE001  (best-effort cleanup; never mask the original error)
                logger.exception("capture revert failed for %s", graph_iri)
        lock.release()


def dump_triples(triples: Iterable[Triple]) -> bytes:
    """Serialize triples to N-Triples (blank-node labels preserved for exact round-trip)."""
    quads = [Quad(s, p, o) for (s, p, o) in triples]
    if not quads:
        return b""
    return bytes(serialize(quads, format=RdfFormat.N_TRIPLES))


def load_triples(data: bytes) -> list[Triple]:
    if not data:
        return []
    return [(q.subject, q.predicate, q.object) for q in parse(data, format=RdfFormat.N_TRIPLES)]


def get_store() -> Store:
    global _store
    if _store is None:
        with _lock:
            if _store is None:
                _store = Store(str(settings.oxigraph_dir))
    return _store


def add_triples(graph_iri: str, triples: Iterable[Triple]) -> int:
    g = NamedNode(graph_iri)
    store = get_store()
    n = 0
    for s, p, o in triples:
        _notify(graph_iri, s, p, o)
        store.add(Quad(s, p, o, g))
        n += 1
    return n


def remove_triples(graph_iri: str, triples: Iterable[Triple]) -> int:
    g = NamedNode(graph_iri)
    store = get_store()
    n = 0
    for s, p, o in triples:
        _notify(graph_iri, s, p, o)
        store.remove(Quad(s, p, o, g))
        n += 1
    return n


def read_triples(graph_iri: str) -> list[Triple]:
    g = NamedNode(graph_iri)
    store = get_store()
    return [(q.subject, q.predicate, q.object) for q in store.quads_for_pattern(None, None, None, g)]


def object_terms(graph_iri: str, s, p) -> list:
    g = NamedNode(graph_iri)
    store = get_store()
    return [q.object for q in store.quads_for_pattern(s, p, None, g)]


def has_triple(graph_iri: str, s, p, o) -> bool:
    g = NamedNode(graph_iri)
    store = get_store()
    return any(True for _ in store.quads_for_pattern(s, p, o, g))


def remove_pattern(graph_iri: str, s=None, p=None, o=None) -> int:
    """Remove every triple matching the (s, p, o) pattern (None = wildcard)."""
    g = NamedNode(graph_iri)
    store = get_store()
    quads = list(store.quads_for_pattern(s, p, o, g))
    for q in quads:
        _notify(graph_iri, q.subject, q.predicate, q.object)
        store.remove(q)
    return len(quads)


def clear_graph(graph_iri: str) -> int:
    g = NamedNode(graph_iri)
    store = get_store()
    quads = list(store.quads_for_pattern(None, None, None, g))
    for q in quads:
        _notify(graph_iri, q.subject, q.predicate, q.object)
        store.remove(q)
    return len(quads)


def remove_entity(graph_iri: str, entity_iri: str) -> int:
    """Remove every triple where the entity appears as subject or object."""
    g = NamedNode(graph_iri)
    node = NamedNode(entity_iri)
    store = get_store()
    quads = list(store.quads_for_pattern(node, None, None, g))
    quads += list(store.quads_for_pattern(None, None, node, g))
    seen = set()
    n = 0
    for q in quads:
        key = (q.subject, q.predicate, q.object)
        if key in seen:
            continue
        seen.add(key)
        _notify(graph_iri, q.subject, q.predicate, q.object)
        store.remove(q)
        n += 1
    return n


# --------------------------------------------------------------------------- #
# Turtle export (via rdflib for a reliable, prefixed serialization)
# --------------------------------------------------------------------------- #
def _to_rdflib(term):
    import rdflib

    if isinstance(term, NamedNode):
        return rdflib.URIRef(term.value)
    if isinstance(term, BlankNode):
        return rdflib.BNode(term.value)
    if isinstance(term, Literal):
        dt = rdflib.URIRef(term.datatype.value) if term.datatype else None
        return rdflib.Literal(term.value, lang=term.language, datatype=dt)
    raise TypeError(f"Unexpected term type: {type(term)}")


def to_rdflib_graph(graph_iri: str):
    """Load the named graph into an in-memory rdflib.Graph (for SPARQL / validation)."""
    import rdflib

    from app.ontology import vocab

    g = rdflib.Graph()
    g.bind("owl", vocab.OWL)
    g.bind("rdfs", vocab.RDFS)
    g.bind("xsd", vocab.XSD)
    for s, p, o in read_triples(graph_iri):
        g.add((_to_rdflib(s), _to_rdflib(p), _to_rdflib(o)))
    return g


# Downloadable serializations: key -> (rdflib format, media type, file extension).
EXPORT_FORMATS: dict[str, tuple[str, str, str]] = {
    "turtle": ("turtle", "text/turtle", "ttl"),
    "rdfxml": ("xml", "application/rdf+xml", "rdf"),
    "ntriples": ("nt", "application/n-triples", "nt"),
    "jsonld": ("json-ld", "application/ld+json", "jsonld"),
}


def serialize_graph(graph_iri: str, fmt: str = "turtle") -> str:
    """Serialize a named graph in one of EXPORT_FORMATS (rdflib-backed, prefixed)."""
    rdflib_fmt = EXPORT_FORMATS[fmt][0]
    return to_rdflib_graph(graph_iri).serialize(format=rdflib_fmt)


def serialize_turtle(graph_iri: str) -> str:
    return serialize_graph(graph_iri, "turtle")
