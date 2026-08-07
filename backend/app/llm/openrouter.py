"""Minimal async OpenRouter chat client.

Mirrors the calling convention used in the ``llm_coupling`` project (Bearer key from
env, ``/chat/completions`` endpoint) but uses httpx with retries. Defaults to a cheap
DeepSeek model — never an expensive Claude model.
"""
from __future__ import annotations

import asyncio
import json
import re
import time
from typing import Any

import httpx

from app.config import settings


class LLMError(RuntimeError):
    pass


def _payload_headers_url(messages, model, temperature, max_tokens):
    from app import model_config  # effective (base_url, key, model) for the current unit of work

    base_url, key, conn_model = model_config.llm_conn()
    if not key:
        raise LLMError("No LLM API key set; add a model entry in Settings or backend/.env")
    payload: dict[str, Any] = {
        "model": model or conn_model or model_config.system_extract_model(),
        "messages": messages,
        "temperature": settings.llm_temperature if temperature is None else temperature,
        "max_tokens": max_tokens or settings.llm_max_tokens,
    }
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
    last_err: Exception | None = None
    for attempt in range(retries):
        try:
            resp = httpx.post(url, json=payload, headers=headers, timeout=settings.llm_timeout_s)
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
    payload, headers, url = _payload_headers_url(messages, model, temperature, max_tokens)

    last_err: Exception | None = None
    async with httpx.AsyncClient(timeout=settings.llm_timeout_s) as client:
        for attempt in range(retries):
            try:
                resp = await client.post(url, json=payload, headers=headers)
                resp.raise_for_status()
                data = resp.json()
                return data["choices"][0]["message"]["content"]
            except Exception as e:  # noqa: BLE001
                last_err = e
                await asyncio.sleep(2 * (attempt + 1))
    raise LLMError(f"OpenRouter call failed after {retries} tries: {last_err}")


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
