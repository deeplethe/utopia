from __future__ import annotations

import asyncio
import json
from contextlib import asynccontextmanager
from typing import Any

import httpx

from app.llm import openrouter


class _FakeStreamResponse:
    def __init__(self, lines: list[str | Exception], *, status_error: Exception | None = None):
        self.lines = lines
        self.status_error = status_error

    async def __aenter__(self):
        return self

    async def __aexit__(self, _exc_type, _exc, _tb):
        return False

    def raise_for_status(self) -> None:
        if self.status_error is not None:
            raise self.status_error

    async def aiter_lines(self):
        for line in self.lines:
            if isinstance(line, Exception):
                raise line
            yield line


class _FakeAsyncClient:
    def __init__(self, responses: list[_FakeStreamResponse], calls: list[dict[str, Any]]):
        self.responses = responses
        self.calls = calls

    async def __aenter__(self):
        return self

    async def __aexit__(self, _exc_type, _exc, _tb):
        return False

    def stream(self, method: str, url: str, **kwargs):
        self.calls.append({"method": method, "url": url, **kwargs})
        return self.responses.pop(0)


def _sse(payload: dict[str, Any]) -> str:
    return "data: " + json.dumps(payload, ensure_ascii=False)


def _install_stream_transport(
    monkeypatch,
    responses: list[_FakeStreamResponse],
) -> tuple[list[dict[str, Any]], list[tuple[str, str, int]]]:
    requests: list[dict[str, Any]] = []
    capacity_events: list[tuple[str, str, int]] = []
    client = _FakeAsyncClient(responses, requests)

    monkeypatch.setattr(
        openrouter,
        "_payload_headers_url",
        lambda messages, _model, _temperature, _max_tokens, tools=None, tool_choice=None: (
            {
                "model": "test-model",
                "messages": messages,
                **({"tools": tools, "tool_choice": tool_choice or "auto"} if tools else {}),
            },
            {"Authorization": "Bearer test"},
            "https://models.example/v1/chat/completions",
        ),
    )
    monkeypatch.setattr(openrouter, "_capacity_spec", lambda: ("llm:test:stream", 2))

    @asynccontextmanager
    async def fake_slot(key: str, limit: int):
        capacity_events.append(("enter", key, limit))
        try:
            yield
        finally:
            capacity_events.append(("exit", key, limit))

    monkeypatch.setattr(openrouter.capacity, "async_slot", fake_slot)
    monkeypatch.setattr(
        openrouter.httpx,
        "AsyncClient",
        lambda *_args, **_kwargs: client,
    )
    return requests, capacity_events


def test_chat_message_stream_yields_provider_content_deltas_and_terminal_message(
    monkeypatch,
) -> None:
    responses = [_FakeStreamResponse([
        _sse({"choices": [{"index": 0, "delta": {"role": "assistant", "content": "你"}}]}),
        _sse({"choices": [{"index": 0, "delta": {"content": "好"}}]}),
        _sse({
            "choices": [{"index": 0, "delta": {"content": "！"}, "finish_reason": "stop"}],
        }),
        "data:[DONE]",
    ])]
    requests, capacity_events = _install_stream_transport(monkeypatch, responses)

    async def collect():
        return [
            event async for event in openrouter.chat_message_stream(
                [{"role": "user", "content": "问候"}],
            )
        ]

    events = asyncio.run(collect())

    assert events == [
        {"type": "content_delta", "delta": "你"},
        {"type": "content_delta", "delta": "好"},
        {"type": "content_delta", "delta": "！"},
        {"type": "message", "message": {"role": "assistant", "content": "你好！"}},
    ]
    assert requests[0]["json"]["stream"] is True
    assert requests[0]["headers"]["Accept"] == "text/event-stream"
    assert capacity_events == [
        ("enter", "llm:test:stream", 2),
        ("exit", "llm:test:stream", 2),
    ]


