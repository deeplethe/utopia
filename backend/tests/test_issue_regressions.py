from __future__ import annotations

from types import SimpleNamespace

import pytest
from fastapi import HTTPException
from pyoxigraph import NamedNode, Store
from sqlalchemy import event
from sqlalchemy.pool import StaticPool
from sqlmodel import Session, SQLModel, create_engine, select

from app.api import documents as documents_api
from app.api import extraction as extraction_api
from app.api import resolution as resolution_api
from app.db.models import (
    AboxProvenance, AxiomProvenance, Chunk, Document, EntityResolution, KnowledgeSystem, User,
)
from app.ontology import abox, abox_provenance, store, vocab
from app.ontology import resolution as resolution_service


def _database(*, foreign_keys: bool = False):
    database = create_engine(
        "sqlite://",
        connect_args={"check_same_thread": False},
        poolclass=StaticPool,
    )
    if foreign_keys:
        @event.listens_for(database, "connect")
        def _foreign_keys(connection, _record):
            connection.execute("PRAGMA foreign_keys=ON")
    SQLModel.metadata.create_all(database)
    return database


def _workspace(session: Session):
    user = User(username="reviewer", password_hash="unused")
    session.add(user)
    session.commit()
    ks = KnowledgeSystem(
        name="Issues",
        public_id="issues",
        owner_id=user.id,
        graph_iri="urn:issues:tbox",
        base_iri="urn:issues:",
    )
    session.add(ks)
    session.commit()
    return user, ks


def test_reparse_rebinds_resolution_and_provenance_with_foreign_keys(tmp_path, monkeypatch) -> None:
    database = _database(foreign_keys=True)
    with Session(database, expire_on_commit=False) as session:
        user, ks = _workspace(session)
        doc = Document(
            knowledge_system_id=ks.id,
            sha256="stable-document-revision",
            original_filename="pump.txt",
            ext="txt",
            storage_path="pump.txt",
            parse_status="parsed",
        )
        session.add(doc)
        session.commit()
        old = Chunk(document_id=doc.id, idx=0, text="Pump P-101", char_end=10)
        session.add(old)
        session.commit()
        decision = EntityResolution(
            knowledge_system_id=ks.id,
            surface_form="P-101",
            class_iri="urn:Pump",
            status="new",
            individual_iri="urn:issues:p101",
            source_chunk_id=old.id,
        )
        axiom_source = AboxProvenance(
            knowledge_system_id=ks.id,
            fact_key=abox_provenance.ind_key("urn:issues:p101"),
            chunk_id=old.id,
        )
        session.add(decision)
        session.add(axiom_source)
        session.commit()

        source_path = tmp_path / "pump.txt"
        source_path.write_text("Pump P-101", encoding="utf-8")
        monkeypatch.setattr(documents_api.blobstore, "abs_path", lambda _path: source_path)
        monkeypatch.setattr(
            documents_api.parser,
            "parse_file",
            lambda *_args: SimpleNamespace(text="Pump P-101", structured_document=None, backend="test"),
        )
        monkeypatch.setattr(
            documents_api.chunker,
            "chunk_document",
            lambda *_args: [SimpleNamespace(
                idx=0, text="Pump P-101", char_start=0, char_end=10, token_estimate=3,
            )],
        )

        result = documents_api._parse_document(doc, ks, user, session)
        assert result.parse_status == "parsed"
        new_chunk = session.exec(select(Chunk).where(Chunk.document_id == doc.id)).one()
        assert new_chunk.id != old.id
        session.refresh(decision)
        session.refresh(axiom_source)
        assert decision.source_document_id == doc.id
        assert decision.source_chunk_id == new_chunk.id
        assert decision.context["source_document_sha256"] == doc.sha256
        assert axiom_source.chunk_id == new_chunk.id

        monkeypatch.setattr(
            documents_api.parser,
            "parse_file",
            lambda *_args: SimpleNamespace(text="Pump P-101 reformatted", structured_document=None, backend="test-v2"),
        )
        monkeypatch.setattr(
            documents_api.chunker,
            "chunk_document",
            lambda *_args: [SimpleNamespace(
                idx=0, text="Pump P-101 reformatted", char_start=0, char_end=22, token_estimate=4,
            )],
        )
        result = documents_api._parse_document(doc, ks, user, session)
        assert result.parse_status == "parsed"
        latest_chunk = session.exec(select(Chunk).where(Chunk.document_id == doc.id)).one()
        session.refresh(decision)
        session.refresh(axiom_source)
        assert decision.source_chunk_id is None
        assert decision.source_document_id == doc.id
        assert axiom_source.chunk_id is None
        assert axiom_source.source_document_id == doc.id
        assert axiom_source.source_document_sha256 == doc.sha256
        stable_source = abox_provenance.sources_for(
            session, ks.id, [axiom_source.fact_key],
        )[axiom_source.fact_key][0]
        assert stable_source["document_id"] == doc.id
        assert stable_source["document_sha256"] == doc.sha256
        assert resolution_service._prior(
            session,
            ks.id,
            decision.surface_form,
            decision.class_iri,
            chunk_id=latest_chunk.id,
        ).id == decision.id


