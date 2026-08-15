from __future__ import annotations

import asyncio
import copy
import json
from contextlib import nullcontext

import pytest
from pyoxigraph import Store
from sqlalchemy.pool import StaticPool
from sqlmodel import Session, SQLModel, create_engine

from app import agent_runtime, mcp_server
from app.api import agent as agent_api
from app.db.models import (
    AxiomProvenance,
    Chunk,
    Conflict,
    Document,
    EntityResolution,
    KnowledgeSystem,
    TermProposal,
    User,
)
from app.ontology import editor, schema, store, workbench


def _database():
    database = create_engine(
        "sqlite://",
        connect_args={"check_same_thread": False},
        poolclass=StaticPool,
    )
    SQLModel.metadata.create_all(database)
    return database


def _workspace(database):
    with Session(database) as session:
        owner = User(username="agent-owner", password_hash="unused")
        session.add(owner)
        session.commit()
        session.refresh(owner)
        ks = KnowledgeSystem(
            name="Factory ontology",
            public_id="factory",
            owner_id=owner.id,
            graph_iri="urn:factory:tbox",
            base_iri="urn:factory:onto:",
        )
        session.add(ks)
        session.commit()
        session.refresh(ks)
        return owner.id, ks.id, ks.graph_iri, ks.base_iri


def test_internal_mcp_delegation_is_read_only_and_live_authorized(monkeypatch) -> None:
    database = _database()
    monkeypatch.setattr(mcp_server, "engine", database)
    monkeypatch.setattr(store, "_store", Store())
    store._graph_locks.clear()
    store._recorders.clear()
    user_id, ks_id, graph_iri, base_iri = _workspace(database)
    editor.apply_edit(graph_iri, base_iri, {"op": "add_class", "label": "Pump"})
    pump = schema.build_view(graph_iri)["classes"][0]

    result = asyncio.run(mcp_server.call_internal_read_tool(
        "get_ontology_neighborhood",
        {"iri": pump["iri"]},
        user_id=user_id,
        knowledge_system_id=ks_id,
    ))
    assert result["kind"] == "class"
    assert result["label"] == "Pump"

    with pytest.raises(mcp_server.ToolError, match="read-only delegation"):
        asyncio.run(mcp_server.call_internal_read_tool(
            "apply_ontology_changes",
            {
                "operations": [{"op": "add_class", "label": "Valve"}],
                "reason": "must not run",
                "expected_revision": workbench.ontology_revision(graph_iri),
            },
            user_id=user_id,
            knowledge_system_id=ks_id,
        ))
    assert [item["label"] for item in schema.build_view(graph_iri)["classes"]] == ["Pump"]

    with Session(database) as session:
        user = session.get(User, user_id)
        user.active = False
        session.add(user)
        session.commit()
    with pytest.raises(mcp_server.ToolError, match="no longer available"):
        asyncio.run(mcp_server.call_internal_read_tool(
            "get_workspace_context",
            {},
            user_id=user_id,
            knowledge_system_id=ks_id,
        ))


def test_model_repeat_of_fresh_tool_read_is_satisfied_silently_from_memory(
    monkeypatch,
) -> None:
    database = _database()
    user_id, ks_id, _graph_iri, _base_iri = _workspace(database)
    provider_calls = 0
    events: list[dict] = []

    async def fake_chat_message(messages, **_kwargs):
        nonlocal provider_calls
        provider_calls += 1
        if provider_calls == 1:
            return {
                "role": "assistant",
                "content": None,
                "tool_calls": [{
                    "id": "repeat-search",
                    "type": "function",
                    "function": {
                        "name": "search_ontology",
                        "arguments": '{"query":"Pump","limit":5}',
                    },
                }],
            }
        tool_message = messages[-1]
        assert tool_message["role"] == "tool"
        assert json.loads(tool_message["content"])["items"][0]["iri"] == "urn:factory:Pump"
        return {
            "role": "assistant",
            "content": json.dumps({
                "answer": "Pump 的 IRI 是 `urn:factory:Pump`。",
                "suggestion": None,
            }, ensure_ascii=False),
        }

    async def forbidden_mcp_call(*_args, **_kwargs):
        raise AssertionError("fresh persisted evidence must not call MCP again")

    async def fake_tool_specs():
        return []

    async def sink(event):
        events.append(event)

    monkeypatch.setattr(agent_runtime.openrouter, "chat_message", fake_chat_message)
    monkeypatch.setattr(agent_runtime, "_mcp_call", forbidden_mcp_call)
    monkeypatch.setattr(agent_runtime, "_tool_specs", fake_tool_specs)

    with Session(database) as session:
        result = asyncio.run(agent_runtime.run(
            session=session,
            user=session.get(User, user_id),
            ks=session.get(KnowledgeSystem, ks_id),
            conversation=[{"role": "user", "content": "它的 IRI 呢？"}],
            event_sink=sink,
            context_observations=[{
                "tool": "search_ontology",
                "arguments": {"query": "Pump", "limit": 5},
                "result": {"items": [{"iri": "urn:factory:Pump"}], "total": 1},
                "persisted": True,
                "source_event_id": 42,
            }],
        ))

    assert provider_calls == 2
    assert result["answer"] == "Pump 的 IRI 是 `urn:factory:Pump`。"
    assert result["trace"] == []
    assert events == []


def test_review_followup_uses_persisted_observations_without_reset_or_new_reads(
    monkeypatch,
) -> None:
    database = _database()
    user_id, ks_id, _graph_iri, _base_iri = _workspace(database)
    events: list[dict] = []
    answer = (
        "冲突 #1（Pump / Pumping Unit）需要你在已登记方案中选择合并方向；"
        "来源证据已读取，但尚不足以替你决定保留哪一个类。"
    )

    async def fake_provider_stream(*_args, **_kwargs):
        payload = json.dumps({"answer": answer, "suggestion": None}, ensure_ascii=False)
        for part in (payload[:24], payload[24:57], payload[57:]):
            yield {"type": "content_delta", "delta": part}
        yield {
            "type": "message",
            "message": {"role": "assistant", "content": payload},
        }

    async def forbidden_mcp_call(*_args, **_kwargs):
        raise AssertionError("revision-fresh review observations must not be read again")

    async def fake_tool_specs():
        return []

    async def sink(event):
        events.append(event)

    monkeypatch.setattr(agent_runtime.openrouter, "chat_message_stream", fake_provider_stream)
    monkeypatch.setattr(agent_runtime, "_mcp_call", forbidden_mcp_call)
    monkeypatch.setattr(agent_runtime, "_tool_specs", fake_tool_specs)
    observations = [
        {
            "tool": "get_workspace_context",
            "arguments": {},
            "result": {
                "review_counts": {
                    "open_conflicts": 1,
                    "pending_entity_resolution": 0,
                    "pending_terminology": 1,
                    "validation_violations": 0,
                },
            },
            "persisted": True,
        },
        {
            "tool": "list_review_items",
            "arguments": {
                "queue": "conflicts",
                "status": "open",
                "limit": 50,
                "offset": 0,
            },
            "result": {
                "items": [{
                    "id": 1,
                    "title": "Possible duplicate classes",
                    "payload": {
                        "entities": [{"label": "Pump"}, {"label": "Pumping Unit"}],
                        "resolutions": [],
                    },
                }],
                "total": 1,
            },
            "persisted": True,
        },
        {
            "tool": "get_conflicts_context",
            "arguments": {"conflict_ids": [1]},
            "result": {
                "items": [{
                    "conflict": {"id": 1},
                    "evidence": [{"text": "Both labels occur in the source."}],
                }],
                "total": 1,
            },
            "persisted": True,
        },
        {
            "tool": "list_review_items",
            "arguments": {
                "queue": "terminology",
                "status": "pending",
                "limit": 50,
                "offset": 0,
            },
            "result": {"items": [{"id": 9, "term": "old term"}], "total": 1},
            "persisted": True,
        },
    ]

    with Session(database) as session:
        result = asyncio.run(agent_runtime.run(
            session=session,
            user=session.get(User, user_id),
            ks=session.get(KnowledgeSystem, ks_id),
            conversation=[{"role": "user", "content": "只看这些冲突，应该如何审批？"}],
            event_sink=sink,
            native_answer_stream=True,
            context_observations=observations,
        ))

    assert result["answer"] == answer
    assert result["trace"] == []
    assert "answer_reset" not in [event["type"] for event in events]
    assert not any(event["type"] in {"progress", "trace"} for event in events)
    assert "".join(event["delta"] for event in events if event["type"] == "delta") == answer


