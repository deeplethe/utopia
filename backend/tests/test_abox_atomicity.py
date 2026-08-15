from __future__ import annotations

import pytest
from fastapi import HTTPException
from pyoxigraph import Literal, NamedNode, Store
from sqlalchemy.pool import StaticPool
from sqlmodel import Session, SQLModel, create_engine, select

from app.api import abox as abox_api
from app.db.models import AboxProvenance, AuditEvent, KnowledgeSystem, User, ValidationDecision
from app.ontology import (
    abox_provenance, editor, statement_provenance, store, validation_agent, workbench,
)
from app.ontology.vocab import OWL_NAMED_INDIVIDUAL, RDF_TYPE


@pytest.fixture
def workspace(monkeypatch):
    database = create_engine(
        "sqlite://",
        connect_args={"check_same_thread": False},
        poolclass=StaticPool,
    )
    SQLModel.metadata.create_all(database)
    monkeypatch.setattr(store, "_store", Store())
    store._graph_locks.clear()
    store._recorders.clear()
    with Session(database, expire_on_commit=False) as session:
        user = User(username="editor", password_hash="unused")
        session.add(user)
        session.commit()
        ks = KnowledgeSystem(
            name="ABox atomicity",
            owner_id=user.id,
            graph_iri="urn:abox-atomic:tbox",
            base_iri="urn:abox-atomic:onto:",
        )
        session.add(ks)
        session.commit()
        session.refresh(ks)
        yield session, user, ks


def _fixture_graph(ks: KnowledgeSystem) -> tuple[str, str, str]:
    cls = editor.apply_edit(ks.graph_iri, ks.base_iri, {"op": "add_class", "label": "Device"})
    prop = editor.apply_edit(ks.graph_iri, ks.base_iri, {
        "op": "add_property", "kind": "data", "label": "serial",
        "domain": cls, "range": "string",
    })
    subject = "urn:abox-atomic:individual:device"
    # The API derives its ABox IRI from the persisted KS id/GRAPH_ROOT, while the
    # generic workbench helper pairs arbitrary test graph IRIs by suffix.
    store.add_triples(abox_api.abox_iri_for(ks), [
        (NamedNode(subject), RDF_TYPE, OWL_NAMED_INDIVIDUAL),
        (NamedNode(subject), RDF_TYPE, NamedNode(cls)),
    ])
    return subject, cls, prop


def test_add_assertion_rejects_new_structural_error_without_sql_or_rdf(workspace, monkeypatch) -> None:
    session, user, ks = workspace
    subject, _cls, prop = _fixture_graph(ks)
    abox_iri = abox_api.abox_iri_for(ks)
    before = set(store.read_triples(abox_iri))
    calls = 0

    monkeypatch.setattr(abox_api.workbench, "structural_error_signatures", lambda _graph: set())

    def errors(_graph, _baseline):
        nonlocal calls
        calls += 1
        return ["abox:new-error"]

    monkeypatch.setattr(abox_api.workbench, "new_structural_errors", errors)

    with pytest.raises(HTTPException) as exc_info:
        abox_api.add_assertion(
            abox_api.Assertion(subject=subject, prop=prop, kind="data", value="SN-42"),
            ks, user, session,
        )

    assert exc_info.value.status_code == 422
    assert calls == 1
    assert set(store.read_triples(abox_iri)) == before
    assert session.exec(select(AuditEvent)).all() == []
    assert session.exec(select(AboxProvenance)).all() == []


def test_remove_assertion_provenance_failure_rolls_back_rdf_and_sql(workspace, monkeypatch) -> None:
    session, user, ks = workspace
    subject, _cls, prop = _fixture_graph(ks)
    abox_iri = abox_api.abox_iri_for(ks)
    triple = (NamedNode(subject), NamedNode(prop), Literal("SN-42"))
    store.add_triples(abox_iri, [triple])
    key = abox_provenance.data_key(subject, prop, "SN-42")
    session.add(AboxProvenance(knowledge_system_id=ks.id, fact_key=key, chunk_id=101))
    session.commit()

    def fail(*_args, **_kwargs):
        raise RuntimeError("injected provenance failure")

    monkeypatch.setattr(statement_provenance, "record_abox_diff", fail)

    with pytest.raises(RuntimeError, match="injected provenance failure"):
        abox_api.remove_assertion(
            abox_api.Assertion(subject=subject, prop=prop, kind="data", value="SN-42"),
            ks, user, session,
        )

    assert store.has_triple(abox_iri, *triple)
    assert session.exec(select(AuditEvent)).all() == []
    rows = session.exec(select(AboxProvenance)).all()
    assert [(row.fact_key, row.chunk_id) for row in rows] == [(key, 101)]


