"""System settings: which model entry is the default LLM and which is the default embedding.

Model entries themselves (the flat list of endpoint+key+model+kind) are managed under /api/providers.
Here we just record which entry is each default; a knowledge system may point at different entries."""
from __future__ import annotations

from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel
from sqlmodel import Session

from app.config import settings
from app.db.database import get_session
from app.db.models import Provider, User, utcnow
from app.model_config import available_models, get_system_config, refresh_runtime
from app.security import current_user, require_admin

router = APIRouter(prefix="/api", tags=["settings"])


class SettingsOut(BaseModel):
    llm_provider_id: int | None        # default LLM model entry
    embedding_provider_id: int | None  # default embedding model entry
    available_models: list[str]        # model-name suggestions
    temperature: float                 # read-only (.env-managed)


def _payload(session: Session) -> SettingsOut:
    cfg = get_system_config(session)
    return SettingsOut(
        llm_provider_id=cfg.llm_provider_id,
        embedding_provider_id=cfg.embedding_provider_id,
        available_models=available_models(),
        temperature=settings.llm_temperature,
    )


@router.get("/settings", response_model=SettingsOut)
def get_settings(_: User = Depends(require_admin), session: Session = Depends(get_session)) -> SettingsOut:
    return _payload(session)


class SettingsUpdate(BaseModel):
    llm_provider_id: int | None = None        # omit = unchanged
    embedding_provider_id: int | None = None


def _require(session: Session, pid: int, kind: str) -> None:
    p = session.get(Provider, pid)
    if p is None:
        raise HTTPException(status_code=400, detail=f"Model entry {pid} not found")
    if (p.kind or "llm") != kind:
        raise HTTPException(status_code=400, detail=f"Entry {pid} is a {p.kind} entry, not {kind}")


@router.put("/settings", response_model=SettingsOut)
def update_settings(
    body: SettingsUpdate, _: User = Depends(require_admin), session: Session = Depends(get_session),
) -> SettingsOut:
    cfg = get_system_config(session)
    if body.llm_provider_id is not None:
        _require(session, body.llm_provider_id, "llm")
        cfg.llm_provider_id = body.llm_provider_id
    if body.embedding_provider_id is not None:
        _require(session, body.embedding_provider_id, "embedding")
        cfg.embedding_provider_id = body.embedding_provider_id
    cfg.updated_at = utcnow()
    session.add(cfg)
    session.commit()
    refresh_runtime(session)  # apply the new default connection/model process-wide, no restart
    return _payload(session)


@router.get("/models")
def list_models(_: User = Depends(current_user)) -> dict:
    """Model-name suggestions + the .env default (any authenticated user can read, for pickers)."""
    return {"models": available_models(), "default": settings.llm_extract_model}
