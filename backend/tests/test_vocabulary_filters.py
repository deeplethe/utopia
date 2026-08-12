from __future__ import annotations

from app.ontology import skos


def test_list_concepts_combines_mapping_origin_status_and_date_filters(monkeypatch) -> None:
    concepts = [
        {
            "iri": "urn:term:one",
            "scheme_iri": "urn:scheme",
            "status": "active",
            "origin": "agent",
            "mapped_entity_iri": None,
            "created_at": "2026-08-01T09:00:00+00:00",
            "modified_at": "2026-08-10T10:00:00+00:00",
            "description": "",
            "notation": "",
            "pref_labels": [{"value": "One", "language": "en"}],
            "alt_labels": [],
            "hidden_labels": [],
        },
        {
            "iri": "urn:term:two",
            "scheme_iri": "urn:scheme",
            "status": "active",
            "origin": "extraction",
            "mapped_entity_iri": "urn:class:two",
            "created_at": "2026-08-02T09:00:00+00:00",
            "modified_at": "2026-08-11T10:00:00+00:00",
            "description": "",
            "notation": "",
            "pref_labels": [{"value": "Two", "language": "en"}],
            "alt_labels": [],
            "hidden_labels": [],
        },
    ]
    monkeypatch.setattr(skos, "build_view", lambda _graph: {"concepts": concepts})

    result = skos.list_concepts(
        "urn:graph",
        status="active",
        mapping="standalone",
        origin="agent",
        start_date="2026-08-10",
        end_date="2026-08-10",
    )

    assert result == {"items": [concepts[0]], "total": 1}
