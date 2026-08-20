"""Durable, user-private memory for the web copilot.

The runtime writes complete event payloads here so a later turn can reuse exact MCP
observations.  Public serializers intentionally expose only bounded, redacted previews;
the database copy remains available to the trusted server-side agent.
"""
from __future__ import annotations

import hashlib
import json
import re
from datetime import datetime
from typing import Any, Literal

from sqlalchemy import func
from sqlmodel import Session, select

from app.db.models import (
    AgentConversation,
    AgentEvent,
    AgentTurn,
    AuditEvent,
    Conflict,
    Document,
    EntityResolution,
    ExtractionJob,
    KnowledgeSystem,
    OntologyRelease,
    TermProposal,
    ValidationDecision,
    utcnow,
)


_SECRET_KEY = re.compile(
    r"(?:^|_)(?:api_?key|authorization|bearer|cookie|password|secret|token|ciphertext)(?:$|_)",
    re.IGNORECASE,
)
_TITLE_LIMIT = 120
_FIRST_MESSAGE_PREVIEW = 240
_EVENT_PREVIEW = 1_200
_GENERIC_EVENT_LIMIT = 4_000


def _jsonable(value: Any) -> Any:
    """Return a JSON-column-safe copy without mutating a caller-owned payload."""

    return json.loads(json.dumps(value, ensure_ascii=False, default=str))


def _canonical_json(value: Any) -> str:
    return json.dumps(
        _jsonable(value), ensure_ascii=False, sort_keys=True, separators=(",", ":"),
    )


def tool_fingerprint(tool_name: str, arguments: dict[str, Any]) -> str:
    """Stable cache key for an exact tool + normalized-arguments request."""

    digest = hashlib.sha256()
    digest.update(tool_name.strip().encode("utf-8"))
    digest.update(b"\0")
    digest.update(_canonical_json(arguments).encode("utf-8"))
    return "sha256:" + digest.hexdigest()


def current_evidence_revision(session: Session, ks: KnowledgeSystem) -> str:
    """Content-address all live state that can change an Agent observation.

    Ontology mutations append an immutable ``AuditEvent``; the additional SQL signatures cover
    queue-only decisions, source processing, and releases. This intentionally stays separate
    from the expensive graph-content proposal revision: ontology writes still calculate that
    exact digest for optimistic concurrency, while ordinary chat turns remain fast.
    """

    audit_max = session.exec(
        select(func.max(AuditEvent.id)).where(AuditEvent.knowledge_system_id == ks.id)
    ).one()
    review_rows: list[Any] = []
    review_rows.extend(session.exec(
        select(Conflict.id, Conflict.status, Conflict.resolved_at, Conflict.resolution)
        .where(Conflict.knowledge_system_id == ks.id)
        .order_by(Conflict.id)
    ).all())
    review_rows.extend(session.exec(
        select(EntityResolution.id, EntityResolution.status, EntityResolution.resolved_at)
        .where(EntityResolution.knowledge_system_id == ks.id)
        .order_by(EntityResolution.id)
    ).all())
    review_rows.extend(session.exec(
        select(TermProposal.id, TermProposal.status, TermProposal.resolved_at)
        .where(TermProposal.knowledge_system_id == ks.id)
        .order_by(TermProposal.id)
    ).all())
    review_rows.extend(session.exec(
        select(ValidationDecision.id, ValidationDecision.action, ValidationDecision.created_at)
        .where(ValidationDecision.knowledge_system_id == ks.id)
        .order_by(ValidationDecision.id)
    ).all())
    source_rows = session.exec(
        select(Document.id, Document.parse_status, Document.chunk_count)
        .where(Document.knowledge_system_id == ks.id)
        .order_by(Document.id)
    ).all()
    job_rows = session.exec(
        select(
            ExtractionJob.id,
            ExtractionJob.status,
            ExtractionJob.phase,
            ExtractionJob.processed_chunks,
            ExtractionJob.finished_at,
        )
        .where(ExtractionJob.knowledge_system_id == ks.id)
        .order_by(ExtractionJob.id)
    ).all()
    release_rows = session.exec(
        select(OntologyRelease.id, OntologyRelease.status, OntologyRelease.version)
        .where(OntologyRelease.knowledge_system_id == ks.id)
        .order_by(OntologyRelease.id)
    ).all()
    state = {
        "knowledge_system": (
            ks.updated_at,
            ks.class_count,
            ks.property_count,
            ks.axiom_count,
        ),
        "audit_max": audit_max,
        "reviews": [tuple(row) for row in review_rows],
        "sources": [tuple(row) for row in source_rows],
        "jobs": [tuple(row) for row in job_rows],
        "releases": [tuple(row) for row in release_rows],
    }
    return "sha256:" + hashlib.sha256(_canonical_json(state).encode("utf-8")).hexdigest()


