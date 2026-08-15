from __future__ import annotations

from fastapi import FastAPI
from fastapi.testclient import TestClient
from sqlalchemy.pool import StaticPool
from sqlmodel import Session, SQLModel, create_engine, select

from app import agent_memory
from app.api import agent as agent_api
from app.db.database import get_session
from app.db.models import AgentConversation, AgentEvent, AgentTurn, Conflict, KnowledgeSystem, User
from app.security import current_user


def _database():
    database = create_engine(
        "sqlite://",
        connect_args={"check_same_thread": False},
        poolclass=StaticPool,
    )
    SQLModel.metadata.create_all(database)
    return database


def _workspace(database):
    with Session(database, expire_on_commit=False) as session:
        owner = User(username="memory-owner", password_hash="unused")
        other = User(username="memory-other", password_hash="unused")
        session.add(owner)
        session.add(other)
        session.commit()
        session.refresh(owner)
        session.refresh(other)
        first = KnowledgeSystem(
            name="Memory one",
            public_id="memory-one",
            owner_id=owner.id,
            graph_iri="urn:memory:one:tbox",
            base_iri="urn:memory:one:",
        )
        second = KnowledgeSystem(
            name="Memory two",
            public_id="memory-two",
            owner_id=owner.id,
            graph_iri="urn:memory:two:tbox",
            base_iri="urn:memory:two:",
        )
        session.add(first)
        session.add(second)
        session.commit()
        session.refresh(first)
        session.refresh(second)
        return owner.id, other.id, first.id, second.id


def test_complete_tool_results_are_persisted_reused_and_safely_serialized() -> None:
    database = _database()
    owner_id, other_id, ks_id, _ = _workspace(database)
    revision = "sha256:current"
    arguments = {"query": "Pump", "limit": 10}
    complete_result = {
        "items": [{"iri": "urn:memory:Pump", "description": "x" * 2_000}],
        "secret_token": "must-not-cross-the-api",
    }

    with Session(database, expire_on_commit=False) as session:
        conversation = agent_memory.create_conversation(
            session,
            user_id=owner_id,
            knowledge_system_id=ks_id,
        )
        user_turn, assistant_turn = agent_memory.start_turn_pair(
            session,
            conversation,
            "What did the Pump search find?",
            knowledge_revision=revision,
            model="test-model",
        )
        agent_memory.append_event(
            session,
            assistant_turn,
            kind="tool_call",
            call_id="search-1",
            tool_name="search_ontology",
            arguments=arguments,
            data={"arguments": arguments},
        )
        result_event = agent_memory.append_event(
            session,
            assistant_turn,
            kind="tool_result",
            call_id="search-1",
            tool_name="search_ontology",
            arguments=arguments,
            data={"result": complete_result, "summary": "Found one class", "ok": True},
        )
        agent_memory.append_event(
            session,
            assistant_turn,
            kind="tool_result",
            call_id="search-failed",
            tool_name="search_ontology",
            arguments=arguments,
            data={"result": {"error": "temporary failure"}, "ok": False},
        )
        agent_memory.finish_turn(
            session,
            assistant_turn,
            content="The Pump class was found.",
            trace=[{"tool": "search_ontology", "summary": "Found one class"}],
        )

        stored = session.get(AgentEvent, result_event.id)
        assert stored.data["result"] == complete_result
        assert stored.result_hash.startswith("sha256:")

        detail = agent_memory.conversation_detail(session, conversation)
        public_result = detail["turns"][1]["events"][1]
        assert public_result["data"]["truncated"] is True
        assert "must-not-cross-the-api" not in public_result["data"]["preview"]
        assert public_result["data"]["summary"] == "Found one class"

        history = agent_memory.load_model_history(
            session,
            conversation_id=conversation.id,
            user_id=owner_id,
            knowledge_system_id=ks_id,
            current_revision=revision,
        )
        assert [message["role"] for message in history] == [
            "user", "assistant", "tool", "assistant",
        ]
        assert "urn:memory:Pump" in history[2]["content"]
        assert "must-not-cross-the-api" not in history[2]["content"]

        stale_history = agent_memory.load_model_history(
            session,
            conversation_id=conversation.id,
            user_id=owner_id,
            knowledge_system_id=ks_id,
            current_revision="sha256:changed",
        )
        assert [message["role"] for message in stale_history] == ["user", "assistant"]

        cached = agent_memory.find_cached_tool_result(
            session,
            conversation_id=conversation.id,
            user_id=owner_id,
            knowledge_system_id=ks_id,
            tool_name="search_ontology",
            arguments=arguments,
            current_revision=revision,
        )
        assert cached is not None and cached.id == result_event.id
        assert agent_memory.find_cached_tool_result(
            session,
            conversation_id=conversation.id,
            user_id=other_id,
            knowledge_system_id=ks_id,
            tool_name="search_ontology",
            arguments=arguments,
            current_revision=revision,
        ) is None
        assert agent_memory.find_cached_tool_result(
            session,
            conversation_id=conversation.id,
            user_id=owner_id,
            knowledge_system_id=ks_id,
            tool_name="search_ontology",
            arguments={"query": "Valve", "limit": 10},
            current_revision=revision,
        ) is None

        assert user_turn.status == "done"
        assert conversation.title == "What did the Pump search find?"


