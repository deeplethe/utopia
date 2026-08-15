from __future__ import annotations

import asyncio

import pytest
from pyoxigraph import NamedNode, Store
from sqlalchemy.pool import StaticPool
from sqlmodel import Session, SQLModel, create_engine, select

from app.api import extraction as extraction_api
from app.db.models import AuditEvent, Conflict, ExtractionJob, KnowledgeSystem, TboxReconciliation, User
from app.ontology import conflict_agent, editor, store, structure_agent, tbox_reconcile, workbench
from app.ontology.vocab import (
    OWL_DISJOINT_WITH,
    RDF_TYPE,
    RDFS_DOMAIN,
    RDFS_SUBCLASSOF,
)


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
        user = User(username="agent-test", password_hash="unused")
        session.add(user)
        session.commit()
        ks = KnowledgeSystem(
            name="Agent atomicity",
            public_id="agent-atomicity",
            owner_id=user.id,
            graph_iri="urn:agent:test:tbox",
            base_iri="urn:agent:test:onto:",
        )
        session.add(ks)
        session.commit()
        session.refresh(ks)
        yield session, ks


def test_conflict_agent_structural_gate_reverts_and_keeps_conflict_open(workspace, monkeypatch) -> None:
    session, ks = workspace
    left = editor.apply_edit(ks.graph_iri, ks.base_iri, {"op": "add_class", "label": "Left"})
    right = editor.apply_edit(ks.graph_iri, ks.base_iri, {"op": "add_class", "label": "Right"})
    store.add_triples(ks.graph_iri, [(NamedNode(left), OWL_DISJOINT_WITH, NamedNode(right))])
    conflict = Conflict(
        knowledge_system_id=ks.id,
        signature="agent-cycle",
        ctype="duplicate",
        severity="warning",
        status="open",
        title="Potential duplicate",
        payload={"resolutions": [{
            "id": "bad",
            "label": "Add contradictory hierarchy",
            "op": {"op": "add_axiom", "type": "subclass", "sub": left, "super": right},
        }]},
    )
    session.add(conflict)
    session.commit()
    before = workbench.ontology_revision(ks.graph_iri)
    monkeypatch.setattr(conflict_agent, "AUTO_APPLY_TYPES", {"duplicate"})
    monkeypatch.setattr(
        conflict_agent,
        "_decide",
        lambda *_args, **_kwargs: {"resolution": "bad", "confidence": 1.0, "reason": "test"},
    )

    assert conflict_agent._resolve(session, ks, None) == []
    assert workbench.ontology_revision(ks.graph_iri) == before
    session.refresh(conflict)
    assert conflict.status == "open"
    assert session.exec(select(AuditEvent)).all() == []


def test_structure_agent_sql_failure_reverts_rdf(workspace, monkeypatch) -> None:
    session, ks = workspace
    child = editor.apply_edit(ks.graph_iri, ks.base_iri, {"op": "add_class", "label": "Pump"})
    before = workbench.ontology_revision(ks.graph_iri)
    monkeypatch.setattr(structure_agent, "engine", session.get_bind())
    monkeypatch.setattr(
        structure_agent.schema,
        "build_view",
        lambda _graph: {
            "classes": [{"iri": child, "label": "Pump", "superclasses": []}],
            "object_properties": [],
            "data_properties": [],
        },
    )
    monkeypatch.setattr(
        structure_agent,
        "_decide",
        lambda *_args, **_kwargs: {
            "parent": "Equipment", "new": True, "confidence": 1.0,
            "evidence": "Pump is equipment", "reason": "test", "verified": True,
        },
    )
    original_commit = Session.commit

    def fail_agent_commit(current: Session) -> None:
        if any(
            item.action == "tbox.attach_isolated"
            for item in current.new
            if isinstance(item, AuditEvent)
        ):
            raise RuntimeError("database failure")
        original_commit(current)

    monkeypatch.setattr(Session, "commit", fail_agent_commit)

    assert structure_agent.attach_isolated_bg(ks.id) == []
    assert workbench.ontology_revision(ks.graph_iri) == before
    assert not store.has_triple(
        ks.graph_iri, NamedNode(child), RDFS_SUBCLASSOF, NamedNode(f"{ks.base_iri}Equipment"),
    )
    assert session.exec(select(AuditEvent)).all() == []