def test_reject_and_defer_are_explicit_idempotent_non_rdf_decisions() -> None:
    database = _database()
    rdf = Store()
    with store.use_store(rdf), Session(database, expire_on_commit=False) as session:
        user, ks = _workspace(session)
        rejected = EntityResolution(
            knowledge_system_id=ks.id, surface_form="noise", class_iri="urn:Thing",
        )
        deferred = EntityResolution(
            knowledge_system_id=ks.id, surface_form="maybe", class_iri="urn:Thing",
        )
        session.add(rejected)
        session.add(deferred)
        session.commit()

        body = resolution_api.ResolveRequest(
            action="reject", reason="OCR artifact", expected_updated_at=rejected.updated_at,
        )
        first = resolution_api.resolve(rejected.id, body, ks, user, session)
        assert first["status"] == "rejected"
        assert store.count_graph(resolution_api.abox_iri_for(ks)) == 0
        again = resolution_api.resolve(rejected.id, body, ks, user, session)
        assert again["idempotent"] is True
        with pytest.raises(HTTPException) as stale:
            resolution_api.resolve(
                rejected.id, resolution_api.ResolveRequest(action="new"), ks, user, session,
            )
        assert stale.value.status_code == 409

        result = resolution_api.resolve(
            deferred.id,
            resolution_api.ResolveRequest(action="defer", reason="Need owner confirmation"),
            ks, user, session,
        )
        assert result["status"] == "deferred"
        assert store.count_graph(resolution_api.abox_iri_for(ks)) == 0


def test_merge_individuals_preserves_facts_resolution_references_and_provenance() -> None:
    database = _database()
    rdf = Store()
    with store.use_store(rdf), Session(database, expire_on_commit=False) as session:
        user, ks = _workspace(session)
        graph = resolution_api.abox_iri_for(ks)
        source = abox.create_individual(graph, ks.base_iri, "Pump duplicate", "urn:Pump")
        canonical = abox.create_individual(graph, ks.base_iri, "Pump", "urn:Pump")
        other = abox.create_individual(graph, ks.base_iri, "Site", "urn:Site")
        abox.add_data_assertion(graph, source, "urn:serial", "P-101")
        abox.add_object_assertion(graph, other, "urn:contains", source)
        decision = EntityResolution(
            knowledge_system_id=ks.id, surface_form="Pump duplicate", class_iri="urn:Pump",
            status="new", individual_iri=source,
        )
        queue = EntityResolution(
            knowledge_system_id=ks.id, surface_form="Pump alias", class_iri="urn:Pump",
            status="pending",
        )
        data_source = AboxProvenance(
            knowledge_system_id=ks.id,
            fact_key=abox_provenance.data_key(source, "urn:serial", "P-101"),
        )
        relation_source = AboxProvenance(
            knowledge_system_id=ks.id,
            fact_key=abox_provenance.obj_key(other, "urn:contains", source),
        )
        session.add(decision)
        session.add(queue)
        session.add(data_source)
        session.add(relation_source)
        session.commit()

        result = resolution_api.merge_individuals(
            resolution_api.MergeIndividualsRequest(
                source_iri=source, canonical_iri=canonical, reason="Confirmed duplicate",
                resolution_id=queue.id,
            ),
            ks,
            user,
            session,
        )
        assert result["idempotent"] is False
        assert not abox.exists(graph, source)
        assert store.has_triple(graph, NamedNode(source), vocab.OWL_SAME_AS, NamedNode(canonical))
        merged = abox.get_individual(graph, canonical, {"urn:Pump": "Pump"}, {})
        assert any(item["value"] == "P-101" for item in merged["data_assertions"])
        assert store.has_triple(
            graph, NamedNode(other), NamedNode("urn:contains"), NamedNode(canonical),
        )
        session.refresh(decision)
        session.refresh(queue)
        session.refresh(data_source)
        session.refresh(relation_source)
        assert decision.individual_iri == canonical
        assert decision.context["merged_from_iri"] == source
        assert queue.status == "matched"
        assert queue.individual_iri == canonical
        assert queue.context["decision_action"] == "merge"
        assert data_source.fact_key == abox_provenance.data_key(canonical, "urn:serial", "P-101")
        assert relation_source.fact_key == abox_provenance.obj_key(other, "urn:contains", canonical)

        retry = resolution_api.merge_individuals(
            resolution_api.MergeIndividualsRequest(
                source_iri=source, canonical_iri=canonical, reason="Confirmed duplicate",
                resolution_id=queue.id,
            ),
            ks,
            user,
            session,
        )
        assert retry["idempotent"] is True


