from __future__ import annotations

import json
import asyncio

from fastapi import FastAPI
from fastapi.testclient import TestClient
from sqlalchemy.pool import StaticPool
from sqlmodel import Session, SQLModel, create_engine

from app import agent_memory, agent_runtime
from app.api import agent as agent_api
from app.db.database import get_session
from app.db.models import AgentConversation, KnowledgeSystem, User
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
        user = User(username="stream-owner", password_hash="unused")
        session.add(user)
        session.commit()
        session.refresh(user)
        ks = KnowledgeSystem(
            name="Stream memory",
            public_id="stream-memory",
            owner_id=user.id,
            graph_iri="urn:stream-memory:tbox",
            base_iri="urn:stream-memory:",
        )
        session.add(ks)
        session.commit()
        session.refresh(ks)
        return user.id, ks.id


def _sse_events(text: str) -> list[dict]:
    events: list[dict] = []
    for frame in text.replace("\r\n", "\n").split("\n\n"):
        if not frame.strip():
            continue
        event_type = "message"
        data = ""
        for line in frame.splitlines():
            if line.startswith("event:"):
                event_type = line.split(":", 1)[1].strip()
            elif line.startswith("data:"):
                data += line.split(":", 1)[1].strip()
        payload = json.loads(data)
        assert payload["type"] == event_type
        events.append(payload)
    return events


def test_force_evidence_refresh_respects_negation() -> None:
    assert agent_api._force_evidence_refresh("请重新读取工作区后回答") is True
    assert agent_api._force_evidence_refresh("不要依赖旧数据，请刷新") is True
    assert agent_api._force_evidence_refresh("不要重新读取，直接用刚才的证据") is False
    assert agent_api._force_evidence_refresh("无需获取最新数据") is False
    assert agent_api._force_evidence_refresh("Use prior evidence; don't refresh") is False


