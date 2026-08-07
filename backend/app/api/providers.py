"""Model endpoints: a flat list of {name, kind, base_url, api_key, model} entries + a live test.

Each entry bundles an OpenAI-compatible connection with a specific model and its kind
(llm | embedding). The system default and each knowledge system just point at one llm entry + one
embedding entry. The api_key is stored server-side and never returned raw (only a masked hint). The
test endpoint makes a tiny live call so the UI can verify an entry actually works.
"""
from __future__ import annotations

import time

import httpx
from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel
from sqlmodel import Session, select

from app.config import settings
from app.db.database import get_session
from app.db.models import KnowledgeSystem, Provider, User, utcnow
from app.model_config import get_system_config, refresh_runtime
from app.security import current_user, require_admin

router = APIRouter(prefix="/api/providers", tags=["providers"])


def _mask(key: str) -> str:
    if not key:
        return ""
    return "••••" + key[-4:] if len(key) > 8 else "••••"


class ProviderOut(BaseModel):
    id: int
    name: str
    kind: str
    base_url: str
    model: str
    has_api_key: bool
    api_key_hint: str
    last_test_ok: bool | None
    last_tested_at: str | None


def _out(p: Provider) -> ProviderOut:
    return ProviderOut(id=p.id, name=p.name, kind=p.kind or "llm", base_url=p.base_url, model=p.model,
                       has_api_key=bool(p.api_key), api_key_hint=_mask(p.api_key),
                       last_test_ok=p.last_test_ok,
                       last_tested_at=p.last_tested_at.isoformat() if p.last_tested_at else None)


@router.get("", response_model=list[ProviderOut])
def list_providers(_: User = Depends(current_user), session: Session = Depends(get_session)) -> list[ProviderOut]:
    # Readable by any authenticated user (writers pick an entry per KS); the api_key is masked. All
    # write ops below stay admin-only.
    return [_out(p) for p in session.exec(select(Provider).order_by(Provider.id)).all()]


class ProviderIn(BaseModel):
    name: str
    kind: str = "llm"  # "llm" | "embedding"
    base_url: str = ""
    api_key: str = ""
    model: str = ""


@router.post("", response_model=ProviderOut)
def create_provider(body: ProviderIn, _: User = Depends(require_admin), session: Session = Depends(get_session)) -> ProviderOut:
    if not body.name.strip():
        raise HTTPException(status_code=400, detail="Name is required")
    kind = body.kind if body.kind in ("llm", "embedding") else "llm"
    p = Provider(name=body.name.strip(), kind=kind, base_url=body.base_url.strip(),
                 api_key=body.api_key.strip(), model=body.model.strip())
    session.add(p)
    session.commit()
    session.refresh(p)
    return _out(p)


class ProviderPatch(BaseModel):
    name: str | None = None
    kind: str | None = None
    base_url: str | None = None
    api_key: str | None = None  # None/"" = keep the stored key; a value replaces it
    model: str | None = None


@router.patch("/{pid}", response_model=ProviderOut)
def update_provider(pid: int, body: ProviderPatch, _: User = Depends(require_admin),
                    session: Session = Depends(get_session)) -> ProviderOut:
    p = session.get(Provider, pid)
    if not p:
        raise HTTPException(status_code=404, detail="Model entry not found")
    if body.name is not None and body.name.strip():
        p.name = body.name.strip()
    if body.kind in ("llm", "embedding"):
        p.kind = body.kind
    if body.base_url is not None:
        p.base_url = body.base_url.strip()
    if body.model is not None:
        p.model = body.model.strip()
    if body.api_key:  # only overwrite when a new non-empty key is provided
        p.api_key = body.api_key.strip()
    session.add(p)
    session.commit()
    refresh_runtime(session)  # a referenced entry's endpoint/key/model may have changed
    return _out(p)


@router.delete("/{pid}")
def delete_provider(pid: int, _: User = Depends(require_admin), session: Session = Depends(get_session)) -> dict:
    p = session.get(Provider, pid)
    if not p:
        raise HTTPException(status_code=404, detail="Model entry not found")
    cfg = get_system_config(session)
    if cfg.llm_provider_id == pid or cfg.embedding_provider_id == pid:
        raise HTTPException(status_code=409, detail="This is a system default; pick another default first")
    in_use = session.exec(
        select(KnowledgeSystem).where(
            (KnowledgeSystem.llm_provider_id == pid) | (KnowledgeSystem.embedding_provider_id == pid))
    ).first()
    if in_use:
        raise HTTPException(status_code=409, detail="Used by a knowledge system; reassign it there first")
    session.delete(p)
    session.commit()
    return {"deleted": pid}


class TestReq(BaseModel):
    provider_id: int | None = None  # test a saved entry (its stored key/model unless overridden)
    base_url: str | None = None
    api_key: str | None = None
    model: str | None = None
    kind: str | None = None  # "llm" | "embedding"


@router.post("/test")
def test_provider(body: TestReq, _: User = Depends(require_admin), session: Session = Depends(get_session)) -> dict:
    """Make a minimal live call to verify an entry (endpoint + key + model). Returns {ok, message, latency_ms}."""
    base_url, api_key, model, kind = body.base_url, body.api_key, body.model, body.kind
    if body.provider_id:
        p = session.get(Provider, body.provider_id)
        if p:
            base_url = base_url or p.base_url
            api_key = api_key or p.api_key
            model = model or p.model
            kind = kind or p.kind
    base_url = (base_url or settings.openrouter_base_url).rstrip("/")
    kind = kind or "llm"
    if not api_key:
        raise HTTPException(status_code=400, detail="No API key to test with")
    if not (model or "").strip():
        raise HTTPException(status_code=400, detail="A model name is required to test")
    headers = {"Authorization": f"Bearer {api_key}", "Content-Type": "application/json"}
    t0 = time.perf_counter()
    try:
        if kind == "embedding":
            r = httpx.post(f"{base_url}/embeddings", headers=headers,
                           json={"model": model, "input": ["ping"]}, timeout=30.0)
        else:
            r = httpx.post(f"{base_url}/chat/completions", headers=headers,
                           json={"model": model, "messages": [{"role": "user", "content": "ping"}],
                                 "max_tokens": 1}, timeout=30.0)
        dt = int((time.perf_counter() - t0) * 1000)
        if r.status_code == 200:
            result = {"ok": True, "message": f"Connected in {dt} ms", "latency_ms": dt}
        else:
            try:
                detail = (r.json().get("error") or {}).get("message") or r.text[:200]
            except Exception:  # noqa: BLE001
                detail = r.text[:200]
            result = {"ok": False, "message": f"HTTP {r.status_code}: {detail}", "latency_ms": dt}
    except Exception as e:  # noqa: BLE001
        result = {"ok": False, "message": str(e)[:200], "latency_ms": int((time.perf_counter() - t0) * 1000)}

    # Persist the result on a saved entry so its status survives a page refresh.
    if body.provider_id:
        p = session.get(Provider, body.provider_id)
        if p:
            p.last_test_ok = result["ok"]
            p.last_tested_at = utcnow()
            session.add(p)
            session.commit()
    return result
