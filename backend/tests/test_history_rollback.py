from __future__ import annotations

import gzip

import pytest
from fastapi import HTTPException
from pyoxigraph import Literal, NamedNode, Store
from sqlalchemy.pool import StaticPool
from sqlmodel import Session, SQLModel, create_engine, select

from app.api import history as history_api
from app.db.models import AboxProvenance, AuditEvent, KnowledgeSystem, User
from app.ontology import abox_provenance, statement_provenance, store, workbench
from app.ontology.vocab import (
    OWL_CLASS,
    OWL_DISJOINT_WITH,
    OWL_NAMED_INDIVIDUAL,
    RDF_TYPE,
    RDFS_COMMENT,
    RDFS_LABEL,
)


@pytest.fixture
def history_workspace(monkeypatch):
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
        user = User(username="history-editor", password_hash="unused")
        session.add(user)
        session.commit()
        session.refresh(user)
        ks = KnowledgeSystem(
            name="History",
            public_id="history",
            owner_id=user.id,
            graph_iri="urn:history:tbox",
            base_iri="urn:history:onto:",
        )
        session.add(ks)
        session.commit()
        session.refresh(ks)
        yield session, user, ks


def _event(
    session: Session,
    ks: KnowledgeSystem,
    *,
    added: list[tuple] | None = None,
    removed: list[tuple] | None = None,
    graph: str | None = None,
    group_id: str | None = None,
) -> AuditEvent:
    row = AuditEvent(
        knowledge_system_id=ks.id,
        actor_name="editor",
        action="ontology.change_set",
        summary="fixture change",
        graph=graph,
        group_id=group_id,
        added=gzip.compress(store.dump_triples(added or [])) if added else None,
        removed=gzip.compress(store.dump_triples(removed or [])) if removed else None,
    )
    session.add(row)
    session.commit()
    session.refresh(row)
    return row


def test_multigraph_rollback_is_one_sql_transaction_and_updates_abox_provenance(
    history_workspace, monkeypatch,
) -> None:
    session, user, ks = history_workspace
    abox_iri = workbench.abox_iri_for(ks.graph_iri)
    cls = NamedNode(ks.base_iri + "Pump")
    ind = NamedNode("urn:history:individual:p1")
    label = (cls, RDFS_LABEL, Literal("Pump"))
    class_decl = (cls, RDF_TYPE, OWL_CLASS)
    individual_decl = (ind, RDF_TYPE, OWL_NAMED_INDIVIDUAL)
    individual_type = (ind, RDF_TYPE, cls)
    store.add_triples(ks.graph_iri, [class_decl, label])
    store.add_triples(abox_iri, [individual_decl, individual_type])
    group_id = "paired-change"
    first = _event(session, ks, added=[class_decl, label], group_id=group_id)
    _event(
        session,
        ks,
        added=[individual_decl, individual_type],
        graph=abox_iri,
        group_id=group_id,
    )
    session.add(AboxProvenance(
        knowledge_system_id=ks.id,
        fact_key=abox_provenance.ind_key(ind.value),
        method="manual",
        actor_name="editor",
    ))
    session.commit()

    commits = 0
    original_commit = session.commit

    def counted_commit() -> None:
        nonlocal commits
        commits += 1
        original_commit()

    monkeypatch.setattr(session, "commit", counted_commit)
    result = history_api.rollback(first.id, ks, user, session)

    assert result["undone"] == 2
    assert commits == 1
    assert store.read_triples(ks.graph_iri) == []
    assert store.read_triples(abox_iri) == []
    assert session.exec(select(AboxProvenance)).all() == []
    rollbacks = session.exec(
        select(AuditEvent).where(AuditEvent.action == "system.rollback")
    ).all()
    assert len(rollbacks) == 2
    assert {row.graph for row in rollbacks} == {ks.graph_iri, abox_iri}
    assert len({row.group_id for row in rollbacks}) == 1
    assert rollbacks[0].group_id is not None