def _result_hash(data: dict[str, Any]) -> str:
    return "sha256:" + hashlib.sha256(_canonical_json(data).encode("utf-8")).hexdigest()


def _redact(value: Any, *, depth: int = 0) -> Any:
    if depth >= 12:
        return "[nested data omitted]"
    if isinstance(value, dict):
        return {
            str(key): "[redacted]" if _SECRET_KEY.search(str(key)) else _redact(item, depth=depth + 1)
            for key, item in value.items()
        }
    if isinstance(value, list):
        return [_redact(item, depth=depth + 1) for item in value]
    if isinstance(value, str) and len(value) > _GENERIC_EVENT_LIMIT:
        return value[:_GENERIC_EVENT_LIMIT] + "…"
    return value


def _bounded_payload(value: Any, limit: int) -> tuple[str, bool]:
    if isinstance(value, str):
        text = value
    else:
        text = json.dumps(_redact(_jsonable(value)), ensure_ascii=False, default=str)
    return (text[:limit] + ("…" if len(text) > limit else ""), len(text) > limit)


def owned_conversation(
    session: Session,
    *,
    conversation_id: int,
    user_id: int,
    knowledge_system_id: int,
) -> AgentConversation | None:
    """Resolve a conversation only when both private-owner and KS scopes match."""

    return session.exec(
        select(AgentConversation).where(
            AgentConversation.id == conversation_id,
            AgentConversation.user_id == user_id,
            AgentConversation.knowledge_system_id == knowledge_system_id,
            AgentConversation.deleted_at.is_(None),
        )
    ).first()


def create_conversation(
    session: Session,
    *,
    user_id: int,
    knowledge_system_id: int,
    title: str = "",
) -> AgentConversation:
    row = AgentConversation(
        user_id=user_id,
        knowledge_system_id=knowledge_system_id,
        title=title.strip()[:_TITLE_LIMIT],
    )
    session.add(row)
    session.commit()
    session.refresh(row)
    return row


def list_conversations(
    session: Session,
    *,
    user_id: int,
    knowledge_system_id: int,
) -> list[AgentConversation]:
    return list(session.exec(
        select(AgentConversation).where(
            AgentConversation.user_id == user_id,
            AgentConversation.knowledge_system_id == knowledge_system_id,
            AgentConversation.deleted_at.is_(None),
        ).order_by(AgentConversation.updated_at.desc(), AgentConversation.id.desc()).limit(100)
    ).all())


def list_deleted_conversations(
    session: Session,
    *,
    user_id: int,
    knowledge_system_id: int,
) -> list[AgentConversation]:
    """List recoverable tombstones for an explicit trash/recovery view."""
    return list(session.exec(
        select(AgentConversation).where(
            AgentConversation.user_id == user_id,
            AgentConversation.knowledge_system_id == knowledge_system_id,
            AgentConversation.deleted_at.is_not(None),
        ).order_by(AgentConversation.deleted_at.desc(), AgentConversation.id.desc()).limit(100)
    ).all())