def test_agent_explores_with_mcp_and_previews_without_writing(monkeypatch) -> None:
    database = _database()
    monkeypatch.setattr(mcp_server, "engine", database)
    monkeypatch.setattr(store, "_store", Store())
    store._graph_locks.clear()
    store._recorders.clear()
    user_id, ks_id, graph_iri, base_iri = _workspace(database)
    editor.apply_edit(graph_iri, base_iri, {"op": "add_class", "label": "Pump"})
    pump = schema.build_view(graph_iri)["classes"][0]

    replies = iter([
        {
            "role": "assistant",
            "content": None,
            "tool_calls": [{
                "id": "search-1",
                "type": "function",
                "function": {
                    "name": "search_ontology",
                    "arguments": json.dumps({"query": "Pump"}),
                },
            }],
        },
        {
            "role": "assistant",
            "content": json.dumps({
                "answer": "Pump is currently a root class. Adding a broader Equipment class is a reviewable improvement.",
                "suggestion": {
                    "summary": "Place Pump under Equipment",
                    "reason": "The inspected Pump has no superclass.",
                    "operations": [{"op": "add_class", "label": "Equipment"}],
                },
            }),
        },
    ])

    calls = []

    async def fake_chat_message(messages, **_kwargs):
        calls.append(messages)
        return next(replies)

    mcp_calls = []
    original_mcp_call = agent_runtime._mcp_call

    async def recording_mcp_call(name, arguments, *, user, ks):
        mcp_calls.append((name, copy.deepcopy(arguments)))
        return await original_mcp_call(name, arguments, user=user, ks=ks)

    monkeypatch.setattr(agent_runtime.openrouter, "chat_message", fake_chat_message)
    monkeypatch.setattr(agent_runtime, "_mcp_call", recording_mcp_call)
    with Session(database) as session:
        user = session.get(User, user_id)
        ks = session.get(KnowledgeSystem, ks_id)
        result = asyncio.run(agent_runtime.run(
            session=session,
            user=user,
            ks=ks,
            conversation=[{"role": "user", "content": "How should I improve the Pump class?"}],
        ))

    tools = [step["tool"] for step in result["trace"]]
    assert tools[0] == "search_ontology"
    assert "get_workspace_context" not in tools
    assert tools[-1] == "preview_ontology_changes"
    assert not any(
        "CURRENT UI CONTEXT" in str(message.get("content") or "")
        for message in calls[0]
    )
    assert result["proposal"]["preview"]["dry_run"] is True
    assert result["proposal"]["preview"]["diff"]["counts"]["tbox_added"] > 0
    preview_arguments = next(arguments for name, arguments in mcp_calls if name == "preview_ontology_changes")
    assert preview_arguments["include_rdf_diff"] is False
    assert result["proposal"]["preview"]["diff"]["tbox_added"] == ""
    assert [item["label"] for item in schema.build_view(graph_iri)["classes"]] == ["Pump"]


def test_agent_can_answer_without_a_mandatory_bootstrap_tool(monkeypatch) -> None:
    database = _database()
    monkeypatch.setattr(mcp_server, "engine", database)
    monkeypatch.setattr(store, "_store", Store())
    store._graph_locks.clear()
    store._recorders.clear()
    user_id, ks_id, _graph_iri, _base_iri = _workspace(database)
    calls = []

    async def fake_chat_message(messages, **kwargs):
        calls.append((copy.deepcopy(messages), kwargs))
        return {
            "role": "assistant",
            "content": json.dumps({
                "answer": "An ontology defines concepts and the relationships between them.",
                "suggestion": None,
            }),
        }

    monkeypatch.setattr(agent_runtime.openrouter, "chat_message", fake_chat_message)
    with Session(database) as session:
        result = asyncio.run(agent_runtime.run(
            session=session,
            user=session.get(User, user_id),
            ks=session.get(KnowledgeSystem, ks_id),
            conversation=[{"role": "user", "content": "What is an ontology?"}],
        ))

    assert result["trace"] == []
    assert len(calls) == 1
    assert calls[0][1]["tool_choice"] == "auto"
    assert "no mandatory bootstrap action" in calls[0][0][0]["content"]


def test_agent_repairs_an_invalid_proposal_schema(monkeypatch) -> None:
    database = _database()
    monkeypatch.setattr(mcp_server, "engine", database)
    monkeypatch.setattr(store, "_store", Store())
    store._graph_locks.clear()
    store._recorders.clear()
    user_id, ks_id, graph_iri, base_iri = _workspace(database)
    editor.apply_edit(graph_iri, base_iri, {"op": "add_class", "label": "Pump"})
    pump = schema.build_view(graph_iri)["classes"][0]

    replies = iter([
        {
            "role": "assistant",
            "content": json.dumps({
                "answer": "A serial number would identify the pump.",
                "suggestion": {
                    "summary": "Add serial number",
                    "reason": "The class has no identifier data property.",
                    "operations": [{"op": "add_data_property", "label": "serial number"}],
                },
            }),
        },
        {
            "role": "assistant",
            "content": json.dumps({
                "answer": "A serial number would identify the pump.",
                "suggestion": {
                    "summary": "Add serial number",
                    "reason": "The class has no identifier data property.",
                    "operations": [{
                        "op": "add_property",
                        "kind": "data",
                        "label": "serial number",
                        "domain": pump["iri"],
                        "range": "string",
                    }],
                },
            }),
        },
    ])
    calls = []

    async def fake_chat_message(messages, **_kwargs):
        calls.append(messages)
        return next(replies)

    monkeypatch.setattr(agent_runtime.openrouter, "chat_message", fake_chat_message)
    with Session(database) as session:
        result = asyncio.run(agent_runtime.run(
            session=session,
            user=session.get(User, user_id),
            ks=session.get(KnowledgeSystem, ks_id),
            conversation=[{"role": "user", "content": "Add a serial number to the Pump class"}],
        ))

    assert len(calls) == 2
    assert "rejected the proposal schema" in calls[1][-1]["content"]
    assert result["proposal"]["operations"][0]["op"] == "add_property"
    assert result["trace"][-1]["tool"] == "preview_ontology_changes"
    assert [item["label"] for item in schema.build_view(graph_iri)["classes"]] == ["Pump"]


def test_agent_returns_a_tool_result_for_every_requested_call(monkeypatch) -> None:
    database = _database()
    monkeypatch.setattr(mcp_server, "engine", database)
    monkeypatch.setattr(store, "_store", Store())
    store._graph_locks.clear()
    store._recorders.clear()
    user_id, ks_id, graph_iri, _base_iri = _workspace(database)
    requested = [{
        "id": f"tool-{index}",
        "type": "function",
        "function": {"name": "get_workspace_context", "arguments": "{}"},
    } for index in range(5)]
    replies = iter([
        {"role": "assistant", "content": None, "tool_calls": requested},
        {"role": "assistant", "content": json.dumps({"answer": "Done", "suggestion": None})},
    ])
    calls = []

    async def fake_chat_message(messages, **_kwargs):
        calls.append(messages)
        return next(replies)

    monkeypatch.setattr(agent_runtime.openrouter, "chat_message", fake_chat_message)
    with Session(database) as session:
        result = asyncio.run(agent_runtime.run(
            session=session,
            user=session.get(User, user_id),
            ks=session.get(KnowledgeSystem, ks_id),
            conversation=[{"role": "user", "content": "Inspect the workspace"}],
        ))

    tool_results = [message for message in calls[1] if message.get("role") == "tool"]
    assert {message["tool_call_id"] for message in tool_results}.issuperset(
        {call["id"] for call in requested},
    )
    assert not any(step["summary"].startswith("Failed:") for step in result["trace"])


def test_agent_does_not_stop_productive_tool_use_at_legacy_budget(monkeypatch) -> None:
    database = _database()
    monkeypatch.setattr(mcp_server, "engine", database)
    monkeypatch.setattr(store, "_store", Store())
    store._graph_locks.clear()
    store._recorders.clear()
    user_id, ks_id, _graph_iri, _base_iri = _workspace(database)
    monkeypatch.setattr(agent_runtime.settings, "agentic_max_steps", 2)
    replies = iter([
        {
            "role": "assistant",
            "content": None,
            "tool_calls": [{
                "id": "tool-1",
                "type": "function",
                "function": {"name": "get_workspace_context", "arguments": "{}"},
            }],
        },
        {
            "role": "assistant",
            "content": None,
            "tool_calls": [{
                "id": "tool-2",
                "type": "function",
                "function": {"name": "get_ontology", "arguments": "{}"},
            }],
        },
        {
            "role": "assistant",
            "content": None,
            "tool_calls": [{
                "id": "tool-3",
                "type": "function",
                "function": {
                    "name": "search_ontology",
                    "arguments": '{"query":"Pump","limit":5}',
                },
            }],
        },
        {
            "role": "assistant",
            "content": json.dumps({
                "answer": "The collected evidence supports a final answer.",
                "suggestion": None,
            }),
        },
    ])
    tool_options = []

    async def fake_chat_message(_messages, **kwargs):
        tool_options.append(kwargs.get("tools"))
        return next(replies)

    monkeypatch.setattr(agent_runtime.openrouter, "chat_message", fake_chat_message)
    with Session(database) as session:
        result = asyncio.run(agent_runtime.run(
            session=session,
            user=session.get(User, user_id),
            ks=session.get(KnowledgeSystem, ks_id),
            conversation=[{"role": "user", "content": "Inspect the ontology thoroughly"}],
        ))

    assert result["answer"] == "The collected evidence supports a final answer."
    assert all(options for options in tool_options)
    assert len(result["trace"]) == 3


