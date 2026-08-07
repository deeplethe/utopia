"""Model-endpoint resolution over a flat list of entries.

Each entry (``Provider`` row) bundles an OpenAI-compatible connection + a model + a kind
(llm | embedding). The system picks one default llm entry + one default embedding entry; a knowledge
system may point at different entries. The effective (base_url, api_key, model) for the current unit
of work is published via contextvars (set by ``use_ks_connections`` / ``set_ks_connections`` at job /
detect entry) so the many LLM/embedding call sites don't each need it threaded — contextvars
propagate into ``asyncio.gather`` children and ``asyncio.to_thread`` workers.
"""
from __future__ import annotations

import contextlib
import contextvars
from dataclasses import dataclass

from sqlmodel import Session, select

from app.config import settings
from app.db.models import KnowledgeSystem, Provider, SystemConfig

Conn = tuple[str, str, str]  # (base_url, api_key, model)


@dataclass
class _Defaults:
    llm: Conn
    embed: Conn


_rt = _Defaults(
    llm=(settings.openrouter_base_url, settings.openrouter_api_key, settings.llm_extract_model),
    embed=(settings.openrouter_base_url, settings.openrouter_api_key, settings.embedding_model),
)

_llm_conn: contextvars.ContextVar = contextvars.ContextVar("llm_conn", default=None)      # Conn
_embed_conn: contextvars.ContextVar = contextvars.ContextVar("embed_conn", default=None)  # Conn


def get_system_config(session: Session) -> SystemConfig:
    cfg = session.get(SystemConfig, 1)
    if cfg is None:
        cfg = SystemConfig(id=1)
        session.add(cfg)
        session.commit()
        session.refresh(cfg)
    return cfg


def _entry(session: Session, pid) -> Provider | None:
    return session.get(Provider, pid) if pid else None


def _conn_of(p: Provider | None, fallback: Conn) -> Conn:
    if p is None:
        return fallback
    return (p.base_url or fallback[0], p.api_key or fallback[1], p.model or fallback[2])


def seed_default_provider(session: Session) -> None:
    """On upgrade, ensure a default LLM entry + a default embedding entry exist (from the legacy
    SystemConfig/.env connection) and the system defaults point at the right kind. Idempotent."""
    cfg = get_system_config(session)
    entries = session.exec(select(Provider)).all()
    base = cfg.base_url or settings.openrouter_base_url
    key = cfg.api_key or settings.openrouter_api_key

    llm = next((p for p in entries if (p.kind or "llm") == "llm"), None)
    if llm is None:
        llm = Provider(name="Default", base_url=base, api_key=key, model=settings.llm_extract_model, kind="llm")
        session.add(llm)
        session.commit()
        session.refresh(llm)
    else:  # backfill a legacy connection-only entry
        changed = False
        if not llm.kind:
            llm.kind, changed = "llm", True
        if not llm.model:
            llm.model, changed = settings.llm_extract_model, True
        if changed:
            session.add(llm)
            session.commit()

    emb = next((p for p in entries if p.kind == "embedding"), None)
    if emb is None:
        emb = Provider(name="Default embedding", base_url=base, api_key=key,
                       model=settings.embedding_model, kind="embedding")
        session.add(emb)
        session.commit()
        session.refresh(emb)

    # Point the system defaults at the correct kind (fix a legacy embedding default that pointed at
    # the llm entry).
    cur_llm = _entry(session, cfg.llm_provider_id)
    cur_emb = _entry(session, cfg.embedding_provider_id)
    changed = False
    if cur_llm is None or cur_llm.kind == "embedding":
        cfg.llm_provider_id, changed = llm.id, True
    if cur_emb is None or cur_emb.kind != "embedding":
        cfg.embedding_provider_id, changed = emb.id, True
    if changed:
        session.add(cfg)
        session.commit()


def refresh_runtime(session: Session) -> None:
    """Reload the system-default (base_url, api_key, model) for llm + embedding from the DB."""
    cfg = get_system_config(session)
    _rt.llm = _conn_of(_entry(session, cfg.llm_provider_id),
                       (settings.openrouter_base_url, settings.openrouter_api_key, settings.llm_extract_model))
    _rt.embed = _conn_of(_entry(session, cfg.embedding_provider_id),
                         (settings.openrouter_base_url, settings.openrouter_api_key, settings.embedding_model))


# --- effective connections (read by openrouter + embeddings) ---
def llm_conn() -> Conn:
    return _llm_conn.get() or _rt.llm


def embed_conn() -> Conn:
    return _embed_conn.get() or _rt.embed


def system_extract_model() -> str:
    return _rt.llm[2] or settings.llm_extract_model


def available_models() -> list[str]:
    """Suggested model names for the UI. Free text is allowed; this is only a convenience list."""
    choices = list(settings.llm_model_choices or [])
    if settings.llm_extract_model not in choices:
        choices.insert(0, settings.llm_extract_model)
    return choices


def resolve_extract_model(session: Session, ks: KnowledgeSystem | None = None,
                          request_model: str | None = None) -> str:
    """Effective LLM model for a job: explicit request > the KS's (or default) llm entry's model."""
    if request_model:
        return request_model
    return _resolve_llm_conn(session, ks)[2] or settings.llm_extract_model


# --- per-KS resolution ---
def _resolve_llm_conn(session: Session, ks: KnowledgeSystem | None) -> Conn:
    p = _entry(session, getattr(ks, "llm_provider_id", None) if ks else None)
    return _conn_of(p, _rt.llm)


def _resolve_embed_conn(session: Session, ks: KnowledgeSystem | None) -> Conn:
    p = _entry(session, getattr(ks, "embedding_provider_id", None) if ks else None)
    return _conn_of(p, _rt.embed)


def set_ks_connections(session: Session, ks: KnowledgeSystem | None) -> None:
    """Set the KS's llm + embedding connections for the current task (no reset — for a background
    job whose task ends afterwards; request handlers should prefer use_ks_connections)."""
    _llm_conn.set(_resolve_llm_conn(session, ks))
    _embed_conn.set(_resolve_embed_conn(session, ks))


@contextlib.contextmanager
def use_ks_connections(session: Session, ks: KnowledgeSystem | None):
    """Publish the KS's effective llm + embedding connections for the duration of the block."""
    t1 = _llm_conn.set(_resolve_llm_conn(session, ks))
    t2 = _embed_conn.set(_resolve_embed_conn(session, ks))
    try:
        yield
    finally:
        _llm_conn.reset(t1)
        _embed_conn.reset(t2)