def _turns(session: Session, conversation_id: int) -> list[AgentTurn]:
    return list(session.exec(
        select(AgentTurn).where(AgentTurn.conversation_id == conversation_id)
        .order_by(AgentTurn.id)
    ).all())


def _events(session: Session, turn_id: int) -> list[AgentEvent]:
    return list(session.exec(
        select(AgentEvent).where(AgentEvent.turn_id == turn_id)
        .order_by(AgentEvent.idx, AgentEvent.id)
    ).all())


def conversation_summary(session: Session, row: AgentConversation) -> dict[str, Any]:
    turns = _turns(session, row.id)
    first = next((turn.content for turn in turns if turn.role == "user" and turn.content), "")
    return {
        "id": row.id,
        "title": row.title,
        "first_user_message": first[:_FIRST_MESSAGE_PREVIEW],
        "turn_count": len(turns),
        "created_at": row.created_at,
        "updated_at": row.updated_at,
        "deleted_at": row.deleted_at,
        "deleted_by": row.deleted_by_name,
    }


def conversation_summaries(
    session: Session,
    rows: list[AgentConversation],
) -> list[dict[str, Any]]:
    """Serialize a conversation list with one batched turn lookup."""

    ids = [row.id for row in rows if row.id is not None]
    counts = {conversation_id: 0 for conversation_id in ids}
    first_messages = {conversation_id: "" for conversation_id in ids}
    if ids:
        turns = session.exec(
            select(AgentTurn).where(AgentTurn.conversation_id.in_(ids)).order_by(AgentTurn.id)
        ).all()
        for turn in turns:
            counts[turn.conversation_id] = counts.get(turn.conversation_id, 0) + 1
            if (
                turn.role == "user"
                and turn.content
                and not first_messages.get(turn.conversation_id)
            ):
                first_messages[turn.conversation_id] = turn.content[:_FIRST_MESSAGE_PREVIEW]
    return [
        {
            "id": row.id,
            "title": row.title,
            "first_user_message": first_messages.get(row.id, ""),
            "turn_count": counts.get(row.id, 0),
            "created_at": row.created_at,
            "updated_at": row.updated_at,
            "deleted_at": row.deleted_at,
            "deleted_by": row.deleted_by_name,
        }
        for row in rows
    ]


def safe_event(event: AgentEvent) -> dict[str, Any]:
    """Serialize an event without returning a complete stored MCP observation."""

    raw = _jsonable(event.data or {})
    if event.kind == "tool_result":
        result = raw.get("result", raw.get("content", raw)) if isinstance(raw, dict) else raw
        preview, truncated = _bounded_payload(result, _EVENT_PREVIEW)
        data: dict[str, Any] = {
            "preview": preview,
            "truncated": truncated,
        }
        if isinstance(raw, dict) and raw.get("summary") is not None:
            data["summary"] = str(raw["summary"])[:500]
        if isinstance(raw, dict) and raw.get("ok") is not None:
            data["ok"] = bool(raw["ok"])
    else:
        redacted = _redact(raw)
        encoded = json.dumps(redacted, ensure_ascii=False, default=str)
        if len(encoded) <= _GENERIC_EVENT_LIMIT:
            data = redacted if isinstance(redacted, dict) else {"value": redacted}
        else:
            data = {"preview": encoded[:_GENERIC_EVENT_LIMIT] + "…", "truncated": True}
    return {
        "id": event.id,
        "idx": event.idx,
        "kind": event.kind,
        "call_id": event.call_id,
        "tool_name": event.tool_name,
        "data": data,
        "fingerprint": event.fingerprint,
        "knowledge_revision": event.knowledge_revision,
        "result_hash": event.result_hash,
        "cached_from_event_id": event.cached_from_event_id,
        "created_at": event.created_at,
    }