def test_agent_plans_conflict_rows_instead_of_answering_from_count(monkeypatch) -> None:
    database = _database()
    monkeypatch.setattr(mcp_server, "engine", database)
    monkeypatch.setattr(store, "_store", Store())
    store._graph_locks.clear()
    store._recorders.clear()
    user_id, ks_id, _graph_iri, _base_iri = _workspace(database)
    with Session(database) as session:
        session.add(Conflict(
            knowledge_system_id=ks_id,
            signature="duplicate|pump|pumping-unit",
            ctype="duplicate",
            severity="warning",
            title="Pump and Pumping Unit may be duplicates",
            detail="The labels are similar.",
            payload={"entities": [], "resolutions": []},
        ))
        session.commit()

    replies = iter([
        {
            "role": "assistant",
            "content": None,
            "tool_calls": [{
                "id": "list-conflicts",
                "type": "function",
                "function": {
                    "name": "list_review_items",
                    "arguments": json.dumps({"queue": "conflicts", "status": "open"}),
                },
            }],
        },
        {
            "role": "assistant",
            "content": json.dumps({
                "answer": "当前待处理项是“Pump 和 Pumping Unit 可能重复”。",
                "suggestion": None,
            }),
        },
    ])
    calls = []

    async def fake_chat_message(messages, **_kwargs):
        calls.append(messages)
        return next(replies)

    monkeypatch.setattr(agent_runtime.openrouter, "chat_message", fake_chat_message)
    with Session(database) as session:
        result = asyncio.run(agent_runtime.run(
            session=session,
            user=session.get(User, user_id),
            ks=session.get(KnowledgeSystem, ks_id),
            conversation=[{"role": "user", "content": "有哪些待处理冲突？"}],
        ))

    assert "Pump 和 Pumping Unit" in result["answer"]
    assert [step["tool"] for step in result["trace"]][-1] == "list_review_items"
    observed_tools = [
        message.get("name")
        for call in calls
        for message in call
        if message.get("role") == "tool"
    ]
    assert "list_review_items" in observed_tools


def test_agent_reads_each_listed_conflict_before_advising(monkeypatch) -> None:
    database = _database()
    monkeypatch.setattr(mcp_server, "engine", database)
    monkeypatch.setattr(store, "_store", Store())
    store._graph_locks.clear()
    store._recorders.clear()
    user_id, ks_id, _graph_iri, _base_iri = _workspace(database)
    with Session(database) as session:
        conflict = Conflict(
            knowledge_system_id=ks_id,
            signature="duplicate|pump|pumping-unit",
            ctype="duplicate",
            severity="warning",
            title="Pump and Pumping Unit may be duplicates",
            detail="The labels are similar.",
            payload={
                "entities": [
                    {"iri": "urn:factory:onto:Pump", "label": "Pump"},
                    {"iri": "urn:factory:onto:PumpingUnit", "label": "Pumping Unit"},
                ],
                "resolutions": [{
                    "id": "merge-pumping-unit-into-pump",
                    "label": "Merge Pumping Unit into Pump",
                    "op": {
                        "op": "merge_classes",
                        "source": "urn:factory:onto:PumpingUnit",
                        "target": "urn:factory:onto:Pump",
                    },
                }],
            },
        )
        session.add(conflict)
        session.commit()
        session.refresh(conflict)
        conflict_id = conflict.id

    replies = iter([
        {
            "role": "assistant",
            "content": None,
            "tool_calls": [{
                "id": "list-conflicts",
                "type": "function",
                "function": {
                    "name": "list_review_items",
                    "arguments": json.dumps({"queue": "conflicts", "status": "open"}),
                },
            }],
        },
        {
            "role": "assistant",
            "content": None,
            "tool_calls": [{
                "id": "read-conflict-context",
                "type": "function",
                "function": {
                    "name": "get_conflicts_context",
                    "arguments": json.dumps({"conflict_ids": [conflict_id]}),
                },
            }],
        },
        {
            "role": "assistant",
            "content": json.dumps({
                "answer": (
                    "这是 Pump 和 Pumping Unit 之间的疑似重复。已登记的候选方案是将 "
                    "Pumping Unit 合并到 Pump，但当前没有来源证据，因此选择前应确认二者可互换。"
                ),
                "suggestion": None,
            }),
        },
    ])
    calls = []

    async def fake_chat_message(messages, **_kwargs):
        calls.append(messages)
        return next(replies)

    monkeypatch.setattr(agent_runtime.openrouter, "chat_message", fake_chat_message)
    with Session(database) as session:
        result = asyncio.run(agent_runtime.run(
            session=session,
            user=session.get(User, user_id),
            ks=session.get(KnowledgeSystem, ks_id),
            conversation=[
                {"role": "assistant", "content": "There is one open conflict."},
                {"role": "user", "content": "该怎么处理？"},
            ],
        ))

    tools = [step["tool"] for step in result["trace"]]
    assert tools[-2:] == ["list_review_items", "get_conflicts_context"]
    assert "Pumping Unit 合并到 Pump" in result["answer"]
    final_call_tools = [message.get("name") for message in calls[-1] if message.get("role") == "tool"]
    assert final_call_tools[-2:] == ["list_review_items", "get_conflicts_context"]


def test_agent_does_not_propose_conflict_edits_without_source_evidence(monkeypatch) -> None:
    database = _database()
    monkeypatch.setattr(mcp_server, "engine", database)
    monkeypatch.setattr(store, "_store", Store())
    store._graph_locks.clear()
    store._recorders.clear()
    user_id, ks_id, graph_iri, base_iri = _workspace(database)
    editor.apply_edit(graph_iri, base_iri, {"op": "add_class", "label": "Pump"})
    editor.apply_edit(graph_iri, base_iri, {"op": "add_class", "label": "Pumping Unit"})
    classes = {item["label"]: item["iri"] for item in schema.build_view(graph_iri)["classes"]}
    with Session(database) as session:
        session.add(Conflict(
            knowledge_system_id=ks_id,
            signature="duplicate|pump|pumping-unit",
            ctype="duplicate",
            severity="warning",
            title="Pump and Pumping Unit may be duplicates",
            detail="The labels are similar.",
            payload={
                "entities": [
                    {"iri": classes["Pump"], "label": "Pump"},
                    {"iri": classes["Pumping Unit"], "label": "Pumping Unit"},
                ],
                "resolutions": [{
                    "id": "merge-pumping-unit-into-pump",
                    "label": "Merge Pumping Unit into Pump",
                    "op": {
                        "op": "merge_classes",
                        "source": classes["Pumping Unit"],
                        "target": classes["Pump"],
                    },
                }],
            },
        ))
        session.commit()

    replies = iter([
        {
            "role": "assistant",
            "content": None,
            "tool_calls": [{
                "id": "list-conflicts",
                "type": "function",
                "function": {
                    "name": "list_review_items",
                    "arguments": json.dumps({"queue": "conflicts", "status": "open"}),
                },
            }],
        },
        {
            "role": "assistant",
            "content": None,
            "tool_calls": [{
                "id": "read-conflict-context",
                "type": "function",
                "function": {
                    "name": "get_conflicts_context",
                    "arguments": json.dumps({"conflict_ids": [1]}),
                },
            }],
        },
        {
            "role": "assistant",
            "content": json.dumps({
                "answer": "应将 Pumping Unit 合并到 Pump。",
                "suggestion": {
                    "summary": "合并重复类",
                    "reason": "已登记候选合并方案。",
                    "operations": [{
                        "op": "merge_classes",
                        "source": classes["Pumping Unit"],
                        "target": classes["Pump"],
                    }],
                },
            }),
        },
        {
            "role": "assistant",
            "content": json.dumps({
                "answer": (
                    "登记的候选路径是将 Pumping Unit 合并到 Pump，但当前没有来源证据。"
                    "需要先核实二者在业务语义上可互换，而不是上下位类或关联概念。"
                ),
                "suggestion": None,
            }),
        },
    ])
    calls = []

    async def fake_chat_message(messages, **_kwargs):
        calls.append(messages)
        return next(replies)

    monkeypatch.setattr(agent_runtime.openrouter, "chat_message", fake_chat_message)
    with Session(database) as session:
        result = asyncio.run(agent_runtime.run(
            session=session,
            user=session.get(User, user_id),
            ks=session.get(KnowledgeSystem, ks_id),
            conversation=[{"role": "user", "content": "这个冲突应该怎么处理？"}],
        ))

    assert result["proposal"] is None
    assert "没有来源证据" in result["answer"]
    assert any(
        "Runtime provenance check" in str(message.get("content") or "")
        for call in calls
        for message in call
        if message.get("role") == "user"
    )