def test_conversation_stream_persists_and_reuses_fresh_tool_evidence(monkeypatch) -> None:
    database = _database()
    user_id, ks_id = _workspace(database)
    app = FastAPI()
    app.include_router(agent_api.router)
    revision = {"value": "evidence-v1"}
    actual_reads = {"count": 0}
    contexts: list[list[dict]] = []
    runtime_observations: list[list[dict]] = []

    monkeypatch.setattr(
        agent_memory,
        "current_evidence_revision",
        lambda _session, _ks: revision["value"],
    )

    async def fake_stream(
        *,
        evidence_lookup,
        evidence_sink,
        context_messages,
        context_observations,
        **_kwargs,
    ):
        contexts.append(context_messages)
        runtime_observations.append(context_observations)
        arguments = {"query": "Pump", "limit": 5}
        cached = await evidence_lookup("search_ontology", arguments)
        await evidence_sink({
            "kind": "tool_call",
            "call_id": f"search-{len(contexts)}",
            "tool": "search_ontology",
            "arguments": arguments,
        })
        if cached is None:
            actual_reads["count"] += 1
            result = {"items": [{"iri": "urn:Pump"}], "api_token": "never expose"}
            cached_from = None
        else:
            result = cached["result"]
            cached_from = cached["event_id"]
        await evidence_sink({
            "kind": "tool_result",
            "call_id": f"search-{len(contexts)}",
            "tool": "search_ontology",
            "arguments": arguments,
            "result": result,
            "cached": cached is not None,
            "cached_from_event_id": cached_from,
        })
        trace = {
            "tool": "search_ontology",
            "arguments": arguments,
            "summary": "Found Pump",
        }
        yield {"type": "commentary", "text": "Pump 已找到，继续核对它的标识。"}
        yield {"type": "trace", "trace": trace}
        yield {"type": "delta", "delta": "Pump "}
        yield {"type": "delta", "delta": "exists."}
        # A compatibility terminal payload may be shorter than the native deltas. Persistence
        # must keep the reconstructed stream instead of truncating the assistant turn to this.
        yield {"type": "done", "answer": "Pump", "trace": [], "proposal": None}

    monkeypatch.setattr(agent_runtime, "stream", fake_stream)

    with Session(database, expire_on_commit=False) as session:
        user = session.get(User, user_id)
        ks = session.get(KnowledgeSystem, ks_id)
        app.dependency_overrides[get_session] = lambda: session
        app.dependency_overrides[current_user] = lambda: user
        app.dependency_overrides[agent_api.ks_reader] = lambda: ks

        with TestClient(app) as client:
            first = client.post(
                f"/api/knowledge/{ks_id}/agent/chat/stream",
                json={"message": "Find Pump"},
            )
            assert first.status_code == 200
            first_events = _sse_events(first.text)
            assert first_events[0]["type"] == "turn_started"
            assert "".join(
                event["delta"] for event in first_events if event["type"] == "delta"
            ) == "Pump exists."
            assert [
                event["text"] for event in first_events if event["type"] == "commentary"
            ] == ["Pump 已找到，继续核对它的标识。"]
            conversation_id = first_events[0]["conversation_id"]
            assert agent_memory.owned_conversation(
                session,
                conversation_id=conversation_id,
                user_id=user_id,
                knowledge_system_id=ks_id,
            ) is not None
            cached_after_first = agent_memory.find_cached_tool_result(
                session,
                conversation_id=conversation_id,
                user_id=user_id,
                knowledge_system_id=ks_id,
                tool_name="search_ontology",
                arguments={"query": "Pump", "limit": 5},
                current_revision="evidence-v1",
            )
            assert cached_after_first is not None

            second = client.post(
                f"/api/knowledge/{ks_id}/agent/chat/stream",
                json={"message": "What about its IRI?", "conversation_id": conversation_id},
            )
            assert second.status_code == 200
            assert actual_reads["count"] == 1
            assert any(message["role"] == "tool" for message in contexts[1])
            assert "never expose" not in json.dumps(contexts[1], ensure_ascii=False)
            assert len(runtime_observations[1]) == 1
            assert runtime_observations[1][0]["tool"] == "search_ontology"
            assert runtime_observations[1][0]["result"]["api_token"] == "[redacted]"

            revision["value"] = "evidence-v2"
            third = client.post(
                f"/api/knowledge/{ks_id}/agent/chat/stream",
                json={"message": "Check it once more", "conversation_id": conversation_id},
            )
            assert third.status_code == 200
            assert actual_reads["count"] == 2
            assert runtime_observations[2] == []

            detail = client.get(
                f"/api/knowledge/{ks_id}/agent/conversations/{conversation_id}"
            )
            assert detail.status_code == 200
            turns = detail.json()["turns"]
            assert [turn["role"] for turn in turns] == [
                "user", "assistant", "user", "assistant", "user", "assistant",
            ]
            assert all(turn["status"] == "done" for turn in turns)
            assert turns[1]["content"] == "Pump exists."
            commentary_events = [
                event for event in turns[1]["events"] if event["kind"] == "commentary"
            ]
            assert commentary_events[0]["data"]["text"] == "Pump 已找到，继续核对它的标识。"
            public_events = json.dumps(turns[1]["events"], ensure_ascii=False)
            assert "never expose" not in public_events

        assert session.get(AgentConversation, conversation_id) is not None


def test_native_agent_stream_forwards_provider_answer_tokens(monkeypatch) -> None:
    database = _database()
    user_id, ks_id = _workspace(database)

    async def fake_provider_stream(*_args, **_kwargs):
        chunks = [
            '{"answer":"',
            "第一段",
            "\\n第二段",
            '","suggestion":null}',
        ]
        for chunk in chunks:
            yield {"type": "content_delta", "delta": chunk}
        yield {
            "type": "message",
            "message": {
                "role": "assistant",
                "content": "".join(chunks),
            },
        }

    async def no_prefetch(**_kwargs):
        return None

    monkeypatch.setattr(agent_runtime.openrouter, "chat_message_stream", fake_provider_stream)
    monkeypatch.setattr(agent_runtime, "_prefetch_review_evidence", no_prefetch)
    monkeypatch.setattr(agent_runtime, "_tool_specs", lambda: _async_value([]))

    async def collect():
        with Session(database, expire_on_commit=False) as session:
            user = session.get(User, user_id)
            ks = session.get(KnowledgeSystem, ks_id)
            return [
                event
                async for event in agent_runtime.stream(
                    session=session,
                    user=user,
                    ks=ks,
                    conversation=[{"role": "user", "content": "请直接回答"}],
                    native_tokens=True,
                )
            ]

    events = asyncio.run(collect())

    deltas = [event["delta"] for event in events if event["type"] == "delta"]
    assert deltas == ["第一段", "\n第二段"]
    assert "".join(deltas) == "第一段\n第二段"
    assert [event["type"] for event in events][-1] == "done"