def conversation_detail(session: Session, row: AgentConversation) -> dict[str, Any]:
    summary = conversation_summary(session, row)
    summary["turns"] = [
        {
            "id": turn.id,
            "role": turn.role,
            "content": turn.content,
            "status": turn.status,
            "trace": turn.trace,
            "proposal": turn.proposal,
            "error": turn.error,
            "knowledge_revision": turn.knowledge_revision,
            "model": turn.model,
            "created_at": turn.created_at,
            "updated_at": turn.updated_at,
            "events": [safe_event(event) for event in _events(session, turn.id)],
        }
        for turn in _turns(session, row.id)
    ]
    return summary


def rename_conversation(session: Session, row: AgentConversation, title: str) -> AgentConversation:
    row.title = title.strip()[:_TITLE_LIMIT]
    row.updated_at = utcnow()
    session.add(row)
    session.commit()
    session.refresh(row)
    return row


def delete_conversation(
    session: Session,
    row: AgentConversation,
    *,
    deleted_by_id: int | None = None,
    deleted_by_name: str = "",
    commit: bool = True,
) -> None:
    """Soft-delete a conversation while retaining its complete evidentiary history."""

    if row.deleted_at is None:
        row.deleted_at = utcnow()
        row.deleted_by_id = deleted_by_id
        row.deleted_by_name = deleted_by_name or None
        row.updated_at = row.deleted_at
        session.add(row)
    if commit:
        session.commit()
    else:
        session.flush()


def restore_conversation(session: Session, row: AgentConversation, *, commit: bool = True) -> None:
    """Restore a soft-deleted conversation and its retained turns/events."""
    row.deleted_at = None
    row.deleted_by_id = None
    row.deleted_by_name = None
    row.updated_at = utcnow()
    session.add(row)
    if commit:
        session.commit()
    else:
        session.flush()


def purge_conversation(session: Session, row: AgentConversation, *, commit: bool = True) -> None:
    """Physically remove a conversation for explicit parent/user teardown only."""

    turns = _turns(session, row.id)
    events_by_turn = {turn.id: _events(session, turn.id) for turn in turns}
    event_ids = {
        event.id
        for events in events_by_turn.values()
        for event in events
        if event.id is not None
    }
    # Cached results reference an earlier event. Break those self-FKs before deleting
    # either side; PostgreSQL checks the constraint immediately and cannot rely on the
    # ORM happening to emit event deletes in dependent-first order.
    for events in events_by_turn.values():
        for event in events:
            if event.cached_from_event_id in event_ids:
                event.cached_from_event_id = None
                session.add(event)
    session.flush()
    for turn in turns:
        for event in events_by_turn[turn.id]:
            session.delete(event)
    session.flush()
    for turn in turns:
        session.delete(turn)
    session.flush()
    session.delete(row)
    if commit:
        session.commit()
    else:
        session.flush()


def delete_scoped_conversations(
    session: Session,
    *,
    user_id: int | None = None,
    knowledge_system_id: int | None = None,
    commit: bool = False,
) -> int:
    """Delete every conversation matching one or both ownership scopes."""

    statement = select(AgentConversation)
    if user_id is not None:
        statement = statement.where(AgentConversation.user_id == user_id)
    if knowledge_system_id is not None:
        statement = statement.where(AgentConversation.knowledge_system_id == knowledge_system_id)
    rows = list(session.exec(statement).all())
    for row in rows:
        purge_conversation(session, row, commit=False)
    if commit:
        session.commit()
    return len(rows)


def start_turn_pair(
    session: Session,
    conversation: AgentConversation,
    user_content: str,
    *,
    knowledge_revision: str | None = None,
    model: str | None = None,
) -> tuple[AgentTurn, AgentTurn]:
    """Persist the user message and a running assistant placeholder atomically."""

    content = user_content.strip()
    if not content:
        raise ValueError("user_content is required")
    now = utcnow()
    user_turn = AgentTurn(
        conversation_id=conversation.id,
        role="user",
        content=content,
        status="done",
        knowledge_revision=knowledge_revision,
        created_at=now,
        updated_at=now,
    )
    assistant_turn = AgentTurn(
        conversation_id=conversation.id,
        role="assistant",
        status="running",
        knowledge_revision=knowledge_revision,
        model=model,
        created_at=now,
        updated_at=now,
    )
    if not conversation.title.strip():
        conversation.title = re.sub(r"\s+", " ", content).strip()[:_TITLE_LIMIT]
    conversation.updated_at = now
    session.add(conversation)
    session.add(user_turn)
    session.add(assistant_turn)
    session.commit()
    session.refresh(user_turn)
    session.refresh(assistant_turn)
    session.refresh(conversation)
    return user_turn, assistant_turn


