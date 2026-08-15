from __future__ import annotations

import asyncio

import pytest
from fastapi import HTTPException
from pyoxigraph import Literal, NamedNode, Store
from sqlalchemy.pool import StaticPool
from sqlmodel import Session, SQLModel, create_engine, select

from app.api import extraction as extraction_api
from app.api import resolution as resolution_api
from app.db.models import (
    AboxProvenance,
    AuditEvent,
    Chunk,
    Document,
    EntityResolution,
    ExtractionJob,
    KnowledgeSystem,
    User,
)
from app.ontology import abox, abox_provenance, editor, store, workbench
from app.ontology.vocab import OWL_NAMED_INDIVIDUAL, RDF_TYPE, RDFS_LABEL


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
        user = User(username="abox-reviewer", password_hash="unused")
        session.add(user)
        session.commit()
        session.refresh(user)
        ks = KnowledgeSystem(
            name="ABox extraction atomicity",
            public_id="abox-extraction-atomicity",
            owner_id=user.id,
            graph_iri="urn:abox-extraction:tbox",
            base_iri="urn:abox-extraction:onto:",
        )
        session.add(ks)
        session.commit()
        session.refresh(ks)
        cls = editor.apply_edit(
            ks.graph_iri,
            ks.base_iri,
            {"op": "add_class", "label": "Device"},
        )
        prop = editor.apply_edit(
            ks.graph_iri,
            ks.base_iri,
            {
                "op": "add_property",
                "kind": "data",
                "label": "serial",
                "domain": cls,
                "range": "string",
            },
        )
        yield session, database, user, ks, cls, prop


def _document_job(session: Session, ks: KnowledgeSystem) -> tuple[Document, Chunk, ExtractionJob]:
    document = Document(
        knowledge_system_id=ks.id,
        sha256="abox-extraction-document",
        original_filename="device.txt",
        ext="txt",
    )
    session.add(document)
    session.commit()
    session.refresh(document)
    chunk = Chunk(document_id=document.id, index=0, text="Pump-1 has serial SN-1")
    session.add(chunk)
    session.commit()
    session.refresh(chunk)
    job = ExtractionJob(
        knowledge_system_id=ks.id,
        kind="abox",
        status="pending",
        model="test",
        chunk_ids=[chunk.id],
        total_chunks=1,
    )
    session.add(job)
    session.commit()
    session.refresh(job)
    return document, chunk, job


def _stub_extraction(monkeypatch, *, cls: str, prop: str) -> None:
    async def extract_instances(**kwargs):
        assert kwargs["session"] is not None
        assert kwargs["commit"] is False
        assert kwargs["fail_fast"] is True
        session = kwargs["session"]
        chunk_id = kwargs["chunks"][0][0]
        individual = abox.create_individual(
            kwargs["abox_iri"], kwargs["base_iri"], "Pump-1", cls,
        )
        abox.add_data_assertion(kwargs["abox_iri"], individual, prop, "SN-1")
        session.add(EntityResolution(
            knowledge_system_id=kwargs["ks_id"],
            surface_form="Pump-1",
            class_iri=cls,
            status="new",
            individual_iri=individual,
            source_chunk_id=chunk_id,
        ))
        session.add(AboxProvenance(
            knowledge_system_id=kwargs["ks_id"],
            fact_key=abox_provenance.ind_key(individual),
            chunk_id=chunk_id,
            job_id=kwargs["job_id"],
        ))
        session.flush()
        return {
            "created": 1,
            "matched": 0,
            "queued": 0,
            "assertions": 1,
            "rejected": 0,
            "unknown_classes": {},
            "per_chunk": [{"chunk_id": chunk_id, "status": "ok"}],
            "log": "ok",
        }

    monkeypatch.setattr(
        extraction_api.abox_extract,
        "extract_instances_from_chunks",
        extract_instances,
    )


def test_abox_extraction_commit_failure_reverts_rdf_resolution_provenance_and_document(
    workspace, monkeypatch,
) -> None:
    session, database, _user, ks, cls, prop = workspace
    document, chunk, job = _document_job(session, ks)
    monkeypatch.setattr(extraction_api, "engine", database)
    monkeypatch.setattr(extraction_api.model_config, "set_ks_connections", lambda *_args: None)
    monkeypatch.setattr(extraction_api.prompt_config, "set_ks_prompts", lambda *_args: None)
    monkeypatch.setattr(extraction_api.workbench, "structural_error_signatures", lambda *_args: set())
    monkeypatch.setattr(extraction_api.workbench, "new_structural_errors", lambda *_args: [])
    _stub_extraction(monkeypatch, cls=cls, prop=prop)

    original_commit = Session.commit
    commit_count = 0

    def fail_atomic_commit(current: Session) -> None:
        nonlocal commit_count
        commit_count += 1
        if commit_count == 2:
            raise RuntimeError("injected extraction commit failure")
        original_commit(current)

    monkeypatch.setattr(Session, "commit", fail_atomic_commit)
    asyncio.run(extraction_api._run_abox_extraction_job(
        job.id,
        ks.id,
        [(chunk.id, chunk.text)],
        "test",
    ))

    assert store.read_triples(workbench.abox_iri_for(ks.graph_iri)) == []
    assert session.exec(select(EntityResolution)).all() == []
    assert session.exec(select(AboxProvenance)).all() == []
    assert session.exec(select(AuditEvent)).all() == []
    session.refresh(document)
    assert document.abox_extracted_at is None


