"""Per-knowledge-system prompt catalog, overrides, restore, and audit history."""
from __future__ import annotations

from datetime import datetime

from fastapi import APIRouter, Depends, HTTPException, Response, status
from pydantic import BaseModel
from sqlmodel import Session, select

from app import audit, prompt_config
from app.db.database import get_session
from app.db.models import KnowledgePromptOverride, KnowledgeSystem, User, utcnow
from app.permissions import ks_reader, ks_writer
from app.security import current_user

router = APIRouter(prefix="/api/knowledge", tags=["prompts"])


class PromptUpdateIn(BaseModel):
    content: str


class PromptOut(BaseModel):
    key: str
    category: str
    title: str
    description: str
    default_content: str
    effective_content: str
    variables: list[str]
    is_overridden: bool
    updated_at: datetime | None = None
    updated_by: str | None = None


class PromptListOut(BaseModel):
    items: list[PromptOut]
    total_overrides: int


def _overrides(session: Session, ks_id: int) -> dict[str, KnowledgePromptOverride]:
    rows = session.exec(
        select(KnowledgePromptOverride).where(
            KnowledgePromptOverride.knowledge_system_id == ks_id,
        )
    ).all()
    return {row.prompt_key: row for row in rows}


def _prompt_out(
    item: prompt_config.PromptDefinition,
    override: KnowledgePromptOverride | None,
) -> PromptOut:
    return PromptOut(
        key=item.key,
        category=item.category,
        title=item.title,
        description=item.description,
        default_content=item.default,
        effective_content=override.content if override else item.default,
        variables=list(item.variables),
        is_overridden=override is not None,
        updated_at=override.updated_at if override else None,
        updated_by=(override.updated_by_name or None) if override else None,
    )


def _require_prompt(prompt_key: str) -> prompt_config.PromptDefinition:
    item = prompt_config.definition(prompt_key)
    if item is None:
        raise HTTPException(status_code=404, detail="Unknown prompt")
    return item


@router.get("/{ks_id}/prompts", response_model=PromptListOut)
def list_prompts(
    ks: KnowledgeSystem = Depends(ks_reader),
    session: Session = Depends(get_session),
) -> PromptListOut:
    current = _overrides(session, ks.id)
    return PromptListOut(
        items=[_prompt_out(item, current.get(item.key)) for item in prompt_config.definitions()],
        total_overrides=sum(1 for key in current if prompt_config.definition(key) is not None),
    )


@router.put("/{ks_id}/prompts/{prompt_key:path}", response_model=PromptOut)
def update_prompt(
    prompt_key: str,
    body: PromptUpdateIn,
    ks: KnowledgeSystem = Depends(ks_writer),
    actor: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> PromptOut:
    item = _require_prompt(prompt_key)
    try:
        prompt_config.validate_content(item, body.content)
    except ValueError as exc:
        raise HTTPException(status_code=422, detail=str(exc)) from exc

    override = session.exec(
        select(KnowledgePromptOverride).where(
            KnowledgePromptOverride.knowledge_system_id == ks.id,
            KnowledgePromptOverride.prompt_key == item.key,
        )
    ).first()
    before = override.content if override else item.default
    if body.content == before:
        return _prompt_out(item, override)

    now = utcnow()
    if override is None:
        override = KnowledgePromptOverride(
            knowledge_system_id=ks.id,
            prompt_key=item.key,
            content=body.content,
            updated_by_id=actor.id,
            updated_by_name=actor.username,
            created_at=now,
            updated_at=now,
        )
    else:
        override.content = body.content
        override.updated_by_id = actor.id
        override.updated_by_name = actor.username
        override.updated_at = now
    session.add(override)
    audit.record(
        session,
        ks_id=ks.id,
        action="prompt.update",
        summary=f'Updated prompt "{item.title}"',
        actor_id=actor.id,
        actor_name=actor.username,
        detail={
            "prompt_key": item.key,
            "before": before,
            "after": body.content,
            "previously_overridden": before != item.default,
        },
    )
    return _prompt_out(item, override)


@router.delete("/{ks_id}/prompts/{prompt_key:path}", response_model=PromptOut)
def restore_prompt(
    prompt_key: str,
    ks: KnowledgeSystem = Depends(ks_writer),
    actor: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> PromptOut:
    item = _require_prompt(prompt_key)
    override = session.exec(
        select(KnowledgePromptOverride).where(
            KnowledgePromptOverride.knowledge_system_id == ks.id,
            KnowledgePromptOverride.prompt_key == item.key,
        )
    ).first()
    if override is None:
        return _prompt_out(item, None)
    before = override.content
    session.delete(override)
    audit.record(
        session,
        ks_id=ks.id,
        action="prompt.restore",
        summary=f'Restored prompt "{item.title}" to default',
        actor_id=actor.id,
        actor_name=actor.username,
        detail={"prompt_key": item.key, "before": before, "after": item.default},
    )
    return _prompt_out(item, None)


@router.post("/{ks_id}/prompts/restore-all", status_code=status.HTTP_204_NO_CONTENT)
def restore_all_prompts(
    ks: KnowledgeSystem = Depends(ks_writer),
    actor: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> Response:
    current = _overrides(session, ks.id)
    changes = []
    for item in prompt_config.definitions():
        override = current.get(item.key)
        if override is None:
            continue
        changes.append({"prompt_key": item.key, "before": override.content, "after": item.default})
        session.delete(override)
    if changes:
        audit.record(
            session,
            ks_id=ks.id,
            action="prompt.restore_all",
            summary=f"Restored {len(changes)} prompt(s) to default",
            actor_id=actor.id,
            actor_name=actor.username,
            detail={"changes": changes},
        )
    return Response(status_code=status.HTTP_204_NO_CONTENT)