def append_event(
    session: Session,
    turn: AgentTurn | int,
    *,
    kind: str,
    data: dict[str, Any],
    idx: int | None = None,
    call_id: str | None = None,
    tool_name: str | None = None,
    arguments: dict[str, Any] | None = None,
    fingerprint: str | None = None,
    knowledge_revision: str | None = None,
    cached_from_event_id: int | None = None,
    commit: bool = True,
) -> AgentEvent:
    """Append one ordered event while retaining the complete JSON-safe payload."""

    turn_row = session.get(AgentTurn, turn) if isinstance(turn, int) else turn
    if turn_row is None or turn_row.id is None:
        raise ValueError("turn not found")
    if idx is None:
        highest = session.exec(
            select(func.max(AgentEvent.idx)).where(AgentEvent.turn_id == turn_row.id)
        ).one()
        idx = int(highest) + 1 if highest is not None else 0
    stored = _jsonable(data)
    if fingerprint is None and tool_name and arguments is not None:
        fingerprint = tool_fingerprint(tool_name, arguments)
    event = AgentEvent(
        turn_id=turn_row.id,
        idx=idx,
        kind=kind,
        call_id=call_id,
        tool_name=tool_name,
        data=stored,
        fingerprint=fingerprint,
        knowledge_revision=knowledge_revision or turn_row.knowledge_revision,
        result_hash=_result_hash(stored) if kind == "tool_result" else None,
        cached_from_event_id=cached_from_event_id,
    )
    now = utcnow()
    turn_row.updated_at = now
    conversation = session.get(AgentConversation, turn_row.conversation_id)
    if conversation is not None:
        conversation.updated_at = now
        session.add(conversation)
    session.add(turn_row)
    session.add(event)
    if commit:
        session.commit()
        session.refresh(event)
    else:
        session.flush()
    return event


def finish_turn(
    session: Session,
    turn: AgentTurn | int,
    *,
    content: str,
    trace: list[dict[str, Any]] | None = None,
    proposal: dict[str, Any] | None = None,
    knowledge_revision: str | None = None,
) -> AgentTurn:
    turn_row = session.get(AgentTurn, turn) if isinstance(turn, int) else turn
    if turn_row is None:
        raise ValueError("turn not found")
    now = utcnow()
    turn_row.content = content
    turn_row.status = "done"
    turn_row.trace = _jsonable(trace or [])
    turn_row.proposal = _jsonable(proposal) if proposal is not None else None
    turn_row.error = None
    turn_row.knowledge_revision = knowledge_revision or turn_row.knowledge_revision
    turn_row.updated_at = now
    conversation = session.get(AgentConversation, turn_row.conversation_id)
    if conversation is not None:
        conversation.updated_at = now
        session.add(conversation)
    session.add(turn_row)
    session.commit()
    session.refresh(turn_row)
    return turn_row


def fail_turn(
    session: Session,
    turn: AgentTurn | int,
    *,
    error: str,
    content: str | None = None,
    status: Literal["failed", "cancelled"] = "failed",
) -> AgentTurn:
    turn_row = session.get(AgentTurn, turn) if isinstance(turn, int) else turn
    if turn_row is None:
        raise ValueError("turn not found")
    now = utcnow()
    if content is not None:
        turn_row.content = content
    turn_row.status = status
    turn_row.error = error[:4_000]
    turn_row.updated_at = now
    conversation = session.get(AgentConversation, turn_row.conversation_id)
    if conversation is not None:
        conversation.updated_at = now
        session.add(conversation)
    session.add(turn_row)
    session.commit()
    session.refresh(turn_row)
    return turn_row