def test_reconcile_returns_unpersisted_decision_for_callers_transaction(workspace, monkeypatch) -> None:
    session, ks = workspace
    general = editor.apply_edit(ks.graph_iri, ks.base_iri, {"op": "add_class", "label": "Equipment"})
    pump = editor.apply_edit(ks.graph_iri, ks.base_iri, {"op": "add_class", "label": "Pump"})
    editor.apply_edit(ks.graph_iri, ks.base_iri, {
        "op": "add_axiom", "type": "subclass", "sub": pump, "super": general,
    })
    prop = editor.apply_edit(ks.graph_iri, ks.base_iri, {
        "op": "add_property", "kind": "object", "label": "uses", "domain": pump,
    })
    store.add_triples(ks.graph_iri, [(NamedNode(prop), RDFS_DOMAIN, NamedNode(general))])
    monkeypatch.setattr(tbox_reconcile, "engine", session.get_bind())

    applied, decisions = tbox_reconcile.reconcile(ks.id, ks.graph_iri, ks.base_iri)

    assert applied == ["uses.domain → Equipment (subsumed)"]
    assert len(decisions) == 1
    assert decisions[0].choice == "subsume"
    assert session.exec(select(TboxReconciliation)).all() == []
    assert store.object_terms(ks.graph_iri, NamedNode(prop), RDFS_DOMAIN) == [NamedNode(general)]


def test_extraction_sql_failure_reverts_tbox_and_decision(workspace, monkeypatch) -> None:
    session, ks = workspace
    job = ExtractionJob(
        knowledge_system_id=ks.id,
        kind="tbox",
        status="pending",
        model="test",
        chunk_ids=[1],
        total_chunks=1,
    )
    session.add(job)
    session.commit()
    session.refresh(job)
    monkeypatch.setattr(extraction_api, "engine", session.get_bind())
    monkeypatch.setattr(extraction_api.model_config, "set_ks_connections", lambda *_args: None)
    monkeypatch.setattr(extraction_api.prompt_config, "set_ks_prompts", lambda *_args: None)
    monkeypatch.setattr(extraction_api, "_terminology_aliases", lambda _ks: {})

    async def extract_one(**kwargs):
        iri = editor.apply_edit(kwargs["graph_iri"], kwargs["base_iri"], {
            "op": "add_class", "label": "Pump",
        })
        return {
            "classes_added": 1, "properties_added": 0, "axioms_added": 0,
            "provenance": [], "per_chunk": [{"chunk_id": 1, "status": "ok"}],
            "log": "ok", "iri": iri,
        }

    monkeypatch.setattr(extraction_api.extract, "extract_tbox_from_chunks", extract_one)
    monkeypatch.setattr(
        extraction_api.tbox_reconcile,
        "reconcile",
        lambda *_args: (
            ["learned"],
            [TboxReconciliation(
                knowledge_system_id=ks.id, slot="domain", property_label="uses",
                choice="keep", chosen_label="Pump", resolved_by="agent",
            )],
        ),
    )
    original_commit = Session.commit
    commit_count = 0

    def fail_extraction_commit(current: Session) -> None:
        nonlocal commit_count
        commit_count += 1
        # First commit marks the job running; the next one is the atomic
        # extraction/history/decision boundary inside the graph captures.
        if commit_count == 2:
            raise RuntimeError("database failure")
        original_commit(current)

    monkeypatch.setattr(Session, "commit", fail_extraction_commit)

    asyncio.run(extraction_api._run_extraction_job(
        job.id, ks.id, [(1, "Pump is equipment")], "test",
    ))

    assert not store.has_triple(
        ks.graph_iri, NamedNode(f"{ks.base_iri}Pump"), RDF_TYPE, None,
    )
    assert session.exec(select(AuditEvent)).all() == []
    assert session.exec(select(TboxReconciliation)).all() == []