def test_tool_commentary_requires_explicit_public_prefix() -> None:
    assert agent_runtime._public_tool_commentary({
        "content": "COMMENTARY: 已找到 6 个冲突，继续核对来源证据。",
    }) == "已找到 6 个冲突，继续核对来源证据。"
    assert agent_runtime._public_tool_commentary({
        "content": "I should inspect the graph because...",
    }) == ""
    assert agent_runtime._public_tool_commentary({
        "content": '{"answer":"draft"}',
    }) == ""


def test_native_chat_reconciles_terminal_answer_suffix(monkeypatch) -> None:
    async def fake_provider_stream(*_args, **_kwargs):
        yield {"type": "content_delta", "delta": '{"answer":"第一项'}
        yield {
            "type": "message",
            "message": {
                "role": "assistant",
                "content": '{"answer":"第一项\\n第二项","suggestion":null}',
            },
        }

    monkeypatch.setattr(agent_runtime.openrouter, "chat_message_stream", fake_provider_stream)

    async def collect():
        events = []

        async def sink(event):
            events.append(event)

        message, streamed = await agent_runtime._chat_message(
            [{"role": "user", "content": "列出全部项目"}],
            tools=[],
            event_sink=sink,
            native_answer_stream=True,
        )
        return message, streamed, events

    message, streamed, events = asyncio.run(collect())

    assert streamed is True
    assert message["content"].endswith('"suggestion":null}')
    assert events == [
        {"type": "delta", "delta": "第一项"},
        {"type": "delta", "delta": "\n第二项"},
    ]
    assert "".join(event["delta"] for event in events) == "第一项\n第二项"


def test_native_chat_hides_partial_candidate_when_terminal_answer_diverges(monkeypatch) -> None:
    async def fake_provider_stream(*_args, **_kwargs):
        yield {"type": "content_delta", "delta": '{"answer":"错误候选'}
        yield {
            "type": "message",
            "message": {
                "role": "assistant",
                "content": '{"answer":"正确答案","suggestion":null}',
            },
        }

    monkeypatch.setattr(agent_runtime.openrouter, "chat_message_stream", fake_provider_stream)

    async def collect():
        events = []

        async def sink(event):
            events.append(event)

        _message, streamed = await agent_runtime._chat_message(
            [{"role": "user", "content": "给出结论"}],
            tools=[],
            event_sink=sink,
            native_answer_stream=True,
        )
        return streamed, events

    streamed, events = asyncio.run(collect())

    assert streamed is True
    assert events == [
        {"type": "delta", "delta": "正确答案"},
    ]


def test_native_agent_stream_never_exposes_candidate_rejected_by_validation(monkeypatch) -> None:
    database = _database()
    user_id, ks_id = _workspace(database)
    payloads = iter([
        json.dumps({"answer": "This draft is in the wrong language.", "suggestion": None}),
        json.dumps({"answer": "这是校验通过后的最终回答。", "suggestion": None}, ensure_ascii=False),
    ])

    async def fake_provider_stream(*_args, **_kwargs):
        payload = next(payloads)
        midpoint = len(payload) // 2
        for chunk in (payload[:midpoint], payload[midpoint:]):
            yield {"type": "content_delta", "delta": chunk}
        yield {
            "type": "message",
            "message": {"role": "assistant", "content": payload},
        }

    async def no_prefetch(**_kwargs):
        return None

    monkeypatch.setattr(agent_runtime.openrouter, "chat_message_stream", fake_provider_stream)
    monkeypatch.setattr(agent_runtime, "_prefetch_review_evidence", no_prefetch)
    monkeypatch.setattr(agent_runtime, "_tool_specs", lambda: _async_value([]))

    async def collect():
        with Session(database, expire_on_commit=False) as session:
            return [
                event
                async for event in agent_runtime.stream(
                    session=session,
                    user=session.get(User, user_id),
                    ks=session.get(KnowledgeSystem, ks_id),
                    conversation=[{"role": "user", "content": "请用中文解释什么是本体。"}],
                    native_tokens=True,
                )
            ]

    events = asyncio.run(collect())
    event_types = [event["type"] for event in events]
    answer = "".join(event["delta"] for event in events if event["type"] == "delta")

    assert "answer_reset" not in event_types
    assert "wrong language" not in answer
    assert answer == "这是校验通过后的最终回答。"
    assert event_types[-1] == "done"


async def _async_value(value):
    return value