def test_agent_carries_review_intent_from_analysis_to_advice_and_dry_run(monkeypatch) -> None:
    """A short follow-up must act on reviewed rows, not fall back to queue counts."""

    database = _database()
    monkeypatch.setattr(mcp_server, "engine", database)
    monkeypatch.setattr(store, "_store", Store())
    store._graph_locks.clear()
    store._recorders.clear()
    user_id, ks_id, graph_iri, base_iri = _workspace(database)
    editor.apply_edit(graph_iri, base_iri, {"op": "add_class", "label": "Pump"})
    editor.apply_edit(graph_iri, base_iri, {"op": "add_class", "label": "Pumping Unit"})
    classes = {item["label"]: item["iri"] for item in schema.build_view(graph_iri)["classes"]}

    with Session(database) as session:
        document = Document(
            knowledge_system_id=ks_id,
            sha256="review-flow-source",
            original_filename="pump-standard.txt",
            ext="txt",
            size_bytes=76,
            storage_path="review-flow-source",
            parse_status="parsed",
        )
        session.add(document)
        session.commit()
        session.refresh(document)
        chunk = Chunk(
            document_id=document.id,
            idx=0,
            text="The source standard uses Pumping Unit as an alias of Pump.",
            char_end=58,
        )
        session.add(chunk)
        session.commit()
        session.refresh(chunk)

        conflict = Conflict(
            knowledge_system_id=ks_id,
            signature="duplicate|pump|pumping-unit|review-flow",
            ctype="duplicate",
            severity="warning",
            title="Pump and Pumping Unit may be duplicates",
            detail="The source uses both labels for the same equipment type.",
            payload={
                "entities": [
                    {"iri": classes["Pump"], "label": "Pump"},
                    {"iri": classes["Pumping Unit"], "label": "Pumping Unit"},
                ],
                "resolutions": [{
                    "id": "merge-pumping-unit-into-pump",
                    "label": "Merge Pumping Unit into Pump",
                    "op": {
                        "op": "merge_classes",
                        "source": classes["Pumping Unit"],
                        "target": classes["Pump"],
                    },
                }],
            },
        )
        entity_resolution = EntityResolution(
            knowledge_system_id=ks_id,
            surface_form="海探1",
            class_iri=classes["Pump"],
            confidence=0.61,
            context={
                "candidates": [{
                    "iri": "urn:factory:onto:OceanProbeOne",
                    "label": "海洋探测器一号",
                    "score": 0.61,
                }],
                "evidence": "海探1完成本次巡检。",
            },
        )
        terminology = TermProposal(
            knowledge_system_id=ks_id,
            signature="alias|海探1|ocean-probe-one",
            action="add_alias",
            term="海探1",
            target_iri="urn:factory:onto:OceanProbeOne",
            confidence=0.93,
            reason="The source repeatedly abbreviates the registered name.",
            evidence=[{"text": "海探1（海洋探测器一号）完成巡检。"}],
        )
        session.add(conflict)
        session.add(entity_resolution)
        session.add(terminology)
        session.commit()
        session.refresh(conflict)
        session.refresh(entity_resolution)
        session.refresh(terminology)
        conflict_id = conflict.id
        entity_resolution_id = entity_resolution.id
        terminology_id = terminology.id
        session.add(AxiomProvenance(
            knowledge_system_id=ks_id,
            axiom_key="class|Pump",
            chunk_id=chunk.id,
            method="extraction",
            actor_name="extractor",
        ))
        session.add(AxiomProvenance(
            knowledge_system_id=ks_id,
            axiom_key="class|PumpingUnit",
            chunk_id=chunk.id,
            method="extraction",
            actor_name="extractor",
        ))
        session.commit()

    validation_id = "validation:serial-number:not-a-number"
    validation_item = {
        "id": validation_id,
        "kind": "datatype",
        "property_iri": "urn:factory:onto:serialNumber",
        "property_label": "serial number",
        "value": "N/A",
        "severity": "warning",
        "message": "serial number expects an integer",
    }
    monkeypatch.setattr(
        mcp_server.abox_validate,
        "validate",
        lambda *_args, **_kwargs: {
            "violations": [validation_item],
            "counts": {"error": 0, "warning": 1},
        },
    )

    prompts = ("帮我分析待审批项目", "该如何审批", "帮我执行")
    answers = {
        prompts[0]: (
            "发现四类待审批项目：Pump 与 Pumping Unit 重复冲突、海探1实体消歧、"
            "海探1术语别名，以及 serial number 数据类型违规。"
        ),
        prompts[1]: (
            f"冲突 #{conflict_id}：来源证据支持采用 `Merge Pumping Unit into Pump`；"
            "实体消歧仍需选择 match/new，"
            "术语需选择 accept/reject，数据违规需确认具体 fix。"
        ),
        prompts[2]: (
            "已把 1 项有充分来源证据的 Pump 重复类合并整理成 dry-run 预览，尚未写入；"
            "海探1 实体消歧"
            "仍需选择 match/new，海探1 术语需选择 accept/reject，serial number 违规需"
            "确认具体 fix，因此没有声称这些项目已经执行。"
        ),
    }
    model_calls: list[tuple[str, list[dict]]] = []

    async def fake_chat_message(messages, **_kwargs):
        active_prompt = next(
            prompt
            for prompt in reversed(prompts)
            if any(
                message.get("role") == "user" and message.get("content") == prompt
                for message in messages
            )
        )
        model_calls.append((active_prompt, copy.deepcopy(messages)))
        chosen_calls = [
            function
            for message in messages
            for call in message.get("tool_calls", [])
            if isinstance(call, dict)
            for function in [call.get("function") or {}]
        ]
        chosen_tools = [str(call.get("name") or "") for call in chosen_calls]

        def tool_call(name: str, arguments: dict, index: int = 1) -> dict:
            return {
                "id": f"{name}-{index}",
                "type": "function",
                "function": {"name": name, "arguments": json.dumps(arguments)},
            }

        # The mocked model, not the runtime, selects the observations it needs for this intent.
        if "get_workspace_context" not in chosen_tools:
            return {
                "role": "assistant",
                "content": None,
                "tool_calls": [tool_call("get_workspace_context", {})],
            }

        queue_specs = (
            ("conflicts", "open"),
            ("entity_resolution", "pending"),
            ("terminology", "pending"),
            ("validation", "all"),
        )
        inspected_queues = {
            (json.loads(str(call.get("arguments") or "{}")).get("queue"),
             json.loads(str(call.get("arguments") or "{}")).get("status"))
            for call in chosen_calls
            if call.get("name") == "list_review_items"
        }
        missing_queues = [item for item in queue_specs if item not in inspected_queues]
        if missing_queues:
            return {
                "role": "assistant",
                "content": None,
                "tool_calls": [
                    tool_call(
                        "list_review_items",
                        {"queue": queue, "status": status, "limit": 50, "offset": 0},
                        index,
                    )
                    for index, (queue, status) in enumerate(missing_queues, start=1)
                ],
            }
        if active_prompt != prompts[0] and "get_conflicts_context" not in chosen_tools:
            return {
                "role": "assistant",
                "content": None,
                "tool_calls": [tool_call(
                    "get_conflicts_context",
                    {"conflict_ids": [conflict_id]},
                )],
            }

        suggestion = None
        if active_prompt == prompts[2]:
            suggestion = {
                "summary": "Merge the evidenced duplicate pump classes",
                "reason": "The inspected source explicitly uses Pumping Unit as an alias of Pump.",
                "operations": [{
                    "op": "merge_classes",
                    "source": classes["Pumping Unit"],
                    "target": classes["Pump"],
                }],
            }
        return {
            "role": "assistant",
            "content": json.dumps(
                {"answer": answers[active_prompt], "suggestion": suggestion},
                ensure_ascii=False,
            ),
        }

    monkeypatch.setattr(agent_runtime.openrouter, "chat_message", fake_chat_message)
    conversation: list[dict[str, str]] = []
    results = []
    with Session(database) as session:
        user = session.get(User, user_id)
        ks = session.get(KnowledgeSystem, ks_id)
        for prompt in prompts:
            conversation.append({"role": "user", "content": prompt})
            result = asyncio.run(agent_runtime.run(
                session=session,
                user=user,
                ks=ks,
                conversation=conversation,
            ))
            results.append(result)
            conversation.append({"role": "assistant", "content": result["answer"]})

    expected_queue_reads = {
        ("conflicts", "open"),
        ("entity_resolution", "pending"),
        ("terminology", "pending"),
        ("validation", "all"),
    }
    for result in results:
        actual_queue_reads = {
            (step["arguments"].get("queue"), step["arguments"].get("status"))
            for step in result["trace"]
            if step["tool"] == "list_review_items"
        }
        assert actual_queue_reads == expected_queue_reads

    for prompt in prompts[:2]:
        messages = next(messages for active, messages in reversed(model_calls) if active == prompt)
        observations = "\n".join(
            str(message.get("content") or "")
            for message in messages
            if message.get("role") == "tool"
        )
        assert str(conflict_id) in observations
        assert "Pump and Pumping Unit may be duplicates" in observations
        assert str(entity_resolution_id) in observations
        assert "海探1" in observations
        assert str(terminology_id) in observations
        assert "serial number" in observations
        assert validation_id in observations

    advice_tools = [step["tool"] for step in results[1]["trace"]]
    assert "get_conflicts_context" in advice_tools
    # The execution follow-up is now deterministic once the runtime has prefetched every live
    # review row and conflict evidence.  It must not spend another model turn rebuilding the same
    # registered operation or enter a schema-repair loop.
    assert not any(active == prompts[2] for active, _messages in model_calls)

    proposal = results[2]["proposal"]
    assert proposal is not None
    assert proposal["preview"]["dry_run"] is True
    assert results[2]["answer"].startswith("已生成 1 项变更预览，尚未写入。")
    third_tools = [step["tool"] for step in results[2]["trace"]]
    assert "get_conflicts_context" in third_tools
    assert third_tools[-1] == "preview_ontology_changes"
    assert "decide_review_item" not in third_tools
    assert set(third_tools) != {"get_workspace_context", "list_review_items"}
    assert {item["label"] for item in schema.build_view(graph_iri)["classes"]} == {
        "Pump",
        "Pumping Unit",
    }


