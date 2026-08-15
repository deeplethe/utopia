"""Minimal async OpenRouter chat client.

Mirrors the calling convention used in the ``llm_coupling`` project (Bearer key from
env, ``/chat/completions`` endpoint) but uses httpx with retries. Defaults to a cheap
DeepSeek model — never an expensive Claude model.
"""
from __future__ import annotations

import asyncio
import contextlib
import json
import re
import time
from collections.abc import AsyncIterator
from typing import Any

import httpx

from app.config import settings
from app.llm import capacity
from app.llm.http_transport import trust_environment_proxy


class LLMError(RuntimeError):
    pass


def _capacity_spec() -> tuple[str, int]:
    from app import model_config

    return model_config.llm_capacity_key(), model_config.llm_concurrency()


@contextlib.asynccontextmanager
async def capacity_slot():
    """Reserve a slot on the current LLM endpoint before caller timeouts start."""
    key, limit = _capacity_spec()
    async with capacity.async_slot(key, limit):
        yield


def _messages_for_endpoint(messages: list[dict[str, Any]], base_url: str) -> list[dict[str, Any]]:
    """Return an endpoint-compatible copy without exposing private reasoning to the UI.

    DeepSeek thinking models require ``reasoning_content`` to be passed back on every assistant
    tool-call message. OntoPilot also creates deterministic read observations before the first
    model turn, so those synthetic assistant messages have no provider reasoning to preserve.
    A small runtime marker satisfies DeepSeek's continuity contract while keeping the public
    transcript limited to explicit commentary and audit events. Other OpenAI-compatible endpoints
    receive the original schema unchanged.
    """

    if "api.deepseek.com" not in base_url.casefold():
        return messages
    prepared: list[dict[str, Any]] = []
    for message in messages:
        if (
            message.get("role") == "assistant"
            and message.get("tool_calls")
            and "reasoning_content" not in message
        ):
            message = {
                **message,
                "reasoning_content": "Tool-call context retained by the OntoPilot runtime.",
            }
        prepared.append(message)
    return prepared


def _payload_headers_url(messages, model, temperature, max_tokens, tools=None, tool_choice=None):
    from app import model_config  # effective (base_url, key, model) for the current unit of work

    base_url, key, conn_model = model_config.llm_conn()
    if not key:
        raise LLMError("No LLM API key set; add a model entry in Settings or backend/.env")
    payload: dict[str, Any] = {
        "model": model or conn_model or model_config.system_extract_model(),
        "messages": _messages_for_endpoint(messages, base_url),
        "temperature": settings.llm_temperature if temperature is None else temperature,
        "max_tokens": max_tokens or settings.llm_max_tokens,
    }
    if tools:
        payload["tools"] = tools
        payload["tool_choice"] = tool_choice or "auto"
    headers = {
        "Authorization": f"Bearer {key}",
        "Content-Type": "application/json",
        "HTTP-Referer": "http://localhost",
        "X-Title": "OntoPilot",
    }
    url = f"{base_url.rstrip('/')}/chat/completions"
    return payload, headers, url


def chat_sync(
    messages: list[dict[str, str]],
    *,
    model: str | None = None,
    temperature: float | None = None,
    max_tokens: int | None = None,
    retries: int = 3,
) -> str:
    """Synchronous chat completion (for use inside worker threads, e.g. entity resolution,
    where the surrounding code is already off the event loop and uses blocking I/O)."""
    payload, headers, url = _payload_headers_url(messages, model, temperature, max_tokens)
    key, limit = _capacity_spec()
    with capacity.sync_slot(key, limit):
        last_err: Exception | None = None
        for attempt in range(retries):
            try:
                resp = httpx.post(
                    url,
                    json=payload,
                    headers=headers,
                    timeout=settings.llm_timeout_s,
                    trust_env=trust_environment_proxy(url),
                )
                resp.raise_for_status()
                return resp.json()["choices"][0]["message"]["content"]
            except Exception as e:  # noqa: BLE001
                last_err = e
                time.sleep(1.5 * (attempt + 1))
        raise LLMError(f"OpenRouter (sync) call failed after {retries} tries: {last_err}")


async def chat(
    messages: list[dict[str, str]],
    *,
    model: str | None = None,
    temperature: float | None = None,
    max_tokens: int | None = None,
    retries: int = 3,
) -> str:
    """Send a chat completion request, returning the assistant message content."""
    message = await chat_message(
        messages,
        model=model,
        temperature=temperature,
        max_tokens=max_tokens,
        retries=retries,
    )
    return (message.get("content") or "").strip()