def test_fresh_observations_restore_complete_successes_with_strict_scope() -> None:
    database = _database()
    owner_id, other_id, ks_id, other_ks_id = _workspace(database)
    revision = "sha256:current"

    with Session(database, expire_on_commit=False) as session:
        conversation = agent_memory.create_conversation(
            session,
            user_id=owner_id,
            knowledge_system_id=ks_id,
        )
        _user_turn, assistant_turn = agent_memory.start_turn_pair(
            session,
            conversation,
            "Inspect the review queue",
            knowledge_revision=revision,
        )

        def record(call_id: str, *, knowledge_revision: str, data: dict) -> None:
            arguments = {"queue": "conflicts", "status": "open"}
            agent_memory.append_event(
                session,
                assistant_turn,
                kind="tool_call",
                call_id=call_id,
                tool_name="list_review_items",
                arguments=arguments,
                knowledge_revision=knowledge_revision,
                data={"arguments": arguments},
            )
            agent_memory.append_event(
                session,
                assistant_turn,
                kind="tool_result",
                call_id=call_id,
                tool_name="list_review_items",
                arguments=arguments,
                knowledge_revision=knowledge_revision,
                data=data,
            )

        record(
            "fresh",
            knowledge_revision=revision,
            data={
                "ok": True,
                "result": {
                    "items": [{"id": 7, "title": "Possible duplicate"}],
                    "total": 1,
                    "api_token": "must-stay-private",
                },
            },
        )
        record(
            "stale",
            knowledge_revision="sha256:stale",
            data={"ok": True, "result": {"items": [{"id": 8}], "total": 1}},
        )
        record(
            "failed",
            knowledge_revision=revision,
            data={"ok": False, "error": "temporary failure"},
        )
        agent_memory.finish_turn(session, assistant_turn, content="Found one conflict.")

        observations = agent_memory.load_fresh_observations(
            session,
            conversation_id=conversation.id,
            user_id=owner_id,
            knowledge_system_id=ks_id,
            current_revision=revision,
        )

        assert len(observations) == 1
        assert observations[0]["tool"] == "list_review_items"
        assert observations[0]["arguments"] == {"queue": "conflicts", "status": "open"}
        assert observations[0]["result"]["items"] == [
            {"id": 7, "title": "Possible duplicate"},
        ]
        assert observations[0]["result"]["api_token"] == "[redacted]"
        assert observations[0]["persisted"] is True
        assert isinstance(observations[0]["source_event_id"], int)

        assert agent_memory.load_fresh_observations(
            session,
            conversation_id=conversation.id,
            user_id=other_id,
            knowledge_system_id=ks_id,
            current_revision=revision,
        ) == []
        assert agent_memory.load_fresh_observations(
            session,
            conversation_id=conversation.id,
            user_id=owner_id,
            knowledge_system_id=other_ks_id,
            current_revision=revision,
        ) == []
        assert agent_memory.load_fresh_observations(
            session,
            conversation_id=conversation.id,
            user_id=owner_id,
            knowledge_system_id=ks_id,
            current_revision="sha256:changed",
        ) == []


