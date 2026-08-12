from __future__ import annotations

import hashlib

from pyoxigraph import Literal, NamedNode, Store
from sqlalchemy.pool import StaticPool
from sqlmodel import Session, SQLModel, create_engine

from app.db.models import KnowledgeSystem, OntologyRelease, ReleaseStatementProvenance
from app.ontology import release_service, store


def test_release_graphs_are_version_scoped() -> None:
    ks = KnowledgeSystem(id=7, public_id="public-ks", name="Example")
    first = OntologyRelease(id=11, knowledge_system_id=7, version="v1")
    second = OntologyRelease(id=12, knowledge_system_id=7, version="v2")

    first_graphs = release_service.graph_iris(ks, first)
    second_graphs = release_service.graph_iris(ks, second)

    assert set(first_graphs) == {"tbox", "vocabulary", "abox"}
    assert set(first_graphs.values()).isdisjoint(second_graphs.values())
    assert all("public-ks:11" in graph for graph in first_graphs.values())


def test_store_override_keeps_serving_graph_separate() -> None:
    serving = Store()
    graph = "urn:release:tbox"
    triple = (NamedNode("urn:subject"), NamedNode("urn:predicate"), Literal("value"))

    with store.use_store(serving):
        store.add_triples(graph, [triple])
        assert store.count_graph(graph) == 1

    assert list(serving.quads_for_pattern(None, None, None, NamedNode(graph)))


def test_release_provenance_is_read_from_frozen_payload() -> None:
    engine = create_engine(
        "sqlite://",
        connect_args={"check_same_thread": False},
        poolclass=StaticPool,
    )
    SQLModel.metadata.create_all(engine)
    payload = {
        "fact_key": "ind|urn:pump:1",
        "method": "extraction",
        "actor": "agent",
        "chunk": {"id": 3, "text": "Pump P-101 is installed."},
        "document": {"id": 4, "filename": "manual.txt"},
        "extraction": {"job_id": 5, "model": "model-a", "prompt_snapshot": {"hash": "abc"}},
        "reviews": [{"action": "accepted"}],
    }
    with Session(engine) as session:
        session.add(ReleaseStatementProvenance(
            knowledge_system_id=1,
            release_id=2,
            layer="abox",
            statement_key="ind|urn:pump:1",
            payload=payload,
        ))
        session.commit()

        sources = release_service.abox_sources(session, 2, ["ind|urn:pump:1"])

    assert sources["ind|urn:pump:1"] == [{
        "chunk_id": 3,
        "document_id": 4,
        "document": "manual.txt",
        "snippet": "Pump P-101 is installed.",
        "job_id": 5,
        "model": "model-a",
        "prompt_snapshot": {"hash": "abc"},
        "method": "extraction",
        "actor": "agent",
        "review": {"action": "accepted"},
    }]


def test_provision_materializes_verified_release_graphs(tmp_path, monkeypatch) -> None:
    database = create_engine(
        "sqlite://",
        connect_args={"check_same_thread": False},
        poolclass=StaticPool,
    )
    SQLModel.metadata.create_all(database)
    serving = Store()
    monkeypatch.setattr(release_service, "engine", database)
    monkeypatch.setattr(release_service, "get_store", lambda: serving)

    layers = {}
    for layer in ("tbox", "vocabulary", "abox"):
        name = f"{layer}-00001.nq"
        content = f'<urn:{layer}:subject> <urn:predicate> "value" <urn:source:{layer}> .\n'.encode()
        (tmp_path / name).write_bytes(content)
        layers[layer] = {
            "statements": 1,
            "files": [{
                "name": name,
                "statements": 1,
                "bytes": len(content),
                "sha256": hashlib.sha256(content).hexdigest(),
            }],
        }
    provenance_name = "abox-provenance.jsonl"
    provenance = b'{"fact_key":"ind|urn:item","chunk":null,"document":null,"extraction":null}\n'
    (tmp_path / provenance_name).write_bytes(provenance)
    manifest = {
        "capture_status": "ready",
        "layers": layers,
        "provenance": [{
            "name": provenance_name,
            "records": 1,
            "bytes": len(provenance),
            "sha256": hashlib.sha256(provenance).hexdigest(),
        }],
    }

    with Session(database) as session:
        ks = KnowledgeSystem(id=1, public_id="ks-public", name="KS")
        release = OntologyRelease(
            id=2,
            knowledge_system_id=1,
            version="v1",
            status="published",
            snapshot_dir=str(tmp_path),
            manifest=manifest,
        )
        session.add(ks)
        session.add(release)
        session.commit()
        deployment = release_service.ensure_deployment(session, ks, release)
        deployment_id = deployment.id

    release_service.provision(deployment_id)

    with Session(database) as session:
        deployment = release_service.deployment_for(session, 2)
        assert deployment is not None
        assert deployment.status == "active"
        assert deployment.statement_count == 3
        assert deployment.provenance_count == 1
        graph_iris = (
            deployment.tbox_graph_iri,
            deployment.vocabulary_graph_iri,
            deployment.abox_graph_iri,
        )
    assert all(
        sum(1 for _ in serving.quads_for_pattern(None, None, None, NamedNode(graph_iri))) == 1
        for graph_iri in graph_iris
    )