def test_review_execute_fast_path_handles_six_conflicts_and_two_terms_without_model(monkeypatch) -> None:
    database = _database()
    monkeypatch.setattr(store, "_store", Store())
    store._graph_locks.clear()
    store._recorders.clear()
    user_id, ks_id, graph_iri, base_iri = _workspace(database)

    for index in range(1, 7):
        editor.apply_edit(graph_iri, base_iri, {"op": "add_class", "label": f"Source {index}"})
        editor.apply_edit(graph_iri, base_iri, {"op": "add_class", "label": f"Target {index}"})
    classes = {item["label"]: item["iri"] for item in schema.build_view(graph_iri)["classes"]}

    conflict_rows = []
    contexts = {}
    expected_operations = []
    for index in range(1, 7):
        item_id = 100 + index
        operation = {
            "op": "merge_classes",
            "source": classes[f"Source {index}"],
            "target": classes[f"Target {index}"],
        }
        resolution = {
            "id": f"merge-{index}",
            "label": f"Merge Source {index} into Target {index}",
            "op": operation,
        }
        resolutions = [resolution]
        if index == 6:
            resolutions.append({
                "id": "merge-6-reverse",
                "label": "Merge Target 6 into Source 6",
                "op": {
                    "op": "merge_classes",
                    "source": classes["Target 6"],
                    "target": classes["Source 6"],
                },
            })
        payload = {
            "entities": [
                {"iri": classes[f"Source {index}"], "label": f"Source {index}"},
                {"iri": classes[f"Target {index}"], "label": f"Target {index}"},
            ],
            "resolutions": resolutions,
        }
        if index <= 3:
            payload["recommendation"] = {
                "resolution_id": f"merge-{index}",
                "confidence": 0.99,
                "reason": "The source explicitly identifies both names as equivalent.",
            }
        conflict_rows.append({
            "id": item_id,
            "title": f"Source {index} and Target {index} may be duplicates",
            "detail": "Review the registered merge direction.",
            "payload": payload,
        })
        contexts[item_id] = {
            "conflict": {"id": item_id},
            "evidence": [{"text": f"Source {index} is an alias of Target {index}."}],
        }
        if index <= 5:
            expected_operations.append(operation)

    terminology_rows = [
        {"id": 201, "term": "act of sensing", "action": "add_alias"},
        {"id": 202, "term": "actuation input", "action": "add_alias"},
    ]
    mcp_calls: list[tuple[str, dict]] = []

    async def fake_mcp_call(name, arguments, *, user, ks):
        assert user.id == user_id
        assert ks.id == ks_id
        mcp_calls.append((name, copy.deepcopy(arguments)))
        if name == "get_workspace_context":
            return {
                "knowledge_system": {"stats": {"classes": 12, "properties": 0}},
                "review_counts": {
                    "open_conflicts": 6,
                    "pending_entity_resolution": 0,
                    "pending_terminology": 2,
                    "validation_violations": 0,
                },
            }
        if name == "list_review_items":
            rows = conflict_rows if arguments["queue"] == "conflicts" else terminology_rows
            offset = int(arguments.get("offset") or 0)
            limit = int(arguments.get("limit") or 50)
            return {"items": rows[offset:offset + limit], "total": len(rows)}
        if name == "get_conflicts_context":
            items = [contexts[item_id] for item_id in arguments["conflict_ids"]]
            return {"items": items, "total": len(items)}
        if name == "preview_ontology_changes":
            assert arguments["operations"] == expected_operations
            assert arguments["include_rdf_diff"] is False
            return {
                "valid": True,
                "dry_run": True,
                "applied": 0,
                "operations": arguments["operations"],
                "destructive_operations": ["merge_classes"] * len(arguments["operations"]),
                "requires_confirmation": True,
                "base_revision": arguments["expected_revision"],
                "revision": "dry-run-revision",
                "diff": {
                    "counts": {
                        "tbox_added": 0,
                        "tbox_removed": len(arguments["operations"]),
                        "abox_added": 0,
                        "abox_removed": 0,
                    },
                },
                "impact": {"operations": [], "totals": {}},
                "conflicts": [],
                "structural_validation": {"committable": True},
            }
        raise AssertionError(f"unexpected MCP call: {name}")

    async def forbidden_model_call(*_args, **_kwargs):
        raise AssertionError("review execution fast path must not call the model")

    async def forbidden_tool_specs():
        raise AssertionError("review execution fast path does not need model tool schemas")

    monkeypatch.setattr(agent_runtime, "_mcp_call", fake_mcp_call)
    monkeypatch.setattr(agent_runtime, "_tool_specs", forbidden_tool_specs)
    monkeypatch.setattr(agent_runtime.openrouter, "chat_message", forbidden_model_call)
    monkeypatch.setattr(agent_runtime.openrouter, "chat_message_stream", forbidden_model_call)
    conversation = [
        {"role": "user", "content": "帮我分析待审批项目"},
        {"role": "assistant", "content": "发现 6 个冲突和 2 个术语提案。"},
        {"role": "user", "content": "该如何审批"},
        {
            "role": "assistant",
            "content": (
                "前三项有唯一高置信推荐；建议采用 `Merge Source 4 into Target 4`；"
                "建议采用 `Merge Source 5 into Target 5`；冲突 #106 的方向仍需选择。"
                "术语 act of sensing 与 actuation input 均需接受或拒绝。"
            ),
        },
        {"role": "user", "content": "帮我处理"},
    ]
    before = {item["label"] for item in schema.build_view(graph_iri)["classes"]}
    events = []

    async def sink(event):
        events.append(event)

    with Session(database) as session:
        result = asyncio.run(agent_runtime.run(
            session=session,
            user=session.get(User, user_id),
            ks=session.get(KnowledgeSystem, ks_id),
            conversation=conversation,
            event_sink=sink,
            native_answer_stream=True,
        ))

    assert result["answer"].startswith("已生成 5 项变更预览，尚未写入。")
    assert "#106" in result["answer"]
    assert "#106/merge-6" in result["answer"]
    assert "#106/merge-6-reverse" in result["answer"]
    assert "act of sensing" in result["answer"]
    assert "actuation input" in result["answer"]
    assert "接受或拒绝" in result["answer"]
    assert result["proposal"] is not None
    assert result["proposal"]["operations"] == expected_operations
    assert result["proposal"]["preview"]["dry_run"] is True
    assert len(result["proposal"]["review_items"]) == 5
    assert all(item["content"] and item["decision"] for item in result["proposal"]["review_items"])
    assert "".join(event["delta"] for event in events if event["type"] == "delta") == result["answer"]
    assert not any(event["type"] == "answer_reset" for event in events)
    assert [name for name, _arguments in mcp_calls] == [
        "get_workspace_context",
        "list_review_items",
        "list_review_items",
        "get_conflicts_context",
        "preview_ontology_changes",
    ]
    assert result["trace"][-1]["tool"] == "preview_ontology_changes"
    assert {item["label"] for item in schema.build_view(graph_iri)["classes"]} == before


