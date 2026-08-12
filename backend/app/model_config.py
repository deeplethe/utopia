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


@dataclass(frozen=True)
class Endpoint:
    conn: Conn
    capacity_key: str
    concurrency_limit: int


@dataclass
class _Defaults:
    llm: Endpoint
    embed: Endpoint


def _limit(value: int | None) -> int:
    return max(1, min(64, int(value or settings.extraction_concurrency)))


def _fallback_endpoint(kind: str, conn: Conn) -> Endpoint:
    return Endpoint(
        conn=conn,
        capacity_key=f"{kind}:fallback:{conn[0].rstrip('/')}:{conn[2]}",
        concurrency_limit=_limit(settings.extraction_concurrency),
    )


_rt = _Defaults(
    llm=_fallback_endpoint(
        "llm", (settings.openrouter_base_url, settings.openrouter_api_key, settings.llm_extract_model),
    ),
    embed=_fallback_endpoint(
        "embedding", (settings.openrouter_base_url, settings.openrouter_api_key, settings.embedding_model),
    ),
)

_llm_endpoint: contextvars.ContextVar[Endpoint | None] = contextvars.ContextVar(
    "llm_endpoint", default=None,
)
_embed_endpoint: contextvars.ContextVar[Endpoint | None] = contextvars.ContextVar(
    "embed_endpoint", default=None,
)


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


def _endpoint_of(p: Provider | None, fallback: Endpoint, kind: str) -> Endpoint:
    if p is None:
        return fallback
    conn = (
        p.base_url or fallback.conn[0],
        p.api_key or fallback.conn[1],
        p.model or fallback.conn[2],
    )
    return Endpoint(
        conn=conn,
        capacity_key=f"{kind}:provider:{p.id}",
        concurrency_limit=_limit(p.concurrency_limit),
    )


def seed_default_provider(session: Session) -> None:
    """On upgrade, ensure a default LLM entry + a default embedding entry exist (from the legacy
    SystemConfig/.env connection) and the system defaults point at the right kind. Idempotent."""
    cfg = get_system_config(session)
    entries = session.exec(select(Provider)).all()
    base = cfg.base_url or settings.openrouter_base_url
    key = cfg.api_key or settings.openrouter_api_key
    legacy_limit = _limit(cfg.extraction_concurrency)

    llm = next((p for p in entries if (p.kind or "llm") == "llm"), None)
    if llm is None:
        llm = Provider(
            name="Default", base_url=base, api_key=key, model=settings.llm_extract_model,
            kind="llm", concurrency_limit=legacy_limit,
        )
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
                       model=settings.embedding_model, kind="embedding",
                       concurrency_limit=legacy_limit)
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
    """Reload the system-default endpoints, including each endpoint's own capacity."""
    cfg = get_system_config(session)
    llm_fallback = _fallback_endpoint(
        "llm", (settings.openrouter_base_url, settings.openrouter_api_key, settings.llm_extract_model),
    )
    embed_fallback = _fallback_endpoint(
        "embedding", (settings.openrouter_base_url, settings.openrouter_api_key, settings.embedding_model),
    )
    _rt.llm = _endpoint_of(_entry(session, cfg.llm_provider_id), llm_fallback, "llm")
    _rt.embed = _endpoint_of(_entry(session, cfg.embedding_provider_id), embed_fallback, "embedding")


# --- effective connections (read by openrouter + embeddings) ---
def llm_conn() -> Conn:
    return (_llm_endpoint.get() or _rt.llm).conn


def embed_conn() -> Conn:
    return (_embed_endpoint.get() or _rt.embed).conn


def system_extract_model() -> str:
    return _rt.llm.conn[2] or settings.llm_extract_model


def llm_concurrency() -> int:
    return (_llm_endpoint.get() or _rt.llm).concurrency_limit


def embedding_concurrency() -> int:
    return (_embed_endpoint.get() or _rt.embed).concurrency_limit


def llm_capacity_key() -> str:
    return (_llm_endpoint.get() or _rt.llm).capacity_key


def embedding_capacity_key() -> str:
    return (_embed_endpoint.get() or _rt.embed).capacity_key


def extraction_concurrency() -> int:
    """Compatibility alias for code outside this package; the value is endpoint-specific."""
    return llm_concurrency()


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
    return _resolve_llm_endpoint(session, ks).conn[2] or settings.llm_extract_model


# --- per-KS resolution ---
def _resolve_llm_endpoint(session: Session, ks: KnowledgeSystem | None) -> Endpoint:
    p = _entry(session, getattr(ks, "llm_provider_id", None) if ks else None)
    return _endpoint_of(p, _rt.llm, "llm")


def _resolve_embed_endpoint(session: Session, ks: KnowledgeSystem | None) -> Endpoint:
    p = _entry(session, getattr(ks, "embedding_provider_id", None) if ks else None)
    return _endpoint_of(p, _rt.embed, "embedding")


def set_ks_connections(session: Session, ks: KnowledgeSystem | None) -> None:
    """Set the KS's llm + embedding connections for the current task (no reset — for a background
    job whose task ends afterwards; request handlers should prefer use_ks_connections)."""
    _llm_endpoint.set(_resolve_llm_endpoint(session, ks))
    _embed_endpoint.set(_resolve_embed_endpoint(session, ks))


@contextlib.contextmanager
def use_ks_connections(session: Session, ks: KnowledgeSystem | None):
    """Publish the KS's effective llm + embedding connections for the duration of the block."""
    t1 = _llm_endpoint.set(_resolve_llm_endpoint(session, ks))
    t2 = _embed_endpoint.set(_resolve_embed_endpoint(session, ks))
    try:
        yield
    finally:
        _llm_endpoint.reset(t1)
        _embed_endpoint.reset(t2)