def _tool_result_value(data: dict[str, Any]) -> Any:
    if "result" in data:
        return data["result"]
    if "content" in data:
        return data["content"]
    return data


def model_safe_tool_result(value: Any) -> Any:
    """Redact credential-shaped fields before persisted evidence re-enters a model prompt."""

    return _redact(_jsonable(value))


def _tool_result_text(data: dict[str, Any], max_chars: int) -> str:
    value = model_safe_tool_result(_tool_result_value(data))
    text = value if isinstance(value, str) else _canonical_json(value)
    if len(text) <= max_chars:
        return text
    return text[:max_chars] + "\n…[persisted tool result truncated for model context]"


def load_model_history(
    session: Session,
    *,
    conversation_id: int,
    user_id: int,
    knowledge_system_id: int,
    current_revision: str | None,
    before_turn_id: int | None = None,
    max_turns: int = 12,
    max_chars: int = 60_000,
    max_tool_result_chars: int = 24_000,
) -> list[dict[str, Any]]:
    """Rebuild recent model messages, including only revision-fresh tool evidence.

    Full observations stay persisted even when omitted or truncated from the prompt.  A
    missing ``current_revision`` is deliberately fail-closed: text history is returned,
    but no mutable workspace evidence is treated as current.
    """

    conversation = owned_conversation(
        session,
        conversation_id=conversation_id,
        user_id=user_id,
        knowledge_system_id=knowledge_system_id,
    )
    if conversation is None:
        return []
    statement = select(AgentTurn).where(
        AgentTurn.conversation_id == conversation.id,
        AgentTurn.status == "done",
    )
    if before_turn_id is not None:
        statement = statement.where(AgentTurn.id < before_turn_id)
    rows = list(session.exec(statement.order_by(AgentTurn.id.desc()).limit(max(1, max_turns))).all())
    rows.reverse()

    blocks: list[list[dict[str, Any]]] = []
    for turn in rows:
        block: list[dict[str, Any]] = []
        if turn.role == "user":
            if turn.content:
                block.append({"role": "user", "content": turn.content})
        elif turn.role == "assistant":
            events = _events(session, turn.id)
            results_by_call = {
                event.call_id: event
                for event in events
                if event.kind == "tool_result" and event.call_id
            }
            for call in (event for event in events if event.kind == "tool_call"):
                result = results_by_call.get(call.call_id)
                if (
                    result is None
                    or current_revision is None
                    or result.knowledge_revision != current_revision
                ):
                    continue
                call_data = call.data or {}
                arguments = call_data.get("arguments", call_data.get("args", {}))
                if not isinstance(arguments, dict):
                    arguments = {"value": arguments}
                name = call.tool_name or result.tool_name or str(call_data.get("tool") or "")
                if not name:
                    continue
                call_id = call.call_id or f"history-{call.id}"
                block.append({
                    "role": "assistant",
                    "content": None,
                    "tool_calls": [{
                        "id": call_id,
                        "type": "function",
                        "function": {
                            "name": name,
                            "arguments": _canonical_json(arguments),
                        },
                    }],
                })
                block.append({
                    "role": "tool",
                    "tool_call_id": call_id,
                    "name": name,
                    "content": _tool_result_text(result.data or {}, max_tool_result_chars),
                })
            if turn.content:
                block.append({"role": "assistant", "content": turn.content})
        if block:
            blocks.append(block)

    def block_size(block: list[dict[str, Any]]) -> int:
        return len(json.dumps(block, ensure_ascii=False, default=str))

    total = sum(block_size(block) for block in blocks)
    while len(blocks) > 1 and total > max_chars:
        total -= block_size(blocks.pop(0))
    # A bounded window can otherwise begin with an orphan assistant/tool sequence. Keep the
    # OpenAI history structurally valid by starting at the next user turn.
    while len(blocks) > 1 and blocks[0][0].get("role") != "user":
        blocks.pop(0)
    return [message for block in blocks for message in block]