async def chat_message(
    messages: list[dict[str, Any]],
    *,
    tools: list[dict[str, Any]] | None = None,
    tool_choice: str | dict[str, Any] | None = None,
    model: str | None = None,
    temperature: float | None = None,
    max_tokens: int | None = None,
    retries: int = 3,
) -> dict[str, Any]:
    """Return one OpenAI-compatible assistant message, including tool calls.

    The existing :func:`chat` helper intentionally exposes text only.  Agent loops need
    the complete message so they can execute structured tool calls without parsing prose.
    """

    payload, headers, url = _payload_headers_url(
        messages, model, temperature, max_tokens, tools, tool_choice,
    )
    key, limit = _capacity_spec()
    async with capacity.async_slot(key, limit):
        last_err: Exception | None = None
        async with httpx.AsyncClient(
            timeout=settings.llm_timeout_s,
            trust_env=trust_environment_proxy(url),
        ) as client:
            for attempt in range(retries):
                try:
                    resp = await client.post(url, json=payload, headers=headers)
                    resp.raise_for_status()
                    data = resp.json()
                    message = data["choices"][0]["message"]
                    if not isinstance(message, dict):
                        raise LLMError("Model endpoint returned an invalid assistant message")
                    return message
                except Exception as e:  # noqa: BLE001
                    last_err = e
                    await asyncio.sleep(2 * (attempt + 1))
        raise LLMError(f"OpenRouter call failed after {retries} tries: {last_err}")


def _append_tool_call_delta(
    tool_calls: dict[int, dict[str, Any]],
    fragment: dict[str, Any],
    fallback_index: int,
) -> None:
    """Merge one OpenAI streaming ``tool_calls`` fragment by its stable index."""

    raw_index = fragment.get("index", fallback_index)
    try:
        index = int(raw_index)
    except (TypeError, ValueError):
        index = fallback_index

    call = tool_calls.setdefault(index, {
        "id": "",
        "type": "function",
        "function": {"name": "", "arguments": ""},
    })
    call_id = fragment.get("id")
    if isinstance(call_id, str):
        call["id"] += call_id
    call_type = fragment.get("type")
    if isinstance(call_type, str) and call_type:
        call["type"] = call_type

    function = fragment.get("function")
    if not isinstance(function, dict):
        return
    name = function.get("name")
    if isinstance(name, str):
        call["function"]["name"] += name
    arguments = function.get("arguments")
    if isinstance(arguments, str):
        call["function"]["arguments"] += arguments


def _completed_stream_message(
    *,
    role: str,
    content_parts: list[str],
    reasoning_parts: list[str],
    tool_calls: dict[int, dict[str, Any]],
) -> dict[str, Any]:
    content = "".join(content_parts)
    message: dict[str, Any] = {
        "role": role or "assistant",
        "content": content if content_parts or not tool_calls else None,
    }
    if tool_calls:
        message["tool_calls"] = [tool_calls[index] for index in sorted(tool_calls)]
    if reasoning_parts:
        # Kept only in the provider message so DeepSeek thinking-mode tool rounds can continue.
        # The agent runtime never emits this field as commentary or stores it in the transcript.
        message["reasoning_content"] = "".join(reasoning_parts)
    return message