def test_review_execute_plan_caps_preview_at_twenty_and_defers_the_rest() -> None:
    rows = []
    contexts = []
    for item_id in range(1, 22):
        rows.append({
            "id": item_id,
            "title": f"Conflict {item_id}",
            "payload": {
                "entities": [{"label": f"Entity {item_id}"}],
                "recommendation": {
                    "resolution_id": f"choice-{item_id}",
                    "confidence": 0.99,
                },
                "resolutions": [{
                    "id": f"choice-{item_id}",
                    "label": f"Registered choice {item_id}",
                    "op": {"op": "add_class", "label": f"Class {item_id}"},
                }],
            },
        })
        contexts.append({
            "conflict": {"id": item_id},
            "evidence": [{"text": f"Evidence {item_id}"}],
        })
    observations = [
        {
            "tool": "get_workspace_context",
            "arguments": {},
            "result": {"review_counts": {"open_conflicts": 21}},
        },
        {
            "tool": "list_review_items",
            "arguments": {"queue": "conflicts", "status": "open"},
            "result": {"items": rows, "total": 21},
        },
        {
            "tool": "get_conflicts_context",
            "arguments": {"conflict_ids": list(range(1, 22))},
            "result": {"items": contexts, "total": 21},
        },
    ]
    conversation = [
        {"role": "assistant", "content": "已检查全部待审批冲突。"},
        {"role": "user", "content": "帮我执行"},
    ]

    plan = agent_runtime._review_execute_plan(conversation, observations)

    assert plan is not None
    assert len(plan["operations"]) == 20
    assert len(plan["selected"]) == 20
    assert [item["item_id"] for item in plan["overflow"]] == [21]
    answer = agent_runtime._review_execute_answer(plan, "zh-CN", preview_ready=True)
    assert answer.startswith("已生成 20 项变更预览，尚未写入。")
    assert "#21" in answer
    assert "单次预览最多 20 项" in answer
    assert "下一批继续处理" in answer


def test_review_coverage_uses_conflict_entity_labels_not_repeated_titles() -> None:
    observations = [{
        "tool": "list_review_items",
        "arguments": {"queue": "conflicts", "status": "open"},
        "result": {
            "items": [
                {
                    "id": 1,
                    "title": "Possible duplicate classes",
                    "payload": {"entities": [{"label": "Pump"}, {"label": "Pumping Unit"}]},
                },
                {
                    "id": 2,
                    "title": "Possible duplicate classes",
                    "payload": {"entities": [{"label": "Valve"}, {"label": "Control Valve"}]},
                },
            ],
        },
    }]

    feedback = agent_runtime._review_answer_coverage_feedback(
        "list",
        observations,
        "Pump may duplicate another registered class.",
    )

    assert feedback is not None
    assert "Missing queues=[]" in feedback
    assert "conflicts: Valve" in feedback
    assert "conflicts: Pump" not in feedback


def test_execute_proposal_answer_must_name_every_unexecuted_review_row() -> None:
    operation = {
        "op": "merge_classes",
        "source": "urn:factory:PumpingUnit",
        "target": "urn:factory:Pump",
    }
    observations = [
        {
            "tool": "list_review_items",
            "arguments": {"queue": "conflicts", "status": "open"},
            "result": {"items": [{
                "id": 1,
                "title": "Possible duplicate classes",
                "payload": {
                    "entities": [{"label": "Pump"}, {"label": "Pumping Unit"}],
                    "recommendation": {"resolution_id": "merge-pump", "confidence": 0.99},
                    "resolutions": [{
                        "id": "merge-pump",
                        "label": "Merge Pumping Unit into Pump",
                        "op": operation,
                    }],
                },
            }]},
        },
        {
            "tool": "list_review_items",
            "arguments": {"queue": "terminology", "status": "pending"},
            "result": {"items": [
                {"id": 2, "term": "act of sensing"},
                {"id": 3, "term": "actuation input"},
            ]},
        },
        {
            "tool": "get_conflicts_context",
            "arguments": {"conflict_ids": [1]},
            "result": {"items": [{
                "conflict": {"id": 1},
                "evidence": [{"text": "Pumping Unit is an alias of Pump."}],
            }]},
        },
    ]
    parsed = {
        "answer": (
            "Pump duplicate merge is ready as a dry-run. The terminology item act of sensing "
            "still needs accept/reject."
        ),
        "suggestion": {
            "summary": "Merge duplicate pump classes",
            "reason": "Supported by source evidence",
            "operations": [operation],
        },
    }

    feedback = agent_runtime._review_response_feedback(
        [
            {"role": "user", "content": "帮我分析待审批项目"},
            {"role": "assistant", "content": "发现冲突和两个待审核术语。"},
            {"role": "user", "content": "帮我执行"},
        ],
        observations,
        parsed,
    )

    assert feedback is not None
    assert "Keep the valid dry-run suggestion" in feedback
    assert "terminology: actuation input" in feedback


def test_review_advice_rejects_a_verbatim_inventory_repeat() -> None:
    inventory = "当前待审批项目包括 6 个冲突和 2 个术语提案。"

    feedback = agent_runtime._review_response_feedback(
        [
            {"role": "user", "content": "帮我分析待审批项目"},
            {"role": "assistant", "content": inventory},
            {"role": "user", "content": "该如何审批"},
        ],
        [],
        {"answer": inventory, "suggestion": None},
    )

    assert feedback is not None
    assert "merely repeats" in feedback


def test_review_analysis_is_inventory_but_advice_requires_registered_actions() -> None:
    operation = {
        "op": "merge_classes",
        "source": "urn:factory:PumpingUnit",
        "target": "urn:factory:Pump",
    }
    observations = [
        {
            "tool": "list_review_items",
            "arguments": {"queue": "conflicts", "status": "open"},
            "result": {"items": [{
                "id": 7,
                "title": "Possible duplicate classes",
                "payload": {
                    "entities": [{"label": "Pump"}, {"label": "Pumping Unit"}],
                    "recommendation": {"resolution_id": "merge-pump", "confidence": 0.99},
                    "resolutions": [{
                        "id": "merge-pump",
                        "label": "Merge Pumping Unit into Pump",
                        "op": operation,
                    }],
                },
            }]},
        },
        {
            "tool": "list_review_items",
            "arguments": {"queue": "terminology", "status": "pending"},
            "result": {"items": [{"id": 8, "term": "pump unit"}]},
        },
        {
            "tool": "get_conflicts_context",
            "arguments": {"conflict_ids": [7]},
            "result": {"items": [{
                "conflict": {"id": 7},
                "evidence": [{"text": "Pumping Unit is an alias of Pump."}],
            }]},
        },
    ]

    assert agent_runtime._review_intent([
        {"role": "user", "content": "帮我分析待审批项目"},
    ]) == "list"
    feedback = agent_runtime._review_response_feedback(
        [
            {"role": "assistant", "content": "发现 Pump 冲突和 pump unit 术语。"},
            {"role": "user", "content": "该如何审批"},
        ],
        observations,
        {
            "answer": "Pump 存在重复冲突，pump unit 是待审批术语。",
            "suggestion": None,
        },
    )

    assert feedback is not None
    assert "inventory is not enough" in feedback
    assert "Merge Pumping Unit into Pump" in feedback
    assert "terminology action missing=True" in feedback