def test_rollback_rejects_new_structural_error_and_reverts_every_graph(
    history_workspace,
) -> None:
    session, user, ks = history_workspace
    abox_iri = workbench.abox_iri_for(ks.graph_iri)
    left = NamedNode(ks.base_iri + "Left")
    right = NamedNode(ks.base_iri + "Right")
    ind = NamedNode("urn:history:individual:item")
    baseline_tbox = [
        (left, RDF_TYPE, OWL_CLASS),
        (left, RDFS_LABEL, Literal("Left")),
        (right, RDF_TYPE, OWL_CLASS),
        (right, RDFS_LABEL, Literal("Right")),
        (left, OWL_DISJOINT_WITH, right),
    ]
    baseline_abox = [
        (ind, RDF_TYPE, OWL_NAMED_INDIVIDUAL),
        (ind, RDF_TYPE, left),
        (ind, RDF_TYPE, right),
    ]
    store.add_triples(ks.graph_iri, baseline_tbox)
    store.add_triples(abox_iri, baseline_abox)
    # The recorded change fixed the contradiction by removing one type and also added harmless
    # TBox metadata. Rolling it back would restore the disjoint type, so the whole pair must revert.
    store.remove_triples(abox_iri, [(ind, RDF_TYPE, right)])
    harmless_metadata = (right, RDFS_COMMENT, Literal("reviewed"))
    store.add_triples(ks.graph_iri, [harmless_metadata])
    group_id = "fixed-error"
    first = _event(session, ks, added=[harmless_metadata], group_id=group_id)
    _event(
        session,
        ks,
        removed=[(ind, RDF_TYPE, right)],
        graph=abox_iri,
        group_id=group_id,
    )
    before_tbox = set(store.read_triples(ks.graph_iri))
    before_abox = set(store.read_triples(abox_iri))

    with pytest.raises(HTTPException) as caught:
        history_api.rollback(first.id, ks, user, session)

    assert caught.value.status_code == 422
    assert caught.value.detail["code"] == "ontology_structural_validation_failed"
    assert set(store.read_triples(ks.graph_iri)) == before_tbox
    assert set(store.read_triples(abox_iri)) == before_abox
    assert session.exec(
        select(AuditEvent).where(AuditEvent.action == "system.rollback")
    ).all() == []


def test_rollback_noop_creates_no_audit_event(history_workspace) -> None:
    session, user, ks = history_workspace
    cls = NamedNode(ks.base_iri + "Pump")
    triple = (cls, RDF_TYPE, OWL_CLASS)
    event = _event(session, ks, added=[triple])

    with pytest.raises(HTTPException) as caught:
        history_api.rollback(event.id, ks, user, session)

    assert caught.value.status_code == 409
    assert caught.value.detail["code"] == "history_rollback_noop"
    assert session.exec(
        select(AuditEvent).where(AuditEvent.action == "system.rollback")
    ).all() == []


def test_rollback_restores_merge_provenance_without_losing_existing_target_source(
    history_workspace,
) -> None:
    session, user, ks = history_workspace
    abox_iri = workbench.abox_iri_for(ks.graph_iri)
    subject = NamedNode("urn:history:individual:subject")
    prop = NamedNode("urn:history:property:related")
    source = NamedNode("urn:history:individual:source")
    target = NamedNode("urn:history:individual:target")
    old_triple = (subject, prop, source)
    new_triple = (subject, prop, target)
    old_key = abox_provenance.obj_key(subject.value, prop.value, source.value)
    new_key = abox_provenance.obj_key(subject.value, prop.value, target.value)

    # The target fact already existed before the merge. The net graph diff therefore
    # contains only the removed source assertion, while provenance still has to migrate.
    store.add_triples(abox_iri, [old_triple, new_triple])
    session.add_all([
        AboxProvenance(knowledge_system_id=ks.id, fact_key=old_key, chunk_id=101),
        AboxProvenance(knowledge_system_id=ks.id, fact_key=new_key, chunk_id=202),
    ])
    session.commit()

    with store.capture(abox_iri) as cap:
        store.remove_triples(abox_iri, [old_triple])
        added, removed = cap.diff()
    event = _event(session, ks, removed=[old_triple], graph=abox_iri)
    statement_provenance.record_abox_diff(
        session,
        ks.id,
        added,
        removed,
        event,
        abox_iri=abox_iri,
        operations=[{"op": "merge_classes", "source": source.value, "target": target.value}],
        results=[target.value],
    )
    assert {(row.fact_key, row.chunk_id) for row in session.exec(select(AboxProvenance)).all()} == {
        (new_key, 101),
        (new_key, 202),
        (new_key, None),
    }

    result = history_api.rollback(event.id, ks, user, session)

    assert result["undone"] == 1
    assert store.has_triple(abox_iri, *old_triple)
    assert store.has_triple(abox_iri, *new_triple)
    rows = session.exec(select(AboxProvenance)).all()
    assert {(row.fact_key, row.chunk_id) for row in rows} == {
        (old_key, 101),
        (new_key, 202),
    }
