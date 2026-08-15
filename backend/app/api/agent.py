"""Authenticated web endpoint for the MCP-backed knowledge copilot."""
from __future__ import annotations

import asyncio
import json
import logging
import re
import time
from collections.abc import AsyncIterator
from typing import Any, Literal

from fastapi import APIRouter, Depends, HTTPException, Response, status
from fastapi.responses import StreamingResponse
from pydantic import BaseModel, Field
from sqlmodel import Session, select

from app import agent_memory, agent_runtime, model_config, prompt_config
from app.db.database import get_session
from app.db.models import AgentTurn, KnowledgeSystem, User
from app.permissions import ks_reader
from app.security import current_user


router = APIRouter(prefix="/api/knowledge", tags=["agent"])
logger = logging.getLogger(__name__)


class AgentMessage(BaseModel):
    role: Literal["user", "assistant"]
    content: str = Field(min_length=1, max_length=20_000)


class AgentRequest(BaseModel):
    """Compatibility shape used by pre-conversation API clients."""

    messages: list[AgentMessage] = Field(min_length=1, max_length=30)


class AgentConversationRequest(BaseModel):
    """Current-message-only shape; trusted history is rebuilt from server storage."""

    message: str = Field(min_length=1, max_length=20_000)
    conversation_id: int | None = Field(default=None, gt=0)


AgentRequestBody = AgentRequest | AgentConversationRequest


class ConversationCreate(BaseModel):
    title: str = Field(default="", max_length=120)


class ConversationRename(BaseModel):
    title: str = Field(min_length=1, max_length=120)


def _sse_frame(event: dict[str, Any]) -> str:
    event_type = str(event.get("type") or "progress")
    return (
        f"event: {event_type}\n"
        f"data: {json.dumps(event, ensure_ascii=False, separators=(',', ':'), default=str)}\n\n"
    )


def _legacy_messages(body: AgentRequest) -> list[dict[str, str]]:
    messages = [message.model_dump() for message in body.messages]
    if not messages or messages[-1]["role"] != "user":
        raise HTTPException(status_code=422, detail="The last message must be from the user")
    return messages


def _current_message(body: AgentRequestBody) -> str | None:
    if not isinstance(body, AgentConversationRequest):
        return None
    content = body.message.strip()
    if not content:
        raise HTTPException(status_code=422, detail="Agent message cannot be blank")
    return content


def _conversation_or_404(
    conversation_id: int,
    *,
    ks: KnowledgeSystem,
    user: User,
    session: Session,
):
    conversation = agent_memory.owned_conversation(
        session,
        conversation_id=conversation_id,
        user_id=user.id,
        knowledge_system_id=ks.id,
    )
    if conversation is None:
        # Deliberately hide whether another user or KS owns this numeric id.
        raise HTTPException(status_code=404, detail="Agent conversation not found")
    return conversation


def _start_persisted_turn(
    session: Session,
    *,
    ks: KnowledgeSystem,
    user: User,
    message: str,
    conversation_id: int | None,
    model: str | None,
) -> tuple[
    Any,
    AgentTurn,
    AgentTurn,
    str,
    list[dict[str, str]],
    list[dict[str, Any]],
    list[dict[str, Any]],
]:
    if conversation_id is None:
        conversation_row = agent_memory.create_conversation(
            session,
            user_id=user.id,
            knowledge_system_id=ks.id,
        )
    else:
        conversation_row = _conversation_or_404(
            conversation_id,
            ks=ks,
            user=user,
            session=session,
        )
    evidence_revision = agent_memory.current_evidence_revision(session, ks)
    user_turn, assistant_turn = agent_memory.start_turn_pair(
        session,
        conversation_row,
        message,
        knowledge_revision=evidence_revision,
        model=model,
    )
    history_rows = list(session.exec(
        select(AgentTurn).where(
            AgentTurn.conversation_id == conversation_row.id,
            AgentTurn.id <= user_turn.id,
            AgentTurn.status == "done",
            AgentTurn.role.in_(("user", "assistant")),
        ).order_by(AgentTurn.id.desc()).limit(12)
    ).all())
    history_rows.reverse()
    transcript = [
        {"role": turn.role, "content": turn.content}
        for turn in history_rows
        if turn.content
    ]
    context_messages = agent_memory.load_model_history(
        session,
        conversation_id=conversation_row.id,
        user_id=user.id,
        knowledge_system_id=ks.id,
        current_revision=evidence_revision,
        before_turn_id=assistant_turn.id,
        max_turns=16,
    )
    context_observations = agent_memory.load_fresh_observations(
        session,
        conversation_id=conversation_row.id,
        user_id=user.id,
        knowledge_system_id=ks.id,
        current_revision=evidence_revision,
        before_turn_id=assistant_turn.id,
        max_turns=16,
    )
    return (
        conversation_row,
        user_turn,
        assistant_turn,
        evidence_revision,
        transcript,
        context_messages,
        context_observations,
    )