def test_review_execution_rejects_an_arbitrary_unselected_resolution() -> None:
    merge_left = {
        "op": "merge_classes",
        "source": "urn:factory:Pump",
        "target": "urn:factory:PumpingUnit",
    }
    merge_right = {
        "op": "merge_classes",
        "source": "urn:factory:PumpingUnit",
        "target": "urn:factory:Pump",
    }
    observations = [
        {
            "tool": "list_review_items",
            "arguments": {"queue": "conflicts", "status": "open"},
            "result": {"items": [{
                "id": 21,
                "title": "Possible duplicate classes",
                "payload": {
                    "entities": [{"label": "Pump"}, {"label": "Pumping Unit"}],
                    "resolutions": [
                        {"id": "merge-left", "label": "Merge Pump into Pumping Unit", "op": merge_left},
                        {"id": "merge-right", "label": "Merge Pumping Unit into Pump", "op": merge_right},
                    ],
                },
            }]},
        },
        {
            "tool": "get_conflicts_context",
            "arguments": {"conflict_ids": [21]},
            "result": {"items": [{
                "conflict": {"id": 21},
                "evidence": [{"text": "Both labels occur in the source."}],
            }]},
        },
    ]

    feedback = agent_runtime._review_response_feedback(
        [
            {"role": "user", "content": "帮我分析待审批项目"},
            {
                "role": "assistant",
                "content": (
                    "冲突 #21 有 `Merge Pump into Pumping Unit` 和 "
                    "`Merge Pumping Unit into Pump` 两个候选，请选择方向。"
                ),
            },
            {"role": "user", "content": "帮我执行"},
        ],
        observations,
        {
            "answer": "已选择一个方向生成预览。",
            "suggestion": {
                "summary": "Merge duplicate classes",
                "reason": "Candidate resolution",
                "operations": [merge_right],
            },
        },
    )

    assert feedback is not None
    assert "No unique live review resolution" in feedback


def test_review_execution_answer_states_dry_run_count_and_no_write() -> None:
    operation = {
        "op": "merge_classes",
        "source": "urn:factory:PumpingUnit",
        "target": "urn:factory:Pump",
    }
    observations = [
        {
            "tool": "list_review_items",
            "arguments": {"queue": "conflicts", "status": "open"},
            "result": {"items": [{
                "id": 22,
                "title": "Possible duplicate classes",
                "payload": {
                    "entities": [{"label": "Pump"}, {"label": "Pumping Unit"}],
                    "recommendation": {"resolution_id": "merge-pump", "confidence": 0.99},
                    "resolutions": [{
                        "id": "merge-pump",
                        "label": "Merge Pumping Unit into Pump",
                        "op": operation,
                    }],
                },
            }]},
        },
        {
            "tool": "get_conflicts_context",
            "arguments": {"conflict_ids": [22]},
            "result": {"items": [{
                "conflict": {"id": 22},
                "evidence": [{"text": "Pumping Unit is an alias of Pump."}],
            }]},
        },
    ]

    feedback = agent_runtime._review_response_feedback(
        [
            {"role": "user", "content": "帮我分析待审批项目"},
            {"role": "assistant", "content": "建议采用 #22/merge-pump 合并 Pump 重复类。"},
            {"role": "user", "content": "帮我执行"},
        ],
        observations,
        {
            "answer": "Pump 重复类合并预览已经准备好。",
            "suggestion": {
                "summary": "Merge duplicate classes",
                "reason": "Supported by evidence",
                "operations": [operation],
            },
        },
    )

    assert feedback is not None
    assert "1 operation(s)" in feedback
    assert "have not been written" in feedback


def test_review_execution_requires_queue_specific_blocker_actions() -> None:
    observations = [
        {
            "tool": "list_review_items",
            "arguments": {"queue": "entity_resolution", "status": "pending"},
            "result": {"items": [{"id": 31, "surface_form": "海探1"}]},
        },
        {
            "tool": "list_review_items",
            "arguments": {"queue": "terminology", "status": "pending"},
            "result": {"items": [{"id": 32, "term": "actuation input"}]},
        },
    ]

    feedback = agent_runtime._review_response_feedback(
        [
            {"role": "user", "content": "帮我分析待审批项目"},
            {"role": "assistant", "content": "发现海探1和actuation input。"},
            {"role": "user", "content": "帮我执行"},
        ],
        observations,
        {
            "answer": "海探1实体消歧与 actuation input 术语都需要你确认选择。",
            "suggestion": None,
        },
    )

    assert feedback is not None
    assert "accept/reject" in feedback
    assert "match/new" in feedback


def test_negated_resolution_label_is_not_treated_as_a_selection() -> None:
    candidate = {
        "queue": "conflicts",
        "item_id": 41,
        "choice_id": "merge-pump",
        "label": "Merge Pumping Unit into Pump",
        "entity_labels": ["Pump", "Pumping Unit"],
        "operation": {
            "op": "merge_classes",
            "source": "urn:factory:PumpingUnit",
            "target": "urn:factory:Pump",
        },
        "has_evidence": True,
        "recommended": False,
    }

    selected = agent_runtime._selected_review_candidates(
        [candidate],
        [
            {"role": "assistant", "content": "不要采用 `Merge Pumping Unit into Pump`。"},
            {"role": "user", "content": "帮我执行"},
        ],
    )

    assert selected == []


def test_review_intent_uses_the_same_twelve_message_window_as_the_model() -> None:
    conversation = [{"role": "user", "content": "帮我分析待审批项目"}]
    conversation.extend(
        {"role": "assistant" if index % 2 else "user", "content": f"unrelated {index}"}
        for index in range(10)
    )
    conversation.append({"role": "user", "content": "帮我执行"})

    assert agent_runtime._review_intent(conversation) == "execute"


def test_review_tool_arguments_normalize_display_aliases_and_live_statuses() -> None:
    assert agent_runtime._normalize_tool_arguments(
        "list_review_items",
        {"queue": "entity resolution", "status": "open"},
    ) == {"queue": "entity_resolution", "status": "pending"}
    assert agent_runtime._normalize_tool_arguments(
        "list_review_items",
        {"queue": "terminology proposals", "status": "unresolved"},
    ) == {"queue": "terminology", "status": "pending"}
    assert agent_runtime._normalize_tool_arguments(
        "list_review_items",
        {"queue": "validation violations", "status": "open"},
    ) == {"queue": "validation", "status": "all"}
    untouched = {"query": "Pump"}
    assert agent_runtime._normalize_tool_arguments("search_ontology", untouched) is untouched


def test_conflict_only_followup_ignores_older_terminology_observations() -> None:
    observations = [
        {
            "tool": "list_review_items",
            "arguments": {"queue": "conflicts", "status": "open"},
            "result": {
                "items": [{
                    "id": 11,
                    "title": "Possible duplicate",
                    "payload": {"entities": [{"label": "Pump"}], "resolutions": []},
                }],
                "total": 1,
            },
        },
        {
            "tool": "list_review_items",
            "arguments": {"queue": "terminology", "status": "pending"},
            "result": {"items": [{"id": 22, "term": "legacy alias"}], "total": 1},
        },
    ]
    answer = "冲突 #11（Pump）需要你从已登记方案中选择处理方向。"
    conflict_followup = [
        {"role": "user", "content": "帮我分析待审批项目"},
        {"role": "assistant", "content": "存在冲突和术语提案。"},
        {"role": "user", "content": "现在只看冲突，应该如何审批？"},
    ]
    generic_followup = [
        *conflict_followup[:-1],
        {"role": "user", "content": "这些待审批项目应该如何审批？"},
    ]

    assert agent_runtime._review_response_feedback(
        conflict_followup,
        observations,
        {"answer": answer, "suggestion": None},
    ) is None
    assert "terminology" in agent_runtime._review_response_feedback(
        generic_followup,
        observations,
        {"answer": answer, "suggestion": None},
    )


@pytest.mark.parametrize(("message", "expected"), [
    ("只看冲突审核队列", {"conflicts"}),
    ("show the conflict review queue", {"conflicts"}),
    ("检查实体解析队列", {"entity_resolution"}),
    ("show entity resolution", {"entity_resolution"}),
    ("如何处理术语审批？", {"terminology"}),
    ("review terminology", {"terminology"}),
    ("查看校验队列", {"validation"}),
    ("show the validation queue", {"validation"}),
    ("帮我分析待审批项目", {
        "conflicts", "entity_resolution", "terminology", "validation",
    }),
])
def test_review_queue_scope_prefers_explicit_queue_names(
    message: str,
    expected: set[str],
) -> None:
    assert agent_runtime._review_queue_scope([
        {"role": "user", "content": message},
    ]) == expected
    assert agent_runtime._review_intent([
        {"role": "user", "content": message},
    ]) is not None