def test_document_reextraction_replaces_only_exclusive_tbox_and_abox_contributions() -> None:
    database = _database()
    rdf = Store()
    with store.use_store(rdf), Session(database, expire_on_commit=False) as session:
        _user, ks = _workspace(session)
        first = Document(
            knowledge_system_id=ks.id, sha256="first", original_filename="first.txt",
            ext="txt", parse_status="parsed",
        )
        second = Document(
            knowledge_system_id=ks.id, sha256="second", original_filename="second.txt",
            ext="txt", parse_status="parsed",
        )
        session.add(first)
        session.add(second)
        session.commit()
        first_chunk = Chunk(document_id=first.id, idx=0, text="first")
        second_chunk = Chunk(document_id=second.id, idx=0, text="second")
        session.add(first_chunk)
        session.add(second_chunk)
        session.commit()

        exclusive_class = NamedNode(ks.base_iri + "Exclusive")
        shared_class = NamedNode(ks.base_iri + "Shared")
        store.add_triples(ks.graph_iri, [
            (exclusive_class, vocab.RDF_TYPE, vocab.OWL_CLASS),
            (shared_class, vocab.RDF_TYPE, vocab.OWL_CLASS),
        ])
        for key, chunk, document in (
            ("class|Exclusive", first_chunk, first),
            ("class|Shared", first_chunk, first),
            ("class|Shared", second_chunk, second),
        ):
            session.add(AxiomProvenance(
                knowledge_system_id=ks.id, axiom_key=key, chunk_id=chunk.id,
                source_document_id=document.id, source_document_sha256=document.sha256,
            ))

        graph = resolution_api.abox_iri_for(ks)
        exclusive_individual = abox.create_individual(graph, ks.base_iri, "Exclusive", exclusive_class.value)
        shared_individual = abox.create_individual(graph, ks.base_iri, "Shared", shared_class.value)
        for iri, chunk, document in (
            (exclusive_individual, first_chunk, first),
            (shared_individual, first_chunk, first),
            (shared_individual, second_chunk, second),
        ):
            session.add(AboxProvenance(
                knowledge_system_id=ks.id, fact_key=abox_provenance.ind_key(iri),
                chunk_id=chunk.id, source_document_id=document.id,
                source_document_sha256=document.sha256,
            ))
        session.commit()

        contribution = documents_api.document_contribution(first.id, 500, ks, session)
        assert contribution["axiom_count"] == 2
        assert contribution["individual_count"] == 2
        assert contribution["abox_fact_count"] == 2
        assert any(item["shared"] for item in contribution["tbox_axioms"])
        assert any(item["shared"] for item in contribution["abox_facts"])

        abox_result = extraction_api._replace_abox_sources(session, ks, [first_chunk.id])
        tbox = extraction_api._replace_tbox_sources(session, ks, [first_chunk.id])
        assert tbox["sources_replaced"] == 2
        assert tbox["facts_retracted"] == 1
        assert tbox["exclusive_keys"] == ["class|Exclusive"]
        assert abox_result["sources_replaced"] == 2
        assert not store.has_triple(ks.graph_iri, exclusive_class, vocab.RDF_TYPE, vocab.OWL_CLASS)
        assert store.has_triple(ks.graph_iri, shared_class, vocab.RDF_TYPE, vocab.OWL_CLASS)
        assert not abox.exists(graph, exclusive_individual)
        assert abox.exists(graph, shared_individual)
        assert session.exec(select(AxiomProvenance).where(
            AxiomProvenance.source_document_id == second.id,
        )).all()
        assert session.exec(select(AboxProvenance).where(
            AboxProvenance.source_document_id == second.id,
        )).all()