async def chat_message_stream(
    messages: list[dict[str, Any]],
    *,
    tools: list[dict[str, Any]] | None = None,
    tool_choice: str | dict[str, Any] | None = None,
    model: str | None = None,
    temperature: float | None = None,
    max_tokens: int | None = None,
    retries: int = 3,
) -> AsyncIterator[dict[str, Any]]:
    """Stream one OpenAI-compatible assistant message.

    ``content_delta`` events are the provider's original text fragments. The terminal
    ``message`` event contains the reconstructed assistant message, including tool calls whose
    ids, names, and JSON arguments may have arrived across many chunks.

    A request may be retried only before the first upstream ``data:`` event. Once the provider
    has started a response, transparently replaying it could duplicate text already consumed by
    the caller, so any later transport or protocol failure is surfaced immediately.
    """

    payload, headers, url = _payload_headers_url(
        messages, model, temperature, max_tokens, tools, tool_choice,
    )
    payload = {**payload, "stream": True}
    headers = {**headers, "Accept": "text/event-stream"}
    attempts = max(1, retries)

    key, limit = _capacity_spec()
    async with capacity.async_slot(key, limit):
        last_err: Exception | None = None
        async with httpx.AsyncClient(
            timeout=settings.llm_timeout_s,
            trust_env=trust_environment_proxy(url),
        ) as client:
            for attempt in range(attempts):
                received_upstream_event = False
                role = "assistant"
                content_parts: list[str] = []
                reasoning_parts: list[str] = []
                tool_calls: dict[int, dict[str, Any]] = {}
                try:
                    async with client.stream(
                        "POST", url, json=payload, headers=headers,
                    ) as response:
                        response.raise_for_status()
                        async for line in response.aiter_lines():
                            if not line.startswith("data:"):
                                continue
                            raw = line[5:].strip()
                            if not raw:
                                continue
                            received_upstream_event = True
                            if raw == "[DONE]":
                                yield {
                                    "type": "message",
                                    "message": _completed_stream_message(
                                        role=role,
                                        content_parts=content_parts,
                                        reasoning_parts=reasoning_parts,
                                        tool_calls=tool_calls,
                                    ),
                                }
                                return

                            try:
                                chunk = json.loads(raw)
                            except json.JSONDecodeError as exc:
                                raise LLMError("Model endpoint returned malformed SSE JSON") from exc
                            if not isinstance(chunk, dict):
                                raise LLMError("Model endpoint returned an invalid SSE event")
                            if chunk.get("error"):
                                raise LLMError(f"Model endpoint stream error: {chunk['error']}")

                            choices = chunk.get("choices")
                            if not isinstance(choices, list) or not choices:
                                # Some providers send a usage-only chunk immediately before
                                # [DONE]. It is a valid upstream event but has no message delta.
                                continue
                            choice = next(
                                (
                                    item for item in choices
                                    if isinstance(item, dict) and item.get("index", 0) == 0
                                ),
                                choices[0],
                            )
                            if not isinstance(choice, dict):
                                raise LLMError("Model endpoint returned an invalid stream choice")
                            delta = choice.get("delta")
                            if not isinstance(delta, dict):
                                continue

                            delta_role = delta.get("role")
                            if isinstance(delta_role, str) and delta_role:
                                role = delta_role
                            content = delta.get("content")
                            if isinstance(content, str) and content:
                                content_parts.append(content)
                                yield {"type": "content_delta", "delta": content}
                            reasoning_content = delta.get("reasoning_content")
                            if isinstance(reasoning_content, str) and reasoning_content:
                                reasoning_parts.append(reasoning_content)

                            call_fragments = delta.get("tool_calls")
                            if isinstance(call_fragments, list):
                                for fallback_index, fragment in enumerate(call_fragments):
                                    if isinstance(fragment, dict):
                                        _append_tool_call_delta(
                                            tool_calls, fragment, fallback_index,
                                        )

                    raise LLMError("Model endpoint stream ended before data: [DONE]")
                except asyncio.CancelledError:
                    raise
                except Exception as exc:  # noqa: BLE001
                    last_err = exc
                    if received_upstream_event:
                        raise LLMError(
                            f"OpenRouter stream failed after output started: {exc}"
                        ) from exc
                    if attempt + 1 < attempts:
                        await asyncio.sleep(2 * (attempt + 1))

        raise LLMError(f"OpenRouter stream failed after {attempts} tries: {last_err}")


_JSON_FENCE = re.compile(r"```(?:json)?\s*(.*?)```", re.DOTALL)


def extract_json(text: str) -> Any:
    """Best-effort JSON parse of an LLM reply (handles ```json fences and stray prose)."""
    text = (text or "").strip()  # some providers return null content on length/tool-only turns
    # 1) fenced block
    m = _JSON_FENCE.search(text)
    if m:
        text = m.group(1).strip()
    # 2) direct parse
    try:
        return json.loads(text)
    except json.JSONDecodeError:
        pass
    # 3) slice from first { or [ to its matching last } or ]
    for opener, closer in (("{", "}"), ("[", "]")):
        start = text.find(opener)
        end = text.rfind(closer)
        if start != -1 and end != -1 and end > start:
            try:
                return json.loads(text[start : end + 1])
            except json.JSONDecodeError:
                continue
    raise LLMError(f"Could not parse JSON from LLM reply: {text[:300]}...")