def _memory_callbacks(
    session: Session,
    *,
    conversation_id: int,
    assistant_turn: AgentTurn,
    ks: KnowledgeSystem,
    user: User,
    evidence_revision: str,
    force_refresh: bool,
):
    async def lookup(tool_name: str, arguments: dict[str, Any]) -> dict[str, Any] | None:
        if force_refresh:
            return None
        event = agent_memory.find_cached_tool_result(
            session,
            conversation_id=conversation_id,
            user_id=user.id,
            knowledge_system_id=ks.id,
            tool_name=tool_name,
            arguments=arguments,
            current_revision=evidence_revision,
            exclude_turn_id=assistant_turn.id,
        )
        if event is None:
            return None
        data = event.data or {}
        if not isinstance(data, dict) or "result" not in data:
            return None
        return {
            "result": agent_memory.model_safe_tool_result(data["result"]),
            "event_id": event.id,
        }

    async def record(item: dict[str, Any]) -> None:
        kind = str(item.get("kind") or "tool_result")
        tool_name = str(item.get("tool") or "") or None
        arguments = item.get("arguments")
        if not isinstance(arguments, dict):
            arguments = {}
        if kind == "tool_call":
            data = {"arguments": arguments}
        else:
            ok = "error" not in item
            data = {
                "ok": ok,
                "cached": bool(item.get("cached")),
            }
            if ok:
                data["result"] = item.get("result")
            else:
                data["error"] = item.get("error")
        agent_memory.append_event(
            session,
            assistant_turn,
            kind=kind,
            data=data,
            call_id=str(item.get("call_id") or "") or None,
            tool_name=tool_name,
            arguments=arguments,
            knowledge_revision=evidence_revision,
            cached_from_event_id=(
                int(item["cached_from_event_id"])
                if item.get("cached_from_event_id") is not None
                else None
            ),
        )

    return lookup, record


def _force_evidence_refresh(message: str) -> bool:
    normalized = message.casefold()
    # A negated refresh request is common in follow-up questions (for example,
    # "不要重新读取，直接根据刚才的证据回答").  Treating the marker alone as an
    # instruction to refresh defeats conversation memory and causes needless tool calls.
    normalized = re.sub(
        r"(?:不要|无需|无须|不用|不必|不需要|别|请勿)(?:再|去|进行|帮我)?"
        r"(?:重新检查|重新读取|刷新|重查|检查最新|读取最新|获取最新)",
        "",
        normalized,
    )
    normalized = re.sub(
        r"(?:do\s+not|don't|dont|no\s+need\s+to|without)\s+"
        r"(?:refresh(?:ing)?|check(?:ing)?\s+again|re-?check(?:ing)?|"
        r"re-?read(?:ing)?|read(?:ing)?\s+again|fetch(?:ing)?\s+the\s+latest)",
        "",
        normalized,
    )
    return any(marker in normalized for marker in (
        "重新检查", "重新读取", "刷新", "最新", "现在再查", "重查",
        "refresh", "check again", "re-check", "re-read", "reread", "latest", "right now",
    ))