def test_chat_message_stream_aggregates_fragmented_parallel_tool_calls(monkeypatch) -> None:
    responses = [_FakeStreamResponse([
        _sse({
            "choices": [{
                "index": 0,
                "delta": {
                    "role": "assistant",
                    "reasoning_content": "Need to inspect ",
                    "tool_calls": [
                        {
                            "index": 1,
                            "id": "call_",
                            "type": "function",
                            "function": {"name": "get_", "arguments": '{"id":'},
                        },
                        {
                            "index": 0,
                            "id": "call_",
                            "type": "function",
                            "function": {"name": "search_", "arguments": '{"query":"P'},
                        },
                    ],
                },
            }],
        }),
        _sse({
            "choices": [{
                "index": 0,
                "delta": {
                    "reasoning_content": "both entities.",
                    "tool_calls": [
                        {
                            "index": 0,
                            "id": "zero",
                            "function": {"name": "ontology", "arguments": 'ump"}'},
                        },
                        {
                            "index": 1,
                            "id": "one",
                            "function": {"name": "item", "arguments": "7}"},
                        },
                    ],
                },
                "finish_reason": "tool_calls",
            }],
        }),
        "data: [DONE]",
    ])]
    _install_stream_transport(monkeypatch, responses)

    async def collect():
        return [
            event async for event in openrouter.chat_message_stream(
                [{"role": "user", "content": "查找 Pump"}],
                tools=[{"type": "function", "function": {"name": "search_ontology"}}],
            )
        ]

    events = asyncio.run(collect())

    assert events == [{
        "type": "message",
        "message": {
            "role": "assistant",
            "content": None,
            "reasoning_content": "Need to inspect both entities.",
            "tool_calls": [
                {
                    "id": "call_zero",
                    "type": "function",
                    "function": {"name": "search_ontology", "arguments": '{"query":"Pump"}'},
                },
                {
                    "id": "call_one",
                    "type": "function",
                    "function": {"name": "get_item", "arguments": '{"id":7}'},
                },
            ],
        },
    }]


def test_deepseek_payload_backfills_reasoning_context_only_for_tool_messages(monkeypatch) -> None:
    messages = [
        {"role": "user", "content": "inspect"},
        {
            "role": "assistant",
            "content": None,
            "tool_calls": [{
                "id": "runtime-1",
                "type": "function",
                "function": {"name": "get_workspace_context", "arguments": "{}"},
            }],
        },
        {
            "role": "assistant",
            "content": None,
            "reasoning_content": "Provider reasoning that must be preserved.",
            "tool_calls": [{
                "id": "provider-2",
                "type": "function",
                "function": {"name": "search_ontology", "arguments": "{}"},
            }],
        },
    ]
    monkeypatch.setattr(
        "app.model_config.llm_conn",
        lambda: ("https://api.deepseek.com/v1", "secret", "deepseek-v4-pro"),
    )

    payload, _headers, _url = openrouter._payload_headers_url(
        messages, None, None, None,
    )

    assert payload["messages"][1]["reasoning_content"]
    assert payload["messages"][2]["reasoning_content"] == (
        "Provider reasoning that must be preserved."
    )
    assert "reasoning_content" not in messages[1]


def test_chat_message_stream_retries_only_before_first_upstream_event(monkeypatch) -> None:
    responses = [
        _FakeStreamResponse([httpx.ReadError("connection dropped before data")]),
        _FakeStreamResponse([
            _sse({"choices": [{"index": 0, "delta": {"content": "Recovered"}}]}),
            "data: [DONE]",
        ]),
    ]
    requests, _capacity_events = _install_stream_transport(monkeypatch, responses)
    sleeps: list[float] = []

    async def no_sleep(delay: float) -> None:
        sleeps.append(delay)

    monkeypatch.setattr(openrouter.asyncio, "sleep", no_sleep)

    async def collect():
        return [
            event async for event in openrouter.chat_message_stream(
                [{"role": "user", "content": "retry"}],
                retries=3,
            )
        ]

    events = asyncio.run(collect())

    assert len(requests) == 2
    assert sleeps == [2]
    assert events == [
        {"type": "content_delta", "delta": "Recovered"},
        {
            "type": "message",
            "message": {"role": "assistant", "content": "Recovered"},
        },
    ]


def test_chat_message_stream_does_not_retry_or_duplicate_after_partial_output(
    monkeypatch,
) -> None:
    responses = [
        _FakeStreamResponse([
            _sse({"choices": [{"index": 0, "delta": {"content": "partial"}}]}),
            httpx.ReadError("connection dropped mid-stream"),
        ]),
        _FakeStreamResponse([
            _sse({"choices": [{"index": 0, "delta": {"content": "duplicate"}}]}),
            "data: [DONE]",
        ]),
    ]
    requests, _capacity_events = _install_stream_transport(monkeypatch, responses)

    async def collect_until_error():
        events = []
        error: Exception | None = None
        try:
            async for event in openrouter.chat_message_stream(
                [{"role": "user", "content": "do not duplicate"}],
                retries=3,
            ):
                events.append(event)
        except Exception as exc:  # noqa: BLE001 - asserted below
            error = exc
        return events, error

    events, error = asyncio.run(collect_until_error())

    assert events == [{"type": "content_delta", "delta": "partial"}]
    assert isinstance(error, openrouter.LLMError)
    assert "after output started" in str(error)
    assert len(requests) == 1