def test_conversation_crud_api_is_private_and_deletes_children_in_order() -> None:
    database = _database()
    owner_id, other_id, first_ks_id, second_ks_id = _workspace(database)
    app = FastAPI()
    app.include_router(agent_api.router)

    with Session(database, expire_on_commit=False) as session:
        owner = session.get(User, owner_id)
        other = session.get(User, other_id)
        first_ks = session.get(KnowledgeSystem, first_ks_id)
        second_ks = session.get(KnowledgeSystem, second_ks_id)
        identity = {"user": owner, "ks": first_ks}
        app.dependency_overrides[get_session] = lambda: session
        app.dependency_overrides[current_user] = lambda: identity["user"]
        app.dependency_overrides[agent_api.ks_reader] = lambda: identity["ks"]

        with TestClient(app) as client:
            created = client.post(
                f"/api/knowledge/{first_ks_id}/agent/conversations",
                json={"title": "Pump review"},
            )
            assert created.status_code == 201
            conversation_id = created.json()["id"]

            conversation = session.get(AgentConversation, conversation_id)
            _, assistant = agent_memory.start_turn_pair(session, conversation, "Inspect Pump")
            source_event = agent_memory.append_event(
                session,
                assistant,
                kind="tool_result",
                call_id="tool-1",
                tool_name="search_ontology",
                arguments={"query": "Pump"},
                data={"result": {"items": ["Pump"]}},
            )
            agent_memory.append_event(
                session,
                assistant,
                kind="tool_result",
                call_id="tool-2",
                tool_name="search_ontology",
                arguments={"query": "Pump"},
                data={"result": {"items": ["Pump"]}},
                cached_from_event_id=source_event.id,
            )
            agent_memory.finish_turn(session, assistant, content="Pump exists.")

            listed = client.get(f"/api/knowledge/{first_ks_id}/agent/conversations")
            assert listed.status_code == 200
            assert listed.json()["conversations"][0]["first_user_message"] == "Inspect Pump"
            assert listed.json()["conversations"][0]["turn_count"] == 2

            detail = client.get(
                f"/api/knowledge/{first_ks_id}/agent/conversations/{conversation_id}"
            )
            assert detail.status_code == 200
            assert [turn["role"] for turn in detail.json()["turns"]] == ["user", "assistant"]

            renamed = client.patch(
                f"/api/knowledge/{first_ks_id}/agent/conversations/{conversation_id}",
                json={"title": "Renamed review"},
            )
            assert renamed.status_code == 200
            assert renamed.json()["title"] == "Renamed review"

            identity["user"] = other
            assert client.get(
                f"/api/knowledge/{first_ks_id}/agent/conversations/{conversation_id}"
            ).status_code == 404
            identity["user"] = owner
            identity["ks"] = second_ks
            assert client.get(
                f"/api/knowledge/{second_ks_id}/agent/conversations/{conversation_id}"
            ).status_code == 404
            identity["ks"] = first_ks

            deleted = client.delete(
                f"/api/knowledge/{first_ks_id}/agent/conversations/{conversation_id}"
            )
            assert deleted.status_code == 204
            assert deleted.content == b""

        assert session.get(AgentConversation, conversation_id) is None
        assert session.exec(select(AgentTurn)).all() == []
        assert session.exec(select(AgentEvent)).all() == []


def test_scoped_cleanup_never_deletes_another_user_or_knowledge_system() -> None:
    database = _database()
    owner_id, other_id, first_ks_id, second_ks_id = _workspace(database)
    with Session(database) as session:
        keep_other_user = agent_memory.create_conversation(
            session, user_id=other_id, knowledge_system_id=first_ks_id, title="Other user",
        )
        remove = agent_memory.create_conversation(
            session, user_id=owner_id, knowledge_system_id=first_ks_id, title="Remove",
        )
        keep_other_ks = agent_memory.create_conversation(
            session, user_id=owner_id, knowledge_system_id=second_ks_id, title="Other KS",
        )

        assert agent_memory.delete_scoped_conversations(
            session,
            user_id=owner_id,
            knowledge_system_id=first_ks_id,
            commit=True,
        ) == 1
        assert session.get(AgentConversation, remove.id) is None
        assert session.get(AgentConversation, keep_other_user.id) is not None
        assert session.get(AgentConversation, keep_other_ks.id) is not None


def test_evidence_revision_includes_queue_only_changes() -> None:
    database = _database()
    _, _, ks_id, _ = _workspace(database)

    with Session(database, expire_on_commit=False) as session:
        ks = session.get(KnowledgeSystem, ks_id)
        before = agent_memory.current_evidence_revision(session, ks)
        conflict = Conflict(
            knowledge_system_id=ks_id,
            signature="queue-only",
            ctype="duplicate",
            status="open",
            title="Possible duplicate",
        )
        session.add(conflict)
        session.commit()
        open_revision = agent_memory.current_evidence_revision(session, ks)
        conflict.status = "dismissed"
        session.add(conflict)
        session.commit()
        dismissed_revision = agent_memory.current_evidence_revision(session, ks)

    assert before != open_revision
    assert open_revision != dismissed_revision