def test_remove_assertion_noop_does_not_write_audit_or_erase_provenance(workspace) -> None:
    session, user, ks = workspace
    subject, _cls, prop = _fixture_graph(ks)
    key = abox_provenance.data_key(subject, prop, "already-gone")
    session.add(AboxProvenance(knowledge_system_id=ks.id, fact_key=key, chunk_id=202))
    session.commit()

    abox_api.remove_assertion(
        abox_api.Assertion(subject=subject, prop=prop, kind="data", value="already-gone"),
        ks, user, session,
    )

    assert session.exec(select(AuditEvent)).all() == []
    rows = session.exec(select(AboxProvenance)).all()
    assert [(row.fact_key, row.chunk_id) for row in rows] == [(key, 202)]


def test_relax_range_sql_failure_restores_tbox_and_decision(workspace, monkeypatch) -> None:
    session, user, ks = workspace
    cls = editor.apply_edit(ks.graph_iri, ks.base_iri, {"op": "add_class", "label": "Device"})
    prop = editor.apply_edit(ks.graph_iri, ks.base_iri, {
        "op": "add_property", "kind": "data", "label": "pressure",
        "domain": cls, "range": "integer",
    })
    before = set(store.read_triples(ks.graph_iri))
    original_commit = session.commit

    def fail_commit():
        raise RuntimeError("injected SQL commit failure")

    monkeypatch.setattr(session, "commit", fail_commit)
    with pytest.raises(RuntimeError, match="injected SQL commit failure"):
        abox_api.fix_violation(
            abox_api.FixRequest(op={
                "kind": "relax_range", "prop": prop,
                "prop_label": "pressure", "xsd": "integer",
            }),
            ks, user, session,
        )
    monkeypatch.setattr(session, "commit", original_commit)

    assert set(store.read_triples(ks.graph_iri)) == before
    assert session.exec(select(AuditEvent)).all() == []
    assert session.exec(select(ValidationDecision)).all() == []


def test_validation_agent_remove_failure_restores_rdf_and_has_no_side_effects(
    workspace, monkeypatch,
) -> None:
    session, _user, ks = workspace
    subject, _cls, prop = _fixture_graph(ks)
    abox_iri = abox_api.abox_iri_for(ks)
    bad = (NamedNode(subject), NamedNode(prop), Literal("noise"))
    store.add_triples(abox_iri, [bad])
    stats = [{
        "prop": prop,
        "label": "serial",
        "xsd": "integer",
        "bad": [{"subject": subject, "value": "noise", "datatype": None}],
        "total": 1,
        "sample_values": ["noise"],
        "bad_values": ["noise"],
    }]
    monkeypatch.setattr(validation_agent.settings, "agentic_validation", True)
    monkeypatch.setattr(validation_agent.abox_validate, "datatype_stats", lambda *_args: stats)
    monkeypatch.setattr(
        validation_agent,
        "_decide",
        lambda *_args: {"action": "remove", "confidence": 1.0, "reason": "noise"},
    )

    def fail(*_args, **_kwargs):
        raise RuntimeError("injected provenance failure")

    monkeypatch.setattr(validation_agent.statement_provenance, "record_abox_diff", fail)
    monkeypatch.setattr(validation_agent, "engine", session.get_bind())

    assert validation_agent.triage_bg(ks.id) == []
    assert store.has_triple(abox_iri, *bad)
    assert session.exec(select(AuditEvent)).all() == []
    assert session.exec(select(ValidationDecision)).all() == []
