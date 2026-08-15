from __future__ import annotations

from contextlib import contextmanager
from datetime import datetime, timezone

import pytest
from fastapi import HTTPException
from pyoxigraph import BlankNode, Literal, NamedNode, Store
from sqlalchemy.pool import StaticPool
from sqlmodel import Session, SQLModel, create_engine, select

from app.api import conflicts as conflicts_api
from app.api import ontology as ontology_api
from app.api import rdf_import as rdf_import_api
from app.db.models import AboxProvenance, AuditEvent, Conflict, Document, KnowledgeSystem, User
from app.ontology import abox_provenance, editor, modeling_assistant, retrieval, schema, skos, store, workbench
from app.ontology.vocab import (
    OWL_DISJOINT_WITH,
    OWL_NAMED_INDIVIDUAL,
    RDF_TYPE,
    RDFS_LABEL,
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
        user = User(username="editor", password_hash="unused")
        session.add(user)
        session.commit()
        session.refresh(user)
        ks = KnowledgeSystem(
            name="Workbench",
            public_id="workbench",
            owner_id=user.id,
            graph_iri="urn:workbench:tbox",
            base_iri="urn:workbench:onto:",
        )
        session.add(ks)
        session.commit()
        session.refresh(ks)
        yield session, user, ks


def _changes(session, user, ks, operations, **kwargs):
    if not kwargs.get("dry_run") and "expected_revision" not in kwargs:
        kwargs["expected_revision"] = workbench.ontology_revision(ks.graph_iri)
    return ontology_api.change_ontology(
        ontology_api.ChangeSetRequest(operations=operations, **kwargs),
        ks,
        user,
        session,
    )


def test_revision_is_stable_across_triple_insertion_order(monkeypatch) -> None:
    monkeypatch.setattr(store, "_store", Store())
    store._graph_locks.clear()
    store._recorders.clear()
    graph = "urn:revision:tbox"
    triples = [
        (NamedNode("urn:c:b"), RDF_TYPE, NamedNode("http://www.w3.org/2002/07/owl#Class")),
        (NamedNode("urn:c:b"), RDFS_LABEL, Literal("B")),
        (NamedNode("urn:c:a"), RDF_TYPE, NamedNode("http://www.w3.org/2002/07/owl#Class")),
        (NamedNode("urn:c:a"), RDFS_LABEL, Literal("A")),
    ]
    store.add_triples(graph, triples)
    first = workbench.ontology_revision(graph)
    store.clear_graph(graph)
    store.add_triples(graph, reversed(triples))
    assert workbench.ontology_revision(graph) == first


def test_preview_returns_diff_conflicts_and_impact_without_persisting(workspace) -> None:
    session, user, ks = workspace
    device = editor.apply_edit(ks.graph_iri, ks.base_iri, {"op": "add_class", "label": "Device"})
    pump = editor.apply_edit(ks.graph_iri, ks.base_iri, {"op": "add_class", "label": "Pump"})
    prop = editor.apply_edit(ks.graph_iri, ks.base_iri, {
        "op": "add_property", "kind": "object", "label": "uses",
        "domain_members": [device, pump], "range_members": [device, pump],
    })
    ind = NamedNode("urn:workbench:individual:p1")
    store.add_triples(workbench.abox_iri_for(ks.graph_iri), [
        (ind, RDF_TYPE, OWL_NAMED_INDIVIDUAL),
        (ind, RDF_TYPE, NamedNode(pump)),
        (ind, NamedNode(prop), ind),
    ])
    revision = workbench.ontology_revision(ks.graph_iri)

    response = _changes(
        session, user, ks,
        [{"op": "delete_class", "iri": pump}],
        dry_run=True,
        expected_revision=revision,
    )

    assert response["applied"] == 0
    assert response["base_revision"] == revision
    assert response["revision"] != revision  # proposed resulting revision
    assert response["diff"]["counts"]["tbox_removed"] > 0
    assert response["diff"]["counts"]["abox_removed"] == 3
    assert response["impact"]["totals"]["individuals_deleted"] == 1
    assert response["impact"]["totals"]["properties_using_class"] == 1
    assert response["impact"]["totals"]["abox_type_assertions"] == 1
    assert response["impact"]["totals"]["abox_property_assertions"] == 1
    assert response["impact"]["operations"][0]["entity_iri"] == pump
    assert response["conflicts"] == []
    assert workbench.ontology_revision(ks.graph_iri) == revision
    assert {item["iri"] for item in schema.build_view(ks.graph_iri)["classes"]} == {device, pump}
    assert session.exec(select(Conflict)).all() == []
    assert session.exec(select(AuditEvent)).all() == []


def test_compact_preview_omits_rdf_bodies_but_keeps_counts_and_validation(workspace) -> None:
    session, user, ks = workspace
    pump = editor.apply_edit(
        ks.graph_iri,
        ks.base_iri,
        {"op": "add_class", "label": "Pump"},
    )
    individual = NamedNode("urn:workbench:individual:p1")
    abox_iri = workbench.abox_iri_for(ks.graph_iri)
    store.add_triples(abox_iri, [
        (individual, RDF_TYPE, OWL_NAMED_INDIVIDUAL),
        (individual, RDF_TYPE, NamedNode(pump)),
    ])
    revision = workbench.ontology_revision(ks.graph_iri)

    response = _changes(
        session,
        user,
        ks,
        [{"op": "delete_class", "iri": pump}],
        dry_run=True,
        expected_revision=revision,
        include_rdf_diff=False,
    )

    assert response["diff"] == {
        "tbox_added": "",
        "tbox_removed": "",
        "abox_added": "",
        "abox_removed": "",
        "counts": {
            "tbox_added": 0,
            "tbox_removed": 2,
            "abox_added": 0,
            "abox_removed": 2,
        },
    }
    assert response["impact"]["totals"]["individuals_deleted"] == 1
    assert response["impact"]["totals"]["abox_assertions"] == 2
    assert response["structural_validation"]["committable"] is True
    assert workbench.ontology_revision(ks.graph_iri) == revision
    assert {item["iri"] for item in schema.build_view(ks.graph_iri)["classes"]} == {pump}


def test_class_delete_impact_includes_subjects_pointing_to_deleted_individual(workspace) -> None:
    _session, _user, ks = workspace
    pump = editor.apply_edit(ks.graph_iri, ks.base_iri, {"op": "add_class", "label": "Pump"})
    prop = editor.apply_edit(ks.graph_iri, ks.base_iri, {
        "op": "add_property", "kind": "object", "label": "uses",
    })
    owner = NamedNode("urn:workbench:individual:owner")
    doomed = NamedNode("urn:workbench:individual:doomed")
    store.add_triples(workbench.abox_iri_for(ks.graph_iri), [
        (doomed, RDF_TYPE, OWL_NAMED_INDIVIDUAL),
        (doomed, RDF_TYPE, NamedNode(pump)),
        (owner, NamedNode(prop), doomed),
    ])

    impact = workbench.analyze_entity_impact(ks.graph_iri, pump, "class")

    assert set(impact["affected_individuals"]) == {owner.value, doomed.value}
    assert impact["affected_individual_count"] == 2


def test_batch_failure_rolls_back_all_tbox_and_abox_changes(workspace) -> None:
    session, user, ks = workspace
    before = workbench.ontology_revision(ks.graph_iri)

    with pytest.raises(HTTPException) as caught:
        _changes(session, user, ks, [
            {"op": "add_class", "label": "Pump"},
            {"op": "update_class", "iri": "urn:missing", "label": "Broken"},
        ])

    assert caught.value.status_code == 400
    assert workbench.ontology_revision(ks.graph_iri) == before
    assert schema.build_view(ks.graph_iri)["classes"] == []
    assert session.exec(select(AuditEvent)).all() == []


def test_legacy_single_edit_uses_workbench_validation_and_confirmation(workspace) -> None:
    session, user, ks = workspace
    pump = editor.apply_edit(ks.graph_iri, ks.base_iri, {"op": "add_class", "label": "Pump"})
    before = workbench.ontology_revision(ks.graph_iri)

    with pytest.raises(HTTPException) as destructive:
        ontology_api.edit_ontology(
            ontology_api.EditRequest(op="delete_class", iri=pump), ks, user, session,
        )
    assert destructive.value.status_code == 400
    assert workbench.ontology_revision(ks.graph_iri) == before

    response = ontology_api.edit_ontology(
        ontology_api.EditRequest(op="add_class", label="Valve"), ks, user, session,
    )
    assert response["result"].endswith("Valve")
    assert len(session.exec(select(AuditEvent)).all()) == 1


def test_commit_requires_confirmation_and_rejects_stale_revision(workspace) -> None:
    session, user, ks = workspace
    pump = editor.apply_edit(ks.graph_iri, ks.base_iri, {"op": "add_class", "label": "Pump"})
    revision = workbench.ontology_revision(ks.graph_iri)

    with pytest.raises(HTTPException) as confirmation:
        _changes(
            session, user, ks,
            [{"op": "delete_class", "iri": pump}],
            expected_revision=revision,
        )
    assert confirmation.value.status_code == 400

    editor.apply_edit(ks.graph_iri, ks.base_iri, {"op": "add_class", "label": "Valve"})
    current = workbench.ontology_revision(ks.graph_iri)
    with pytest.raises(HTTPException) as stale:
        _changes(
            session, user, ks,
            [{"op": "delete_class", "iri": pump}],
            expected_revision=revision,
            confirm_destructive=True,
        )
    assert stale.value.status_code == 409
    assert stale.value.detail["code"] == "ontology_revision_conflict"
    assert stale.value.detail["current_revision"] == current


def test_commit_requires_expected_revision(workspace) -> None:
    session, user, ks = workspace
    with pytest.raises(HTTPException) as missing:
        ontology_api.change_ontology(
            ontology_api.ChangeSetRequest(
                operations=[{"op": "add_class", "label": "Pump"}],
            ),
            ks,
            user,
            session,
        )
    assert missing.value.status_code == 400
    assert "expected_revision" in str(missing.value.detail)
    assert schema.build_view(ks.graph_iri)["classes"] == []


def test_atomic_commit_records_one_tbox_event_and_revision(workspace) -> None:
    session, user, ks = workspace
    before = workbench.ontology_revision(ks.graph_iri)
    response = _changes(
        session, user, ks,
        [
            {"op": "add_class", "label": "Device"},
            {"op": "add_class", "label": "Pump"},
            {"op": "add_axiom", "type": "subclass", "sub": "Pump", "super": "Device"},
        ],
        expected_revision=before,
        reason="Create the equipment hierarchy",
    )

    assert response["applied"] == 3
    assert response["revision"] != before
    assert response["revision"] == workbench.ontology_revision(ks.graph_iri)
    assert response["view"]["stats"] == {
        "class_count": 2, "property_count": 0, "axiom_count": 1,
    }
    events = session.exec(select(AuditEvent).where(AuditEvent.action == "ontology.change_set")).all()
    assert len(events) == 1
    assert events[0].summary == "Create the equipment hierarchy"


def test_class_delete_shrinks_union_without_dangling_blank_nodes(workspace) -> None:
    _session, _user, ks = workspace
    a = editor.apply_edit(ks.graph_iri, ks.base_iri, {"op": "add_class", "label": "A"})
    b = editor.apply_edit(ks.graph_iri, ks.base_iri, {"op": "add_class", "label": "B"})
    prop = editor.apply_edit(ks.graph_iri, ks.base_iri, {
        "op": "add_property", "kind": "object", "label": "rel",
        "domain_members": [a, b], "range_members": [a, b],
    })

    editor.apply_edit(ks.graph_iri, ks.base_iri, {"op": "delete_class", "iri": b})

    item = next(item for item in schema.build_view(ks.graph_iri)["object_properties"] if item["iri"] == prop)
    assert item["domain"] == a
    assert item["domain_members"] == [a]
    assert item["range"] == a
    assert item["range_members"] == [a]


def test_data_property_rejects_class_range_members(workspace) -> None:
    _session, _user, ks = workspace
    with pytest.raises(editor.EditError, match="only valid for object"):
        editor.apply_edit(ks.graph_iri, ks.base_iri, {
            "op": "add_property", "kind": "data", "label": "serial",
            "range_members": ["Device", "Asset"],
        })


def test_clear_union_removes_anonymous_expression(workspace) -> None:
    _session, _user, ks = workspace
    a = editor.apply_edit(ks.graph_iri, ks.base_iri, {"op": "add_class", "label": "A"})
    b = editor.apply_edit(ks.graph_iri, ks.base_iri, {"op": "add_class", "label": "B"})
    prop = editor.apply_edit(ks.graph_iri, ks.base_iri, {
        "op": "add_property", "kind": "object", "label": "rel",
        "domain_members": [a, b],
    })
    before = len(store.read_triples(ks.graph_iri))

    editor.apply_edit(ks.graph_iri, ks.base_iri, {
        "op": "update_property", "iri": prop, "clear_domain": True,
    })

    item = schema.build_view(ks.graph_iri)["object_properties"][0]
    assert item["domain"] is None
    assert item["domain_members"] == []
    assert len(store.read_triples(ks.graph_iri)) < before - 1


def test_modeling_assistant_validates_and_previews_without_applying(workspace, monkeypatch) -> None:
    session, _user, ks = workspace
    device = editor.apply_edit(ks.graph_iri, ks.base_iri, {"op": "add_class", "label": "Device"})
    revision = workbench.ontology_revision(ks.graph_iri)
    monkeypatch.setattr(modeling_assistant.openrouter, "chat_sync", lambda *args, **kwargs: """
        {"summary":"Add pump","reason":"Pump is a kind of device","operations":[
          {"op":"add_class","label":"Pump"},
          {"op":"add_axiom","type":"subclass","sub":"urn:workbench:onto:Pump",
           "super":"urn:workbench:onto:Device"}
        ]}
    """)

    # The assistant may only reference IRIs that existed before the suggestion; it cannot
    # guess the generated IRI of a class added earlier in the same payload.
    with pytest.raises(HTTPException) as invalid:
        ontology_api.suggest_ontology_changes(
            ontology_api.SuggestOntologyRequest(
                instruction="Add Pump under Device", expected_revision=revision,
            ),
            ks,
            session,
        )
    assert invalid.value.status_code == 422
    assert workbench.ontology_revision(ks.graph_iri) == revision

    monkeypatch.setattr(modeling_assistant.openrouter, "chat_sync", lambda *args, **kwargs: f"""
        {{"summary":"Document device","reason":"Improve the definition","operations":[
          {{"op":"update_class","iri":"{device}","comment":"A managed physical asset"}}
        ]}}
    """)
    result = ontology_api.suggest_ontology_changes(
        ontology_api.SuggestOntologyRequest(
            instruction="Improve the Device definition", expected_revision=revision,
        ),
        ks,
        session,
    )
    assert result["summary"] == "Document device"
    assert result["preview"]["dry_run"] is True
    assert result["preview"]["diff"]["counts"]["tbox_added"] == 1
    assert workbench.ontology_revision(ks.graph_iri) == revision
    assert schema.build_view(ks.graph_iri)["classes"][0]["comment"] == ""


def test_modeling_assistant_rejects_unknown_iri(workspace) -> None:
    _session, _user, ks = workspace
    editor.apply_edit(ks.graph_iri, ks.base_iri, {"op": "add_class", "label": "Device"})
    with pytest.raises(modeling_assistant.SuggestionError, match="existing ontology IRI"):
        modeling_assistant.validate_operations(ks.graph_iri, [{
            "op": "update_class", "iri": "urn:hallucinated", "label": "Asset",
        }])


def test_sql_commit_failure_compensates_graph_changes(workspace, monkeypatch) -> None:
    session, user, ks = workspace
    before = workbench.ontology_revision(ks.graph_iri)
    real_commit = session.commit
    attempted = False

    def fail_once():
        nonlocal attempted
        if not attempted:
            attempted = True
            raise RuntimeError("database unavailable")
        return real_commit()

    monkeypatch.setattr(session, "commit", fail_once)
    with pytest.raises(RuntimeError, match="database unavailable"):
        _changes(
            session, user, ks,
            [{"op": "add_class", "label": "Pump"}],
            expected_revision=before,
        )

    assert workbench.ontology_revision(ks.graph_iri) == before
    assert schema.build_view(ks.graph_iri)["classes"] == []


def test_cache_invalidation_failure_does_not_split_sql_and_rdf(workspace, monkeypatch) -> None:
    session, user, ks = workspace
    before = workbench.ontology_revision(ks.graph_iri)
    monkeypatch.setattr(
        retrieval,
        "invalidate",
        lambda _graph_iri: (_ for _ in ()).throw(RuntimeError("cache unavailable")),
    )

    response = _changes(
        session, user, ks,
        [{"op": "add_class", "label": "Pump"}],
        expected_revision=before,
    )

    assert response["applied"] == 1
    assert response["revision"] != before
    assert workbench.ontology_revision(ks.graph_iri) == response["revision"]
    assert [item["label"] for item in schema.build_view(ks.graph_iri)["classes"]] == ["Pump"]
    events = session.exec(select(AuditEvent).where(
        AuditEvent.action == "ontology.change_set",
    )).all()
    assert len(events) == 1
    assert events[0].id == response["audit_event_id"]


def _seed_reset_state(session, ks) -> tuple[Document, Conflict, tuple[str, str, str]]:
    tbox_iri = ks.graph_iri
    abox_iri = workbench.abox_iri_for(tbox_iri)
    vocabulary_iri = skos.graph_iri_for(ks)
    store.add_triples(tbox_iri, [
        (NamedNode("urn:reset:Class"), RDF_TYPE, NamedNode("http://www.w3.org/2002/07/owl#Class")),
    ])
    store.add_triples(abox_iri, [
        (NamedNode("urn:reset:individual"), RDF_TYPE, OWL_NAMED_INDIVIDUAL),
    ])
    store.add_triples(vocabulary_iri, [
        (NamedNode("urn:reset:term"), RDFS_LABEL, Literal("Term")),
    ])
    extracted_at = datetime.now(timezone.utc)
    document = Document(
        knowledge_system_id=ks.id,
        sha256="reset-document",
        original_filename="reset.txt",
        ext="txt",
        tbox_extracted_at=extracted_at,
        abox_extracted_at=extracted_at,
    )
    conflict = Conflict(
        knowledge_system_id=ks.id,
        signature="reset-conflict",
        ctype="cycle",
        status="open",
        title="Reset conflict",
    )
    ks.class_count = 1
    session.add_all([document, conflict, ks])
    session.commit()
    session.refresh(document)
    session.refresh(conflict)
    return document, conflict, (tbox_iri, abox_iri, vocabulary_iri)


def test_reset_is_one_atomic_commit_and_cache_failure_is_best_effort(
    workspace, monkeypatch,
) -> None:
    session, user, ks = workspace
    document, conflict, graph_iris = _seed_reset_state(session, ks)
    real_commit = session.commit
    commits = 0

    def counted_commit() -> None:
        nonlocal commits
        commits += 1
        real_commit()

    invalidated_after_commit = False

    def failed_invalidation(graph_iri: str) -> None:
        nonlocal invalidated_after_commit
        invalidated_after_commit = bool(
            session.exec(select(AuditEvent).where(
                AuditEvent.action == "ontology.reset",
            )).all()
        )
        assert graph_iri == ks.graph_iri
        raise RuntimeError("cache unavailable")

    monkeypatch.setattr(session, "commit", counted_commit)
    monkeypatch.setattr(retrieval, "invalidate", failed_invalidation)

    result = ontology_api.reset_ontology(
        ontology_api.ResetOntologyRequest(confirm=True), ks, user, session,
    )

    assert commits == 1
    assert invalidated_after_commit is True
    assert result["removed_triples"] == {
        "ontology": 1,
        "instances": 1,
        "vocabulary": 1,
    }
    assert result["documents_reset"] == 1
    assert all(store.read_triples(graph_iri) == [] for graph_iri in graph_iris)
    assert session.get(Conflict, conflict.id) is None
    session.refresh(document)
    session.refresh(ks)
    assert document.tbox_extracted_at is None
    assert document.abox_extracted_at is None
    assert (ks.class_count, ks.property_count, ks.axiom_count) == (0, 0, 0)
    events = session.exec(select(AuditEvent).where(
        AuditEvent.action == "ontology.reset",
    )).all()
    assert len(events) == 3
    assert len({event.group_id for event in events}) == 1


def test_reset_sql_failure_compensates_all_graphs_and_relational_state(
    workspace, monkeypatch,
) -> None:
    session, user, ks = workspace
    document, conflict, graph_iris = _seed_reset_state(session, ks)
    before_graphs = [set(store.read_triples(graph_iri)) for graph_iri in graph_iris]
    real_commit = session.commit

    def fail_commit() -> None:
        raise RuntimeError("injected reset commit failure")

    monkeypatch.setattr(session, "commit", fail_commit)
    with pytest.raises(RuntimeError, match="injected reset commit failure"):
        ontology_api.reset_ontology(
            ontology_api.ResetOntologyRequest(confirm=True), ks, user, session,
        )

    monkeypatch.setattr(session, "commit", real_commit)
    session.expire_all()
    assert [set(store.read_triples(graph_iri)) for graph_iri in graph_iris] == before_graphs
    assert session.get(Conflict, conflict.id).status == "open"
    restored_document = session.get(Document, document.id)
    assert restored_document.tbox_extracted_at is not None
    assert restored_document.abox_extracted_at is not None
    assert session.get(KnowledgeSystem, ks.id).class_count == 1
    assert session.exec(select(AuditEvent).where(
        AuditEvent.action == "ontology.reset",
    )).all() == []


def test_reset_true_noop_writes_no_audit_commit_or_cache(workspace, monkeypatch) -> None:
    session, user, ks = workspace
    commits = 0

    def unexpected_commit() -> None:
        nonlocal commits
        commits += 1

    monkeypatch.setattr(session, "commit", unexpected_commit)
    monkeypatch.setattr(
        retrieval,
        "invalidate",
        lambda _graph_iri: pytest.fail("no-op reset must not invalidate cache"),
    )

    result = ontology_api.reset_ontology(
        ontology_api.ResetOntologyRequest(confirm=True), ks, user, session,
    )

    assert commits == 0
    assert result["removed_triples"] == {
        "ontology": 0,
        "instances": 0,
        "vocabulary": 0,
    }
    assert result["documents_reset"] == 0
    assert not any(result["removed_rows"].values())
    assert session.exec(select(AuditEvent)).all() == []


@pytest.mark.parametrize("race", ["status", "options"])
def test_conflict_resolution_reloads_state_and_options_after_graph_lock(
    tmp_path, monkeypatch, race: str,
) -> None:
    database = create_engine(
        f"sqlite:///{tmp_path / 'conflict-race.db'}",
        connect_args={"check_same_thread": False},
    )
    SQLModel.metadata.create_all(database)
    monkeypatch.setattr(store, "_store", Store())
    store._graph_locks.clear()
    store._recorders.clear()
    with Session(database, expire_on_commit=False) as session:
        user = User(username="racer", password_hash="unused")
        session.add(user)
        session.commit()
        ks = KnowledgeSystem(
            name="Race",
            public_id=f"race-{race}",
            owner_id=user.id,
            graph_iri=f"urn:race:{race}:tbox",
            base_iri=f"urn:race:{race}:onto:",
        )
        session.add(ks)
        session.commit()
        cls = editor.apply_edit(
            ks.graph_iri, ks.base_iri, {"op": "add_class", "label": "Device"},
        )
        conflict = Conflict(
            knowledge_system_id=ks.id,
            signature=f"race-{race}",
            ctype="duplicate",
            status="open",
            title="Stale decision",
            payload={
                "entities": [],
                "resolutions": [{
                    "id": "old-choice",
                    "label": "Old choice",
                    "op": {"op": "update_class", "iri": cls, "comment": "stale"},
                }],
            },
        )
        session.add(conflict)
        session.commit()
        session.refresh(conflict)
        session.commit()
        before = workbench.ontology_revision(ks.graph_iri)
        original_capture = store.capture
        raced = False

        @contextmanager
        def racing_capture(graph_iri: str, *, revert_on_error: bool = False):
            nonlocal raced
            if graph_iri == ks.graph_iri and not raced:
                raced = True
                with Session(database, expire_on_commit=False) as competing:
                    fresh = competing.get(Conflict, conflict.id)
                    if race == "status":
                        fresh.status = "dismissed"
                        fresh.resolution = "dismissed"
                    else:
                        fresh.payload = {
                            "entities": [],
                            "resolutions": [{
                                "id": "new-choice",
                                "label": "New choice",
                                "op": {"op": "update_class", "iri": cls, "comment": "fresh"},
                            }],
                        }
                    competing.add(fresh)
                    competing.commit()
            with original_capture(graph_iri, revert_on_error=revert_on_error) as capture:
                yield capture

        monkeypatch.setattr(store, "capture", racing_capture)
        with pytest.raises(HTTPException) as stale:
            conflicts_api.resolve_conflict(
                conflict.id,
                conflicts_api.ResolveRequest(resolution_id="old-choice"),
                ks,
                user,
                session,
            )

        assert stale.value.status_code == (409 if race == "status" else 400)
        assert workbench.ontology_revision(ks.graph_iri) == before
        assert schema.build_view(ks.graph_iri)["classes"][0]["comment"] == ""
        assert session.exec(select(AuditEvent)).all() == []


def test_dismiss_is_one_transaction_and_repeated_dismiss_is_noop(workspace, monkeypatch) -> None:
    session, user, ks = workspace
    conflict = Conflict(
        knowledge_system_id=ks.id,
        signature="dismiss-once",
        ctype="cycle",
        status="open",
        title="Dismiss once",
    )
    session.add(conflict)
    session.commit()
    session.refresh(conflict)
    real_commit = session.commit
    commits = 0

    def counted_commit() -> None:
        nonlocal commits
        commits += 1
        real_commit()

    monkeypatch.setattr(session, "commit", counted_commit)
    first = conflicts_api.dismiss_conflict(conflict.id, ks, user, session)
    first_resolved_at = first.resolved_at
    second = conflicts_api.dismiss_conflict(conflict.id, ks, user, session)

    assert commits == 1
    assert second.status == "dismissed"
    assert second.resolved_at == first_resolved_at
    events = session.exec(select(AuditEvent).where(
        AuditEvent.action == "conflict.dismiss",
    )).all()
    assert len(events) == 1


def test_dismiss_commit_failure_rolls_back_status_and_audit(workspace, monkeypatch) -> None:
    session, user, ks = workspace
    conflict = Conflict(
        knowledge_system_id=ks.id,
        signature="dismiss-failure",
        ctype="cycle",
        status="open",
        title="Dismiss failure",
    )
    session.add(conflict)
    session.commit()
    session.refresh(conflict)
    real_commit = session.commit
    monkeypatch.setattr(
        session,
        "commit",
        lambda: (_ for _ in ()).throw(RuntimeError("injected dismiss commit failure")),
    )

    with pytest.raises(RuntimeError, match="injected dismiss commit failure"):
        conflicts_api.dismiss_conflict(conflict.id, ks, user, session)

    monkeypatch.setattr(session, "commit", real_commit)
    session.expire_all()
    assert session.get(Conflict, conflict.id).status == "open"
    assert session.exec(select(AuditEvent).where(
        AuditEvent.action == "conflict.dismiss",
    )).all() == []


def test_preview_reports_and_commit_blocks_new_structural_errors(workspace) -> None:
    session, user, ks = workspace
    a = editor.apply_edit(ks.graph_iri, ks.base_iri, {"op": "add_class", "label": "A"})
    b = editor.apply_edit(ks.graph_iri, ks.base_iri, {"op": "add_class", "label": "B"})
    revision = workbench.ontology_revision(ks.graph_iri)
    operations = [
        {"op": "add_axiom", "type": "subclass", "sub": a, "super": b},
        {"op": "add_axiom", "type": "subclass", "sub": b, "super": a},
    ]

    preview = _changes(
        session, user, ks, operations,
        expected_revision=revision,
        dry_run=True,
    )
    assert preview["structural_validation"]["committable"] is False
    assert preview["structural_validation"]["new_error_count"] == 1
    assert preview["structural_validation"]["error_count"] == 1
    assert workbench.ontology_revision(ks.graph_iri) == revision

    with pytest.raises(HTTPException) as blocked:
        _changes(session, user, ks, operations, expected_revision=revision)
    assert blocked.value.status_code == 422
    assert blocked.value.detail["code"] == "ontology_structural_validation_failed"
    assert workbench.ontology_revision(ks.graph_iri) == revision


def test_preview_and_commit_block_new_abox_disjoint_error(workspace) -> None:
    session, user, ks = workspace
    left = editor.apply_edit(ks.graph_iri, ks.base_iri, {"op": "add_class", "label": "Left"})
    right = editor.apply_edit(ks.graph_iri, ks.base_iri, {"op": "add_class", "label": "Right"})
    source = editor.apply_edit(ks.graph_iri, ks.base_iri, {"op": "add_class", "label": "Duplicate"})
    store.add_triples(ks.graph_iri, [(NamedNode(left), OWL_DISJOINT_WITH, NamedNode(right))])
    individual = NamedNode("urn:workbench:individual:both-after-merge")
    store.add_triples(workbench.abox_iri_for(ks.graph_iri), [
        (individual, RDF_TYPE, OWL_NAMED_INDIVIDUAL),
        (individual, RDF_TYPE, NamedNode(left)),
        (individual, RDF_TYPE, NamedNode(source)),
    ])
    revision = workbench.ontology_revision(ks.graph_iri)
    operation = {"op": "merge_classes", "source": source, "target": right}

    preview = _changes(
        session, user, ks, [operation], expected_revision=revision, dry_run=True,
    )
    assert preview["structural_validation"]["committable"] is False
    assert preview["structural_validation"]["new_error_count"] == 1
    assert preview["abox_validation"]["counts"]["error"] == 1

    with pytest.raises(HTTPException) as blocked:
        _changes(
            session, user, ks, [operation], expected_revision=revision,
            confirm_destructive=True,
        )
    assert blocked.value.status_code == 422
    assert workbench.ontology_revision(ks.graph_iri) == revision

def test_existing_structural_error_does_not_block_incremental_repairs(workspace) -> None:
    session, user, ks = workspace
    a = editor.apply_edit(ks.graph_iri, ks.base_iri, {"op": "add_class", "label": "A"})
    b = editor.apply_edit(ks.graph_iri, ks.base_iri, {"op": "add_class", "label": "B"})
    editor.apply_edit(ks.graph_iri, ks.base_iri, {
        "op": "add_axiom", "type": "subclass", "sub": a, "super": b,
    })
    editor.apply_edit(ks.graph_iri, ks.base_iri, {
        "op": "add_axiom", "type": "subclass", "sub": b, "super": a,
    })
    revision = workbench.ontology_revision(ks.graph_iri)

    response = _changes(
        session, user, ks,
        [{"op": "update_class", "iri": a, "comment": "Repair in progress"}],
        expected_revision=revision,
    )

    assert response["structural_validation"]["valid"] is False
    assert response["structural_validation"]["committable"] is True
    assert response["structural_validation"]["new_error_count"] == 0


def test_conflict_resolution_cannot_introduce_new_abox_error(workspace) -> None:
    session, user, ks = workspace
    left = editor.apply_edit(ks.graph_iri, ks.base_iri, {"op": "add_class", "label": "Left"})
    right = editor.apply_edit(ks.graph_iri, ks.base_iri, {"op": "add_class", "label": "Right"})
    source = editor.apply_edit(ks.graph_iri, ks.base_iri, {"op": "add_class", "label": "Duplicate"})
    store.add_triples(ks.graph_iri, [(NamedNode(left), OWL_DISJOINT_WITH, NamedNode(right))])
    individual = NamedNode("urn:workbench:individual:conflict-merge")
    store.add_triples(workbench.abox_iri_for(ks.graph_iri), [
        (individual, RDF_TYPE, OWL_NAMED_INDIVIDUAL),
        (individual, RDF_TYPE, NamedNode(left)),
        (individual, RDF_TYPE, NamedNode(source)),
    ])
    conflict = Conflict(
        knowledge_system_id=ks.id,
        signature="test-merge",
        ctype="duplicate",
        severity="warning",
        status="open",
        title="Possible duplicate",
        detail="test",
        payload={
            "entities": [],
            "resolutions": [{
                "id": "merge",
                "label": "Merge classes",
                "op": {"op": "merge_classes", "source": source, "target": right},
            }],
        },
    )
    session.add(conflict)
    session.commit()
    session.refresh(conflict)
    before = workbench.ontology_revision(ks.graph_iri)

    with pytest.raises(HTTPException) as blocked:
        conflicts_api.resolve_conflict(
            conflict.id,
            conflicts_api.ResolveRequest(resolution_id="merge"),
            ks,
            user,
            session,
        )

    assert blocked.value.status_code == 422
    assert workbench.ontology_revision(ks.graph_iri) == before
    session.refresh(conflict)
    assert conflict.status == "open"
    assert session.exec(select(AuditEvent)).all() == []


def test_rdf_import_new_disjoint_abox_error_rolls_back_both_graphs(workspace) -> None:
    session, user, ks = workspace
    before = workbench.ontology_revision(ks.graph_iri)
    turtle = b'''\
@prefix ex: <urn:workbench:onto:> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
ex:Left a owl:Class ; owl:disjointWith ex:Right .
ex:Right a owl:Class .
ex:both a owl:NamedIndividual, ex:Left, ex:Right .
'''
    from io import BytesIO
    from starlette.datastructures import UploadFile

    with pytest.raises(HTTPException) as blocked:
        rdf_import_api.import_rdf(
            UploadFile(filename="invalid.ttl", file=BytesIO(turtle)),
            "auto",
            "merge",
            "turtle",
            None,
            ks,
            user,
            session,
        )

    assert blocked.value.status_code == 422
    assert workbench.ontology_revision(ks.graph_iri) == before
    assert store.read_triples(ks.graph_iri) == []
    assert store.read_triples(workbench.abox_iri_for(ks.graph_iri)) == []
    assert session.exec(select(AuditEvent)).all() == []


def _blank_ids(graph_iri: str) -> set[str]:
    return {
        term.value
        for subject, _predicate, obj in store.read_triples(graph_iri)
        for term in (subject, obj)
        if isinstance(term, BlankNode)
    }


def test_delete_property_garbage_collects_domain_and_range_unions(workspace) -> None:
    _session, _user, ks = workspace
    a = editor.apply_edit(ks.graph_iri, ks.base_iri, {"op": "add_class", "label": "A"})
    b = editor.apply_edit(ks.graph_iri, ks.base_iri, {"op": "add_class", "label": "B"})
    prop = editor.apply_edit(ks.graph_iri, ks.base_iri, {
        "op": "add_property", "kind": "object", "label": "relates",
        "domain_members": [a, b], "range_members": [a, b],
    })
    assert _blank_ids(ks.graph_iri)

    editor.apply_edit(ks.graph_iri, ks.base_iri, {
        "op": "delete_property", "iri": prop,
    })

    view = schema.build_view(ks.graph_iri)
    assert view["object_properties"] == []
    assert _blank_ids(ks.graph_iri) == set()


def test_merge_properties_preserves_union_members_and_collects_source_blanks(workspace) -> None:
    _session, _user, ks = workspace
    classes = [
        editor.apply_edit(ks.graph_iri, ks.base_iri, {"op": "add_class", "label": label})
        for label in ("A", "B", "C", "D")
    ]
    a, b, c, d = classes
    source = editor.apply_edit(ks.graph_iri, ks.base_iri, {
        "op": "add_property", "kind": "object", "label": "special relation",
        "domain_members": [a, b], "range_members": [b, c],
    })
    target = editor.apply_edit(ks.graph_iri, ks.base_iri, {
        "op": "add_property", "kind": "object", "label": "relation",
        "domain_members": [c, d], "range_members": [a, d],
    })
    # Blank IDs are hashes, so identify the source expressions via their direct slots.
    source_roots = {
        value.value
        for predicate in (
            editor.RDFS_DOMAIN,
            editor.RDFS_RANGE,
        )
        for value in store.object_terms(ks.graph_iri, NamedNode(source), predicate)
        if isinstance(value, BlankNode)
    }
    assert source_roots

    editor.apply_edit(ks.graph_iri, ks.base_iri, {
        "op": "merge_properties", "sources": [source], "target": target,
    })

    item = next(
        item for item in schema.build_view(ks.graph_iri)["object_properties"]
        if item["iri"] == target
    )
    assert set(item["domain_members"]) == {a, b, c, d}
    assert set(item["range_members"]) == {a, b, c, d}
    after_blanks = _blank_ids(ks.graph_iri)
    assert source_roots.isdisjoint(after_blanks)


def test_property_union_rejects_unknown_class_iri_without_writing(workspace) -> None:
    _session, _user, ks = workspace
    known = editor.apply_edit(ks.graph_iri, ks.base_iri, {
        "op": "add_class", "label": "Known",
    })
    prop = editor.apply_edit(ks.graph_iri, ks.base_iri, {
        "op": "add_property", "kind": "object", "label": "relates",
    })
    before = workbench.ontology_revision(ks.graph_iri)

    with pytest.raises(editor.EditError, match="Class not found"):
        editor.apply_edit(ks.graph_iri, ks.base_iri, {
            "op": "set_property_union", "iri": prop, "slot": "domain",
            "members": [known, "urn:missing:Class"],
        })

    assert workbench.ontology_revision(ks.graph_iri) == before


@pytest.mark.parametrize(("first_kind", "second_kind"), [
    ("data", "object"),
    ("object", "data"),
])
def test_add_property_rejects_redeclaring_same_label_with_opposite_kind(
    workspace, first_kind: str, second_kind: str,
) -> None:
    _session, _user, ks = workspace
    iri = editor.apply_edit(ks.graph_iri, ks.base_iri, {
        "op": "add_property", "kind": first_kind, "label": "serial value",
    })
    before = workbench.ontology_revision(ks.graph_iri)

    with pytest.raises(editor.EditError, match="already exists"):
        editor.apply_edit(ks.graph_iri, ks.base_iri, {
            "op": "add_property", "kind": second_kind, "label": "serial value",
        })

    assert workbench.ontology_revision(ks.graph_iri) == before
    view = schema.build_view(ks.graph_iri)
    expected = "object_properties" if first_kind == "object" else "data_properties"
    opposite = "data_properties" if first_kind == "object" else "object_properties"
    assert [item["iri"] for item in view[expected]] == [iri]
    assert view[opposite] == []


def _provenance_rows(session, ks_id: int) -> list[AboxProvenance]:
    return list(session.exec(select(AboxProvenance).where(
        AboxProvenance.knowledge_system_id == ks_id,
    )).all())


def test_merge_property_migrates_every_source_and_records_manual_provenance(workspace) -> None:
    session, user, ks = workspace
    cls = editor.apply_edit(ks.graph_iri, ks.base_iri, {
        "op": "add_class", "label": "Device",
    })
    source = editor.apply_edit(ks.graph_iri, ks.base_iri, {
        "op": "add_property", "kind": "object", "label": "uses old",
        "domain": cls, "range": cls,
    })
    target = editor.apply_edit(ks.graph_iri, ks.base_iri, {
        "op": "add_property", "kind": "object", "label": "uses",
        "domain": cls, "range": cls,
    })
    subject = "urn:workbench:individual:source"
    obj = "urn:workbench:individual:target"
    abox_iri = workbench.abox_iri_for(ks.graph_iri)
    store.add_triples(abox_iri, [
        (NamedNode(subject), RDF_TYPE, OWL_NAMED_INDIVIDUAL),
        (NamedNode(subject), RDF_TYPE, NamedNode(cls)),
        (NamedNode(obj), RDF_TYPE, OWL_NAMED_INDIVIDUAL),
        (NamedNode(obj), RDF_TYPE, NamedNode(cls)),
        (NamedNode(subject), NamedNode(source), NamedNode(obj)),
    ])
    old_key = abox_provenance.obj_key(subject, source, obj)
    new_key = abox_provenance.obj_key(subject, target, obj)
    # Two independent extraction mentions of the source fact must both survive.
    session.add_all([
        AboxProvenance(
            knowledge_system_id=ks.id, fact_key=old_key, chunk_id=101,
            job_id=11, method="extraction", actor_name="extractor-a",
        ),
        AboxProvenance(
            knowledge_system_id=ks.id, fact_key=old_key, chunk_id=102,
            job_id=12, method="extraction", actor_name="extractor-b",
        ),
    ])
    session.commit()

    response = _changes(
        session, user, ks,
        [{"op": "merge_properties", "sources": [source], "target": target}],
        expected_revision=workbench.ontology_revision(ks.graph_iri),
        confirm_destructive=True,
        reason="Consolidate relation",
    )

    rows = _provenance_rows(session, ks.id)
    assert old_key not in {row.fact_key for row in rows}
    target_rows = [row for row in rows if row.fact_key == new_key]
    assert {(row.chunk_id, row.job_id) for row in target_rows if row.chunk_id} == {
        (101, 11), (102, 12),
    }
    assert len([row for row in target_rows if row.method == "manual"]) == 1
    assert target_rows[-1].fact_key == new_key
    assert response["diff"]["counts"]["abox_removed"] == 1
    assert response["diff"]["counts"]["abox_added"] == 1


def test_merge_property_preserves_sources_when_target_assertion_already_exists(workspace) -> None:
    session, user, ks = workspace
    cls = editor.apply_edit(ks.graph_iri, ks.base_iri, {
        "op": "add_class", "label": "Device",
    })
    source = editor.apply_edit(ks.graph_iri, ks.base_iri, {
        "op": "add_property", "kind": "object", "label": "old relation",
    })
    target = editor.apply_edit(ks.graph_iri, ks.base_iri, {
        "op": "add_property", "kind": "object", "label": "relation",
    })
    subject = "urn:workbench:individual:s"
    obj = "urn:workbench:individual:o"
    abox_iri = workbench.abox_iri_for(ks.graph_iri)
    store.add_triples(abox_iri, [
        (NamedNode(subject), RDF_TYPE, OWL_NAMED_INDIVIDUAL),
        (NamedNode(subject), RDF_TYPE, NamedNode(cls)),
        (NamedNode(obj), RDF_TYPE, OWL_NAMED_INDIVIDUAL),
        (NamedNode(obj), RDF_TYPE, NamedNode(cls)),
        (NamedNode(subject), NamedNode(source), NamedNode(obj)),
        (NamedNode(subject), NamedNode(target), NamedNode(obj)),
    ])
    old_key = abox_provenance.obj_key(subject, source, obj)
    new_key = abox_provenance.obj_key(subject, target, obj)
    session.add_all([
        AboxProvenance(
            knowledge_system_id=ks.id, fact_key=old_key, chunk_id=201,
            job_id=21, method="extraction", actor_name="old-source",
        ),
        AboxProvenance(
            knowledge_system_id=ks.id, fact_key=new_key, chunk_id=202,
            job_id=22, method="extraction", actor_name="target-source",
        ),
    ])
    session.commit()

    response = _changes(
        session, user, ks,
        [{"op": "merge_properties", "sources": [source], "target": target}],
        expected_revision=workbench.ontology_revision(ks.graph_iri),
        confirm_destructive=True,
    )

    assert response["diff"]["counts"]["abox_removed"] == 1
    # The target assertion existed before the batch, so it is not in the net added diff.
    assert response["diff"]["counts"]["abox_added"] == 0
    rows = _provenance_rows(session, ks.id)
    assert old_key not in {row.fact_key for row in rows}
    target_rows = [row for row in rows if row.fact_key == new_key]
    assert {(row.chunk_id, row.job_id) for row in target_rows if row.chunk_id} == {
        (201, 21), (202, 22),
    }
    assert len([row for row in target_rows if row.method == "manual"]) == 1


def test_delete_property_removes_all_assertion_provenance_sources(workspace) -> None:
    session, user, ks = workspace
    cls = editor.apply_edit(ks.graph_iri, ks.base_iri, {
        "op": "add_class", "label": "Device",
    })
    prop = editor.apply_edit(ks.graph_iri, ks.base_iri, {
        "op": "add_property", "kind": "data", "label": "serial",
        "domain": cls, "range": "string",
    })
    subject = "urn:workbench:individual:device"
    value = "SN-42"
    abox_iri = workbench.abox_iri_for(ks.graph_iri)
    store.add_triples(abox_iri, [
        (NamedNode(subject), RDF_TYPE, OWL_NAMED_INDIVIDUAL),
        (NamedNode(subject), RDF_TYPE, NamedNode(cls)),
        (NamedNode(subject), NamedNode(prop), Literal(value)),
    ])
    key = abox_provenance.data_key(subject, prop, value)
    session.add_all([
        AboxProvenance(
            knowledge_system_id=ks.id, fact_key=key, chunk_id=301,
            method="extraction", actor_name="extractor",
        ),
        AboxProvenance(
            knowledge_system_id=ks.id, fact_key=key,
            method="manual", actor_name="reviewer",
        ),
    ])
    session.commit()

    _changes(
        session, user, ks,
        [{"op": "delete_property", "iri": prop}],
        expected_revision=workbench.ontology_revision(ks.graph_iri),
        confirm_destructive=True,
    )

    assert key not in {row.fact_key for row in _provenance_rows(session, ks.id)}
    assert not store.has_triple(
        abox_iri, NamedNode(subject), NamedNode(prop), Literal(value),
    )


def test_merge_class_keeps_identity_sources_and_adds_governance_provenance(workspace) -> None:
    session, user, ks = workspace
    source = editor.apply_edit(ks.graph_iri, ks.base_iri, {
        "op": "add_class", "label": "Legacy Device",
    })
    target = editor.apply_edit(ks.graph_iri, ks.base_iri, {
        "op": "add_class", "label": "Device",
    })
    individual = "urn:workbench:individual:device"
    abox_iri = workbench.abox_iri_for(ks.graph_iri)
    store.add_triples(abox_iri, [
        (NamedNode(individual), RDF_TYPE, OWL_NAMED_INDIVIDUAL),
        (NamedNode(individual), RDF_TYPE, NamedNode(source)),
    ])
    key = abox_provenance.ind_key(individual)
    session.add_all([
        AboxProvenance(
            knowledge_system_id=ks.id, fact_key=key, chunk_id=351,
            job_id=31, method="extraction", actor_name="extractor-a",
        ),
        AboxProvenance(
            knowledge_system_id=ks.id, fact_key=key, chunk_id=352,
            job_id=32, method="extraction", actor_name="extractor-b",
        ),
    ])
    session.commit()

    _changes(
        session, user, ks,
        [{"op": "merge_classes", "source": source, "target": target}],
        expected_revision=workbench.ontology_revision(ks.graph_iri),
        confirm_destructive=True,
    )

    rows = [row for row in _provenance_rows(session, ks.id) if row.fact_key == key]
    assert {(row.chunk_id, row.job_id) for row in rows if row.chunk_id} == {
        (351, 31), (352, 32),
    }
    assert len([row for row in rows if row.method == "manual"]) == 1
    assert store.has_triple(
        abox_iri, NamedNode(individual), RDF_TYPE, NamedNode(target),
    )


def test_delete_class_removes_identity_and_incoming_assertion_provenance(workspace) -> None:
    session, user, ks = workspace
    owner_cls = editor.apply_edit(ks.graph_iri, ks.base_iri, {
        "op": "add_class", "label": "Owner",
    })
    doomed_cls = editor.apply_edit(ks.graph_iri, ks.base_iri, {
        "op": "add_class", "label": "Doomed",
    })
    prop = editor.apply_edit(ks.graph_iri, ks.base_iri, {
        "op": "add_property", "kind": "object", "label": "owns",
    })
    owner = "urn:workbench:individual:owner"
    doomed = "urn:workbench:individual:doomed"
    abox_iri = workbench.abox_iri_for(ks.graph_iri)
    store.add_triples(abox_iri, [
        (NamedNode(owner), RDF_TYPE, OWL_NAMED_INDIVIDUAL),
        (NamedNode(owner), RDF_TYPE, NamedNode(owner_cls)),
        (NamedNode(doomed), RDF_TYPE, OWL_NAMED_INDIVIDUAL),
        (NamedNode(doomed), RDF_TYPE, NamedNode(doomed_cls)),
        (NamedNode(owner), NamedNode(prop), NamedNode(doomed)),
    ])
    doomed_identity = abox_provenance.ind_key(doomed)
    incoming = abox_provenance.obj_key(owner, prop, doomed)
    owner_identity = abox_provenance.ind_key(owner)
    session.add_all([
        AboxProvenance(knowledge_system_id=ks.id, fact_key=doomed_identity, chunk_id=401),
        AboxProvenance(knowledge_system_id=ks.id, fact_key=incoming, chunk_id=402),
        AboxProvenance(knowledge_system_id=ks.id, fact_key=owner_identity, chunk_id=403),
    ])
    session.commit()

    _changes(
        session, user, ks,
        [{"op": "delete_class", "iri": doomed_cls}],
        expected_revision=workbench.ontology_revision(ks.graph_iri),
        confirm_destructive=True,
    )

    keys = {row.fact_key for row in _provenance_rows(session, ks.id)}
    assert doomed_identity not in keys
    assert incoming not in keys
    assert owner_identity in keys


def test_abox_provenance_failure_rolls_back_sql_and_both_graphs(workspace, monkeypatch) -> None:
    session, user, ks = workspace
    cls = editor.apply_edit(ks.graph_iri, ks.base_iri, {
        "op": "add_class", "label": "Device",
    })
    prop = editor.apply_edit(ks.graph_iri, ks.base_iri, {
        "op": "add_property", "kind": "data", "label": "serial",
    })
    subject = "urn:workbench:individual:device"
    abox_iri = workbench.abox_iri_for(ks.graph_iri)
    store.add_triples(abox_iri, [
        (NamedNode(subject), RDF_TYPE, OWL_NAMED_INDIVIDUAL),
        (NamedNode(subject), RDF_TYPE, NamedNode(cls)),
        (NamedNode(subject), NamedNode(prop), Literal("SN-42")),
    ])
    before_revision = workbench.ontology_revision(ks.graph_iri)
    before_tbox = set(store.read_triples(ks.graph_iri))
    before_abox = set(store.read_triples(abox_iri))
    monkeypatch.setattr(
        ontology_api.statement_provenance,
        "record_abox_diff",
        lambda *args, **kwargs: (_ for _ in ()).throw(RuntimeError("provenance unavailable")),
    )

    with pytest.raises(RuntimeError, match="provenance unavailable"):
        _changes(
            session, user, ks,
            [{"op": "delete_property", "iri": prop}],
            expected_revision=before_revision,
            confirm_destructive=True,
        )

    assert workbench.ontology_revision(ks.graph_iri) == before_revision
    assert set(store.read_triples(ks.graph_iri)) == before_tbox
    assert set(store.read_triples(abox_iri)) == before_abox
    assert session.exec(select(AuditEvent)).all() == []


@pytest.mark.parametrize("operation", [
    {"op": "add_axiom", "type": "subclass", "sub": "urn:missing:Sub", "super": "Known"},
    {"op": "add_axiom", "type": "disjoint", "a": "urn:missing:Left", "b": "Known"},
    {"op": "add_axiom", "type": "equivalent", "a": "Known", "b": "urn:missing:Right"},
])
def test_axiom_unknown_iri_is_a_reference_not_implicit_class_creation(
    workspace, operation: dict,
) -> None:
    _session, _user, ks = workspace
    editor.apply_edit(ks.graph_iri, ks.base_iri, {"op": "add_class", "label": "Known"})
    before = workbench.ontology_revision(ks.graph_iri)

    with pytest.raises(editor.EditError, match="Class not found"):
        editor.apply_edit(ks.graph_iri, ks.base_iri, operation)

    assert workbench.ontology_revision(ks.graph_iri) == before


@pytest.mark.parametrize("payload", [
    {"op": "add_property", "kind": "object", "label": "uses", "domain": "urn:missing:Domain"},
    {"op": "add_property", "kind": "object", "label": "uses", "range": "urn:missing:Range"},
])
def test_property_unknown_class_iri_does_not_leave_a_partial_property(
    workspace, payload: dict,
) -> None:
    _session, _user, ks = workspace
    before = workbench.ontology_revision(ks.graph_iri)

    # Direct editor callers still receive a clear error.  Atomic API/MCP callers wrap
    # this in graph captures, so test that path below before asserting no persistence.
    with pytest.raises(editor.EditError, match="Class not found"):
        with store.capture(ks.graph_iri, revert_on_error=True):
            editor.apply_edit(ks.graph_iri, ks.base_iri, payload)

    assert workbench.ontology_revision(ks.graph_iri) == before
    assert schema.build_view(ks.graph_iri)["object_properties"] == []


@pytest.mark.parametrize("op_name", ["merge_properties", "subordinate_properties"])
@pytest.mark.parametrize("case", ["missing_source", "data_source", "missing_target", "data_target"])
def test_object_property_composition_rejects_missing_or_mistyped_references_before_writing(
    workspace, op_name: str, case: str,
) -> None:
    _session, _user, ks = workspace
    source = editor.apply_edit(ks.graph_iri, ks.base_iri, {
        "op": "add_property", "kind": "object", "label": "specific relation",
    })
    target = editor.apply_edit(ks.graph_iri, ks.base_iri, {
        "op": "add_property", "kind": "object", "label": "general relation",
    })
    data_prop = editor.apply_edit(ks.graph_iri, ks.base_iri, {
        "op": "add_property", "kind": "data", "label": "serial value",
    })
    payload = {"op": op_name, "sources": [source], "target": target}
    if case == "missing_source":
        payload["sources"] = ["urn:missing:source"]
    elif case == "data_source":
        payload["sources"] = [data_prop]
    elif case == "missing_target":
        payload["target"] = "urn:missing:target"
    else:
        payload["target"] = data_prop
    before = workbench.ontology_revision(ks.graph_iri)

    with pytest.raises(editor.EditError, match="property"):
        editor.apply_edit(ks.graph_iri, ks.base_iri, payload)

    assert workbench.ontology_revision(ks.graph_iri) == before
    view = schema.build_view(ks.graph_iri)
    assert {item["iri"] for item in view["object_properties"]} == {source, target}
    assert {item["iri"] for item in view["data_properties"]} == {data_prop}


@pytest.mark.parametrize("operation", [
    {"op": "delete_axiom", "type": "subclass", "sub": "urn:missing:A", "super": "urn:missing:B"},
    {"op": "delete_axiom", "type": "disjoint", "a": "urn:missing:A", "b": "urn:missing:B"},
    {"op": "delete_axiom", "type": "equivalent", "a": "urn:missing:A", "b": "urn:missing:B"},
])
def test_delete_missing_axiom_is_rejected_without_revision_or_audit_change(
    workspace, operation: dict,
) -> None:
    session, user, ks = workspace
    before = workbench.ontology_revision(ks.graph_iri)

    with pytest.raises(HTTPException) as rejected:
        _changes(
            session, user, ks, [operation], expected_revision=before,
            confirm_destructive=True,
        )

    assert rejected.value.status_code == 400
    assert "Axiom not found" in str(rejected.value.detail)
    assert workbench.ontology_revision(ks.graph_iri) == before
    assert session.exec(select(AuditEvent)).all() == []