def test_abox_extraction_structural_gate_reverts_every_side_effect(workspace, monkeypatch) -> None:
    session, database, _user, ks, cls, prop = workspace
    document, chunk, job = _document_job(session, ks)
    monkeypatch.setattr(extraction_api, "engine", database)
    monkeypatch.setattr(extraction_api.model_config, "set_ks_connections", lambda *_args: None)
    monkeypatch.setattr(extraction_api.prompt_config, "set_ks_prompts", lambda *_args: None)
    monkeypatch.setattr(extraction_api.workbench, "structural_error_signatures", lambda *_args: set())
    monkeypatch.setattr(
        extraction_api.workbench,
        "new_structural_errors",
        lambda *_args: ["abox:new-error"],
    )
    _stub_extraction(monkeypatch, cls=cls, prop=prop)

    asyncio.run(extraction_api._run_abox_extraction_job(
        job.id,
        ks.id,
        [(chunk.id, chunk.text)],
        "test",
    ))

    assert store.read_triples(workbench.abox_iri_for(ks.graph_iri)) == []
    assert session.exec(select(EntityResolution)).all() == []
    assert session.exec(select(AboxProvenance)).all() == []
    assert session.exec(select(AuditEvent)).all() == []
    session.refresh(document)
    assert document.abox_extracted_at is None


def test_abox_extraction_no_graph_diff_emits_no_audit(workspace, monkeypatch) -> None:
    session, database, _user, ks, _cls, _prop = workspace
    document, chunk, job = _document_job(session, ks)
    monkeypatch.setattr(extraction_api, "engine", database)
    monkeypatch.setattr(extraction_api.model_config, "set_ks_connections", lambda *_args: None)
    monkeypatch.setattr(extraction_api.prompt_config, "set_ks_prompts", lambda *_args: None)
    monkeypatch.setattr(extraction_api.workbench, "structural_error_signatures", lambda *_args: set())
    monkeypatch.setattr(extraction_api.workbench, "new_structural_errors", lambda *_args: [])

    async def no_op_extraction(**kwargs):
        return {
            "created": 0,
            "matched": 0,
            "queued": 0,
            "assertions": 0,
            "rejected": 0,
            "unknown_classes": {},
            "per_chunk": [{"chunk_id": chunk.id, "status": "ok"}],
            "log": "no graph changes",
        }

    monkeypatch.setattr(
        extraction_api.abox_extract,
        "extract_instances_from_chunks",
        no_op_extraction,
    )
    monkeypatch.setattr(
        extraction_api,
        "_run_terminology_bg",
        lambda *_args: {
            "terms_added": 0,
            "terms_mapped": 0,
            "terminology_proposals": 0,
            "terminology_error": None,
        },
    )
    monkeypatch.setattr(extraction_api.validation_agent, "triage_bg", lambda *_args: [])

    asyncio.run(extraction_api._run_abox_extraction_job(
        job.id,
        ks.id,
        [(chunk.id, chunk.text)],
        "test",
    ))

    assert session.exec(select(AuditEvent)).all() == []
    session.refresh(document)
    assert document.abox_extracted_at is not None
    session.refresh(job)
    assert job.status == "completed"


def test_abox_extraction_cache_failure_is_best_effort(workspace, monkeypatch) -> None:
    session, database, _user, ks, cls, prop = workspace
    _document, chunk, job = _document_job(session, ks)
    monkeypatch.setattr(extraction_api, "engine", database)
    monkeypatch.setattr(extraction_api.model_config, "set_ks_connections", lambda *_args: None)
    monkeypatch.setattr(extraction_api.prompt_config, "set_ks_prompts", lambda *_args: None)
    monkeypatch.setattr(extraction_api.workbench, "structural_error_signatures", lambda *_args: set())
    monkeypatch.setattr(extraction_api.workbench, "new_structural_errors", lambda *_args: [])
    monkeypatch.setattr(
        extraction_api.retrieval,
        "invalidate",
        lambda *_args: (_ for _ in ()).throw(RuntimeError("cache unavailable")),
    )
    _stub_extraction(monkeypatch, cls=cls, prop=prop)
    monkeypatch.setattr(
        extraction_api,
        "_run_terminology_bg",
        lambda *_args: {
            "terms_added": 0,
            "terms_mapped": 0,
            "terminology_proposals": 0,
            "terminology_error": None,
        },
    )
    monkeypatch.setattr(extraction_api.validation_agent, "triage_bg", lambda *_args: [])

    asyncio.run(extraction_api._run_abox_extraction_job(
        job.id,
        ks.id,
        [(chunk.id, chunk.text)],
        "test",
    ))

    session.refresh(job)
    assert job.status == "completed"
    assert len(session.exec(select(AuditEvent)).all()) == 1
    assert store.read_triples(workbench.abox_iri_for(ks.graph_iri))