@pytest.mark.parametrize("follow_up", [
    "帮我执行",
    "帮我处理",
    "处理这些项目",
    "按建议处理",
    "handle them",
    "apply the recommendations",
])
def test_review_execute_intent_recognizes_short_action_followups(follow_up: str) -> None:
    conversation = [
        {"role": "user", "content": "讲讲目前的待审核队列"},
        {"role": "assistant", "content": "有 6 个冲突和 2 个术语提案。"},
        {"role": "user", "content": follow_up},
    ]

    assert agent_runtime._review_intent(conversation) == "execute"


@pytest.mark.parametrize("message", [
    "不要重新读取工作区，直接基于刚才的证据回答",
    "不需要重新读取",
    "无需刷新",
    "不要重查",
    "do not refresh",
    "don't check again",
    "do not re-read the evidence",
])
def test_negated_refresh_language_keeps_fresh_evidence(message: str) -> None:
    assert agent_api._force_evidence_refresh(message) is False


@pytest.mark.parametrize("message", [
    "请重新检查最新状态",
    "不要依赖旧数据，请刷新",
    "refresh the workspace",
    "please re-read the evidence",
])
def test_positive_refresh_language_bypasses_fresh_evidence(message: str) -> None:
    assert agent_api._force_evidence_refresh(message) is True


def test_agent_stream_emits_live_localized_audit_events_and_markdown_deltas(monkeypatch) -> None:
    database = _database()
    monkeypatch.setattr(mcp_server, "engine", database)
    monkeypatch.setattr(store, "_store", Store())
    store._graph_locks.clear()
    store._recorders.clear()
    user_id, ks_id, _graph_iri, _base_iri = _workspace(database)
    model_started = asyncio.Event()
    release_model = asyncio.Event()
    answer = (
        "## 检查结果\n\n"
        "当前工作区已经完成实时检查。下面的建议基于工具返回的结构化观察，"
        "并且不会自动写入本体。"
    )
    model_call_count = 0

    async def fake_chat_message(*_args, **_kwargs):
        nonlocal model_call_count
        model_call_count += 1
        if model_call_count == 1:
            model_started.set()
            await release_model.wait()
            return {
                "role": "assistant",
                "content": None,
                "tool_calls": [{
                    "id": "inspect-workspace",
                    "type": "function",
                    "function": {
                        "name": "get_workspace_context",
                        "arguments": "{}",
                    },
                }],
            }
        return {
            "role": "assistant",
            "content": json.dumps({"answer": answer, "suggestion": None}, ensure_ascii=False),
        }

    monkeypatch.setattr(agent_runtime.openrouter, "chat_message", fake_chat_message)

    async def scenario():
        events = []
        with Session(database) as session:
            async def consume() -> None:
                async for event in agent_runtime.stream(
                    session=session,
                    user=session.get(User, user_id),
                    ks=session.get(KnowledgeSystem, ks_id),
                    conversation=[{"role": "user", "content": "请检查当前工作区并给出结论"}],
                ):
                    events.append(event)

            consumer = asyncio.create_task(consume())
            await asyncio.wait_for(model_started.wait(), timeout=2)
            # No tool is called before the model has classified the request and selected one.
            assert not any(event["type"] == "trace" for event in events)
            release_model.set()
            await asyncio.wait_for(consumer, timeout=2)
        return events

    events = asyncio.run(scenario())
    trace_events = [event for event in events if event["type"] == "trace"]
    deltas = [event["delta"] for event in events if event["type"] == "delta"]

    assert events[0]["type"] == "progress"
    assert trace_events[0]["trace"]["tool"] == "get_workspace_context"
    assert "先读取" in trace_events[0]["trace"]["reason"]
    assert "个类" in trace_events[0]["trace"]["summary"]
    assert len(deltas) >= 2
    assert "".join(deltas) == answer
    assert events[-1]["type"] == "done"
    assert events[-1] == {"type": "done", "answer": "", "trace": [], "proposal": None}


def test_preview_trace_summary_counts_dry_run_operations() -> None:
    preview = {
        "dry_run": True,
        "applied": 0,
        "operations": [{"op": "add_class", "label": "Equipment"}],
        "diff": {"counts": {"tbox_added": 2, "tbox_removed": 0}},
    }

    assert agent_runtime._trace_summary("preview_ontology_changes", preview) == (
        "Previewed 1 operation(s), 2 RDF changes"
    )
    assert agent_runtime._trace_summary("preview_ontology_changes", preview, "zh-CN") == (
        "已预检 1 项操作、2 项 RDF 变更"
    )


def test_agent_stream_emits_proposal_before_complete_result(monkeypatch) -> None:
    proposal = {
        "summary": "Add Equipment",
        "reason": "The root class needs a broader type.",
        "operations": [{"op": "add_class", "label": "Equipment"}],
        "revision": "rev-1",
        "preview": {"dry_run": True},
    }
    result = {
        "answer": "A validated **Markdown** answer.",
        "trace": [{
            "tool": "get_workspace_context",
            "arguments": {},
            "summary": "Inspected workspace",
            "reason": "Ground the response in live state.",
        }],
        "proposal": proposal,
    }

    async def fake_run(**kwargs):
        await kwargs["event_sink"]({"type": "progress", "phase": "tool", "title": "Inspect"})
        await kwargs["event_sink"]({"type": "trace", "trace": result["trace"][0]})
        return result

    monkeypatch.setattr(agent_runtime, "run", fake_run)

    async def collect():
        return [
            event async for event in agent_runtime.stream(
                session=None,
                user=User(id=1, username="stream-user", password_hash="unused"),
                ks=KnowledgeSystem(
                    id=1,
                    name="Stream",
                    public_id="stream",
                    graph_iri="urn:stream:tbox",
                    base_iri="urn:stream:onto:",
                ),
                conversation=[{"role": "user", "content": "Inspect"}],
            )
        ]

    events = asyncio.run(collect())
    types = [event["type"] for event in events]
    assert types[:2] == ["progress", "trace"]
    assert types[-2:] == ["proposal", "done"]
    assert events[-2]["proposal"] == proposal
    assert events[-1] == {"type": "done", "answer": "", "trace": [], "proposal": None}


def test_agent_request_ignores_legacy_frontend_page_context() -> None:
    request = agent_api.AgentRequest.model_validate({
        "messages": [{"role": "user", "content": "Inspect Pump"}],
        "context": {
            "path": "/knowledge/1/ontology",
            "section": "ontology",
            "selected_iri": "urn:outside:hint",
            "selected_label": "Injected hint",
        },
    })

    assert request.model_dump() == {
        "messages": [{"role": "user", "content": "Inspect Pump"}],
    }
    assert not hasattr(request, "context")


def test_agent_stream_endpoint_frames_sse_and_reports_safe_errors(monkeypatch) -> None:
    database = _database()
    user_id, ks_id, _graph_iri, _base_iri = _workspace(database)
    monkeypatch.setattr(agent_api.model_config, "use_ks_connections", lambda *_args: nullcontext())
    monkeypatch.setattr(agent_api.prompt_config, "use_ks_prompts", lambda *_args: nullcontext())
    body = agent_api.AgentRequest(messages=[agent_api.AgentMessage(role="user", content="检查")])

    async def fake_stream(**_kwargs):
        yield {"type": "progress", "phase": "tool", "title": "检查工作区"}
        yield {"type": "delta", "delta": "**完成**"}
        yield {"type": "done", "answer": "**完成**", "trace": [], "proposal": None}

    async def consume(response) -> str:
        chunks = []
        async for chunk in response.body_iterator:
            chunks.append(chunk.decode() if isinstance(chunk, bytes) else chunk)
        return "".join(chunks)

    with Session(database) as session:
        user = session.get(User, user_id)
        ks = session.get(KnowledgeSystem, ks_id)
        monkeypatch.setattr(agent_api.agent_runtime, "stream", fake_stream)
        response = asyncio.run(agent_api.chat_stream(body, ks, user, session))
        payload = asyncio.run(consume(response))

        assert response.media_type == "text/event-stream"
        assert response.headers["cache-control"] == "no-cache, no-transform"
        assert "event: progress\n" in payload
        assert '"title":"检查工作区"' in payload
        assert "event: delta\n" in payload
        assert "event: done\n" in payload

        async def failing_stream(**_kwargs):
            raise agent_runtime.AgentError("tool limit reached")
            yield  # pragma: no cover - keeps this an async generator

        monkeypatch.setattr(agent_api.agent_runtime, "stream", failing_stream)
        response = asyncio.run(agent_api.chat_stream(body, ks, user, session))
        payload = asyncio.run(consume(response))

    assert "event: error\n" in payload
    assert '"code":"agent_error"' in payload
    assert "tool limit reached" in payload