def load_fresh_observations(
    session: Session,
    *,
    conversation_id: int,
    user_id: int,
    knowledge_system_id: int,
    current_revision: str,
    before_turn_id: int | None = None,
    max_turns: int = 16,
    max_observations: int = 64,
) -> list[dict[str, Any]]:
    """Restore exact revision-fresh tool evidence for runtime grounding checks.

    ``load_model_history`` deliberately bounds large tool bodies for the provider context.
    Runtime validators still need the complete structured observation, otherwise they can
    incorrectly decide that the history is ungrounded and request the same tool again.  This
    server-only helper reads the complete stored result, applies the credential redactor, and
    never exposes it through the conversation API.
    """

    conversation = owned_conversation(
        session,
        conversation_id=conversation_id,
        user_id=user_id,
        knowledge_system_id=knowledge_system_id,
    )
    if conversation is None:
        return []
    statement = select(AgentTurn).where(
        AgentTurn.conversation_id == conversation.id,
        AgentTurn.role == "assistant",
        AgentTurn.status == "done",
    )
    if before_turn_id is not None:
        statement = statement.where(AgentTurn.id < before_turn_id)
    turns = list(session.exec(
        statement.order_by(AgentTurn.id.desc()).limit(max(1, max_turns))
    ).all())
    turns.reverse()

    observations: list[dict[str, Any]] = []
    for turn in turns:
        events = _events(session, turn.id)
        calls = {
            event.call_id: event
            for event in events
            if event.kind == "tool_call" and event.call_id
        }
        for result in (event for event in events if event.kind == "tool_result"):
            if (
                not result.call_id
                or result.knowledge_revision != current_revision
                or result.call_id not in calls
            ):
                continue
            data = result.data or {}
            if not isinstance(data, dict) or data.get("ok") is False or "error" in data:
                continue
            value = _tool_result_value(data)
            if isinstance(value, dict) and value.get("error") is not None:
                continue
            call = calls[result.call_id]
            call_data = call.data or {}
            arguments = call_data.get("arguments", call_data.get("args", {}))
            if not isinstance(arguments, dict):
                arguments = {"value": arguments}
            tool_name = call.tool_name or result.tool_name
            if not tool_name:
                continue
            observations.append({
                "tool": tool_name,
                "arguments": _jsonable(arguments),
                "result": model_safe_tool_result(value),
                "persisted": True,
                "source_event_id": result.id,
            })
    return observations[-max(1, max_observations):]


def find_cached_tool_result(
    session: Session,
    *,
    conversation_id: int,
    user_id: int,
    knowledge_system_id: int,
    tool_name: str,
    arguments: dict[str, Any],
    current_revision: str,
    exclude_turn_id: int | None = None,
) -> AgentEvent | None:
    """Find the newest exact, successful result still valid for this ontology revision."""

    if owned_conversation(
        session,
        conversation_id=conversation_id,
        user_id=user_id,
        knowledge_system_id=knowledge_system_id,
    ) is None:
        return None
    statement = (
        select(AgentEvent)
        .join(AgentTurn, AgentTurn.id == AgentEvent.turn_id)
        .where(
            AgentTurn.conversation_id == conversation_id,
            AgentEvent.kind == "tool_result",
            AgentEvent.tool_name == tool_name,
            AgentEvent.fingerprint == tool_fingerprint(tool_name, arguments),
            AgentEvent.knowledge_revision == current_revision,
        )
        .order_by(AgentEvent.id.desc())
    )
    if exclude_turn_id is not None:
        statement = statement.where(AgentEvent.turn_id != exclude_turn_id)
    for event in session.exec(statement).all():
        data = event.data or {}
        if not isinstance(data, dict) or data.get("ok") is False or "error" in data:
            continue
        result = _tool_result_value(data)
        if isinstance(result, dict) and result.get("error") is not None:
            continue
        return event
    return None