def _pending_resolution(
    session: Session,
    ks: KnowledgeSystem,
    cls: str,
    prop: str,
) -> EntityResolution:
    row = EntityResolution(
        knowledge_system_id=ks.id,
        surface_form="Pump-2",
        class_iri=cls,
        status="pending",
        context={
            "pending_attributes": [{"prop": prop, "value": "SN-2"}],
        },
    )
    session.add(row)
    session.commit()
    session.refresh(row)
    return row


def test_manual_resolution_commit_failure_restores_queue_and_rdf(workspace, monkeypatch) -> None:
    session, _database, user, ks, cls, prop = workspace
    row = _pending_resolution(session, ks, cls, prop)
    original_commit = session.commit

    def fail_commit() -> None:
        raise RuntimeError("injected resolution commit failure")

    monkeypatch.setattr(session, "commit", fail_commit)
    with pytest.raises(RuntimeError, match="injected resolution commit failure"):
        resolution_api.resolve(
            row.id,
            resolution_api.ResolveRequest(action="new"),
            ks,
            user,
            session,
        )
    monkeypatch.setattr(session, "commit", original_commit)

    assert store.read_triples(workbench.abox_iri_for(ks.graph_iri)) == []
    session.refresh(row)
    assert row.status == "pending"
    assert row.individual_iri is None
    assert session.exec(select(AuditEvent)).all() == []
    assert session.exec(select(AboxProvenance)).all() == []


def test_manual_resolution_structural_error_reverts_queue_and_rdf(workspace, monkeypatch) -> None:
    session, _database, user, ks, cls, prop = workspace
    row = _pending_resolution(session, ks, cls, prop)
    monkeypatch.setattr(resolution_api.workbench, "structural_error_signatures", lambda *_args: set())
    monkeypatch.setattr(
        resolution_api.workbench,
        "new_structural_errors",
        lambda *_args: ["abox:new-error"],
    )

    with pytest.raises(HTTPException) as exc_info:
        resolution_api.resolve(
            row.id,
            resolution_api.ResolveRequest(action="new"),
            ks,
            user,
            session,
        )

    assert exc_info.value.status_code == 422
    assert store.read_triples(workbench.abox_iri_for(ks.graph_iri)) == []
    session.refresh(row)
    assert row.status == "pending"
    assert session.exec(select(AuditEvent)).all() == []
    assert session.exec(select(AboxProvenance)).all() == []


def test_manual_resolution_cache_failure_is_best_effort(workspace, monkeypatch) -> None:
    session, _database, user, ks, cls, prop = workspace
    row = _pending_resolution(session, ks, cls, prop)
    monkeypatch.setattr(
        resolution_api.retrieval,
        "invalidate",
        lambda *_args: (_ for _ in ()).throw(RuntimeError("cache unavailable")),
    )

    result = resolution_api.resolve(
        row.id,
        resolution_api.ResolveRequest(action="new"),
        ks,
        user,
        session,
    )

    assert result["status"] == "new"
    session.refresh(row)
    assert row.status == "new"
    assert len(session.exec(select(AuditEvent)).all()) == 1
    assert len(session.exec(select(AboxProvenance)).all()) == 2
    assert store.read_triples(workbench.abox_iri_for(ks.graph_iri))


def test_manual_match_without_new_triples_still_records_decision_without_history_noop(
    workspace,
) -> None:
    session, _database, user, ks, cls, prop = workspace
    abox_iri = workbench.abox_iri_for(ks.graph_iri)
    individual = "urn:abox-extraction:individual:existing"
    store.add_triples(abox_iri, [
        (NamedNode(individual), RDF_TYPE, OWL_NAMED_INDIVIDUAL),
        (NamedNode(individual), RDF_TYPE, NamedNode(cls)),
        (NamedNode(individual), RDFS_LABEL, Literal("Pump-2")),
        (NamedNode(individual), NamedNode(prop), Literal("SN-2")),
    ])
    row = _pending_resolution(session, ks, cls, prop)

    result = resolution_api.resolve(
        row.id,
        resolution_api.ResolveRequest(action="match", individual_iri=individual),
        ks,
        user,
        session,
    )

    assert result["status"] == "matched"
    assert session.exec(select(EntityResolution)).one().status == "matched"
    events = session.exec(select(AuditEvent)).all()
    assert len(events) == 1
    assert events[0].added is None and events[0].removed is None
    assert events[0].detail["replayed"] == 0
    facts = {item.fact_key for item in session.exec(select(AboxProvenance)).all()}
    assert abox_provenance.ind_key(individual) in facts
    assert abox_provenance.data_key(individual, prop, "SN-2") in facts
