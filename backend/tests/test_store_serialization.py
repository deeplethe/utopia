from __future__ import annotations

import pytest
from pyoxigraph import Literal, NamedNode, Store

from app.ontology import store


@pytest.mark.parametrize("fmt", store.EXPORT_FORMATS)
def test_serialize_graph_supports_language_tagged_literals(fmt: str, monkeypatch) -> None:
    graph_iri = "urn:test:vocabulary"
    oxigraph = Store()
    monkeypatch.setattr(store, "_store", oxigraph)
    store.add_triples(
        graph_iri,
        [(NamedNode("urn:test:concept"), NamedNode("urn:test:label"), Literal("Pump", language="en"))],
    )

    serialized = store.serialize_graph(graph_iri, fmt)

    assert "Pump" in serialized