@router.get("/{ks_id}/agent/conversations")
def list_conversations(
    ks: KnowledgeSystem = Depends(ks_reader),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict[str, Any]:
    rows = agent_memory.list_conversations(
        session,
        user_id=user.id,
        knowledge_system_id=ks.id,
    )
    return {"conversations": agent_memory.conversation_summaries(session, rows)}


@router.post("/{ks_id}/agent/conversations", status_code=status.HTTP_201_CREATED)
def create_conversation(
    body: ConversationCreate,
    ks: KnowledgeSystem = Depends(ks_reader),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict[str, Any]:
    row = agent_memory.create_conversation(
        session,
        user_id=user.id,
        knowledge_system_id=ks.id,
        title=body.title,
    )
    return agent_memory.conversation_summary(session, row)


@router.get("/{ks_id}/agent/conversations/{conversation_id}")
def get_conversation(
    conversation_id: int,
    ks: KnowledgeSystem = Depends(ks_reader),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict[str, Any]:
    row = _conversation_or_404(
        conversation_id,
        ks=ks,
        user=user,
        session=session,
    )
    return agent_memory.conversation_detail(session, row)


@router.patch("/{ks_id}/agent/conversations/{conversation_id}")
def rename_conversation(
    conversation_id: int,
    body: ConversationRename,
    ks: KnowledgeSystem = Depends(ks_reader),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict[str, Any]:
    row = _conversation_or_404(
        conversation_id,
        ks=ks,
        user=user,
        session=session,
    )
    title = body.title.strip()
    if not title:
        raise HTTPException(status_code=422, detail="Conversation title cannot be blank")
    row = agent_memory.rename_conversation(session, row, title)
    return agent_memory.conversation_summary(session, row)


@router.delete(
    "/{ks_id}/agent/conversations/{conversation_id}",
    status_code=status.HTTP_204_NO_CONTENT,
)
def delete_conversation(
    conversation_id: int,
    ks: KnowledgeSystem = Depends(ks_reader),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> Response:
    row = _conversation_or_404(
        conversation_id,
        ks=ks,
        user=user,
        session=session,
    )
    agent_memory.delete_conversation(session, row)
    return Response(status_code=status.HTTP_204_NO_CONTENT)


@router.post("/{ks_id}/agent/chat", responses={502: {"description": "Model or MCP agent failure"}})
async def chat(
    body: AgentRequestBody,
    ks: KnowledgeSystem = Depends(ks_reader),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict:
    current_message = _current_message(body)
    legacy_conversation = (
        None if current_message is not None else _legacy_messages(body)
    )
    try:
        with (
            model_config.use_ks_connections(session, ks),
            prompt_config.use_ks_prompts(session, ks.id),
        ):
            if current_message is None:
                return await agent_runtime.run(
                    session=session,
                    user=user,
                    ks=ks,
                    conversation=legacy_conversation or [],
                )
            (
                conversation_row,
                _user_turn,
                assistant_turn,
                evidence_revision,
                transcript,
                context_messages,
                context_observations,
            ) = _start_persisted_turn(
                session,
                ks=ks,
                user=user,
                message=current_message,
                conversation_id=(
                    body.conversation_id if isinstance(body, AgentConversationRequest) else None
                ),
                model=model_config.llm_conn()[2],
            )
            force_refresh = _force_evidence_refresh(current_message)
            evidence_lookup, evidence_sink = _memory_callbacks(
                session,
                conversation_id=conversation_row.id,
                assistant_turn=assistant_turn,
                ks=ks,
                user=user,
                evidence_revision=evidence_revision,
                force_refresh=force_refresh,
            )
            try:
                result = await agent_runtime.run(
                    session=session,
                    user=user,
                    ks=ks,
                    conversation=transcript,
                    context_messages=context_messages,
                    context_observations=[] if force_refresh else context_observations,
                    evidence_lookup=evidence_lookup,
                    evidence_sink=evidence_sink,
                )
            except Exception as exc:
                agent_memory.fail_turn(session, assistant_turn, error=str(exc))
                raise
            agent_memory.finish_turn(
                session,
                assistant_turn,
                content=result["answer"],
                trace=result["trace"],
                proposal=result["proposal"],
                knowledge_revision=evidence_revision,
            )
            return {
                **result,
                "conversation": agent_memory.conversation_summary(session, conversation_row),
            }
    except HTTPException:
        raise
    except agent_runtime.AgentError as exc:
        logger.warning("Agent request failed for knowledge system %s: %s", ks.id, exc)
        raise HTTPException(status_code=502, detail=f"Agent failed: {exc}") from exc
    except Exception as exc:
        logger.exception("Unexpected agent failure for knowledge system %s", ks.id)
        raise HTTPException(
            status_code=502,
            detail="Agent service is temporarily unavailable. Check the model endpoint and try again.",
        ) from exc


@router.post(
    "/{ks_id}/agent/chat/stream",
    response_class=StreamingResponse,
    responses={
        200: {
            "description": (
                "SSE stream of turn_started, progress, commentary, trace, answer_reset, delta, proposal, "
                "done, or error events"
            ),
            "content": {"text/event-stream": {}},
        },
    },
)
async def chat_stream(
    body: AgentRequestBody,
    ks: KnowledgeSystem = Depends(ks_reader),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> StreamingResponse:
    """Stream one turn, persisting new conversation-backed requests as they happen."""

    current_message = _current_message(body)
    legacy_conversation = (
        None if current_message is not None else _legacy_messages(body)
    )
    requested_conversation_id = (
        body.conversation_id if isinstance(body, AgentConversationRequest) else None
    )
    if current_message is not None and requested_conversation_id is not None:
        # Resolve before returning StreamingResponse so an invalid/private id is a real 404,
        # rather than a successful SSE response containing a late error frame.
        _conversation_or_404(
            requested_conversation_id,
            ks=ks,
            user=user,
            session=session,
        )

    async def events() -> AsyncIterator[str]:
        if current_message is None:
            try:
                with (
                    model_config.use_ks_connections(session, ks),
                    prompt_config.use_ks_prompts(session, ks.id),
                ):
                    async for event in agent_runtime.stream(
                        session=session,
                        user=user,
                        ks=ks,
                        conversation=legacy_conversation or [],
                    ):
                        yield _sse_frame(event)
            except asyncio.CancelledError:
                raise
            except agent_runtime.AgentError as exc:
                logger.warning("Agent stream failed for knowledge system %s: %s", ks.id, exc)
                yield _sse_frame({
                    "type": "error",
                    "code": "agent_error",
                    "message": f"Agent failed: {exc}",
                })
            except Exception:
                logger.exception("Unexpected agent stream failure for knowledge system %s", ks.id)
                yield _sse_frame({
                    "type": "error",
                    "code": "service_unavailable",
                    "message": (
                        "Agent service is temporarily unavailable. "
                        "Check the model endpoint and try again."
                    ),
                })
            return

        conversation_row = None
        assistant_turn: AgentTurn | None = None
        accumulated_answer = ""
        trace: list[dict[str, Any]] = []
        proposal: dict[str, Any] | None = None
        pending_tokens = ""
        last_token_flush = time.monotonic()

        def flush_tokens() -> None:
            nonlocal pending_tokens, last_token_flush
            if not pending_tokens or assistant_turn is None:
                return
            agent_memory.append_event(
                session,
                assistant_turn,
                kind="token",
                data={"delta": pending_tokens},
            )
            pending_tokens = ""
            last_token_flush = time.monotonic()

        try:
            with (
                model_config.use_ks_connections(session, ks),
                prompt_config.use_ks_prompts(session, ks.id),
            ):
                (
                    conversation_row,
                    user_turn,
                    assistant_turn,
                    evidence_revision,
                    transcript,
                    context_messages,
                    context_observations,
                ) = _start_persisted_turn(
                    session,
                    ks=ks,
                    user=user,
                    message=current_message,
                    conversation_id=requested_conversation_id,
                    model=model_config.llm_conn()[2],
                )
                started = {
                    "type": "turn_started",
                    "conversation_id": conversation_row.id,
                    "conversation": agent_memory.conversation_summary(session, conversation_row),
                    "user_turn_id": user_turn.id,
                    "assistant_turn_id": assistant_turn.id,
                }
                agent_memory.append_event(
                    session,
                    assistant_turn,
                    kind="turn_started",
                    data={
                        "conversation_id": conversation_row.id,
                        "user_turn_id": user_turn.id,
                        "assistant_turn_id": assistant_turn.id,
                    },
                )
                yield _sse_frame(started)

                force_refresh = _force_evidence_refresh(current_message)
                evidence_lookup, evidence_sink = _memory_callbacks(
                    session,
                    conversation_id=conversation_row.id,
                    assistant_turn=assistant_turn,
                    ks=ks,
                    user=user,
                    evidence_revision=evidence_revision,
                    force_refresh=force_refresh,
                )
                async for event in agent_runtime.stream(
                    session=session,
                    user=user,
                    ks=ks,
                    conversation=transcript,
                    context_messages=context_messages,
                    context_observations=[] if force_refresh else context_observations,
                    native_tokens=True,
                    evidence_lookup=evidence_lookup,
                    evidence_sink=evidence_sink,
                ):
                    event_type = event.get("type")
                    if event_type == "delta":
                        delta = str(event.get("delta") or "")
                        accumulated_answer += delta
                        pending_tokens += delta
                        if len(pending_tokens) >= 1_024 or time.monotonic() - last_token_flush >= 0.25:
                            flush_tokens()
                    else:
                        flush_tokens()
                        if event_type == "answer_reset":
                            accumulated_answer = ""
                            agent_memory.append_event(
                                session,
                                assistant_turn,
                                kind="answer_reset",
                                data={},
                            )
                        elif event_type == "trace" and isinstance(event.get("trace"), dict):
                            trace.append(event["trace"])
                            agent_memory.append_event(
                                session,
                                assistant_turn,
                                kind="trace",
                                data={"trace": event["trace"]},
                            )
                        elif event_type == "commentary":
                            commentary = str(event.get("text") or "").strip()[:500]
                            if commentary:
                                agent_memory.append_event(
                                    session,
                                    assistant_turn,
                                    kind="commentary",
                                    data={"text": commentary},
                                )
                        elif event_type == "proposal":
                            proposal = event.get("proposal")
                            agent_memory.append_event(
                                session,
                                assistant_turn,
                                kind="proposal",
                                data={"proposal": proposal},
                            )
                        elif event_type == "done":
                            # Native deltas are the authoritative transcript. A terminal answer
                            # is only a compatibility fallback for non-streaming producers; it
                            # must never overwrite a longer response already reconstructed from
                            # the stream.
                            final_answer = str(accumulated_answer or event.get("answer") or "")
                            final_trace = event.get("trace") or trace
                            final_proposal = event.get("proposal") or proposal
                            agent_memory.append_event(
                                session,
                                assistant_turn,
                                kind="done",
                                data={},
                                commit=False,
                            )
                            agent_memory.finish_turn(
                                session,
                                assistant_turn,
                                content=final_answer,
                                trace=final_trace,
                                proposal=final_proposal,
                                knowledge_revision=evidence_revision,
                            )
                            event = {
                                **event,
                                "conversation_id": conversation_row.id,
                                "conversation": agent_memory.conversation_summary(
                                    session, conversation_row,
                                ),
                            }
                    yield _sse_frame(event)
        except asyncio.CancelledError:
            flush_tokens()
            if assistant_turn is not None:
                agent_memory.append_event(
                    session,
                    assistant_turn,
                    kind="error",
                    data={"status": "cancelled", "message": "Client disconnected"},
                    commit=False,
                )
                agent_memory.fail_turn(
                    session,
                    assistant_turn,
                    error="Client disconnected",
                    content=accumulated_answer,
                    status="cancelled",
                )
            raise
        except agent_runtime.AgentError as exc:
            flush_tokens()
            if assistant_turn is not None:
                agent_memory.append_event(
                    session,
                    assistant_turn,
                    kind="error",
                    data={"status": "failed", "message": str(exc)},
                    commit=False,
                )
                agent_memory.fail_turn(
                    session,
                    assistant_turn,
                    error=str(exc),
                    content=accumulated_answer,
                )
            logger.warning("Agent stream failed for knowledge system %s: %s", ks.id, exc)
            yield _sse_frame({
                "type": "error",
                "code": "agent_error",
                "message": f"Agent failed: {exc}",
            })
        except Exception as exc:
            flush_tokens()
            if assistant_turn is not None:
                agent_memory.append_event(
                    session,
                    assistant_turn,
                    kind="error",
                    data={"status": "failed", "message": str(exc)[:4_000]},
                    commit=False,
                )
                agent_memory.fail_turn(
                    session,
                    assistant_turn,
                    error=str(exc),
                    content=accumulated_answer,
                )
            logger.exception("Unexpected agent stream failure for knowledge system %s", ks.id)
            yield _sse_frame({
                "type": "error",
                "code": "service_unavailable",
                "message": (
                    "Agent service is temporarily unavailable. "
                    "Check the model endpoint and try again."
                ),
            })

    return StreamingResponse(
        events(),
        media_type="text/event-stream",
        headers={
            "Cache-Control": "no-cache, no-transform",
            "X-Accel-Buffering": "no",
        },
    )
