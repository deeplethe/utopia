"""Registered model prompts with per-knowledge-system runtime overrides."""
from __future__ import annotations

import hashlib
from collections.abc import Iterator
from contextlib import contextmanager
from contextvars import ContextVar, Token
from dataclasses import dataclass
from importlib import import_module

from sqlmodel import Session, select

from app.config import settings
from app.db.models import KnowledgePromptOverride
from app.prompt_locales import ZH_CN_PROMPTS


@dataclass(frozen=True)
class PromptDefinition:
    key: str
    category: str
    title: str
    description: str
    default: str
    variables: tuple[str, ...] = ()
    order: int = 0


_registry: dict[str, PromptDefinition] = {}
_active_overrides: ContextVar[dict[str, str] | None] = ContextVar(
    "knowledge_prompt_overrides",
    default=None,
)
_catalog_loaded = False

_PROMPT_MODULES = (
    "app.ontology.extract",
    "app.ontology.abox_extract",
    "app.ontology.resolution",
    "app.ontology.conflicts",
    "app.ontology.conflict_agent",
    "app.ontology.structure_agent",
    "app.ontology.tbox_reconcile",
    "app.ontology.terminology_agent",
    "app.ontology.validation_agent",
    "app.ontology.modeling_assistant",
    "app.agent_runtime",
)


def _localized_default(key: str, english_default: str) -> str:
    if settings.system_language == "en":
        return english_default
    localized = ZH_CN_PROMPTS.get(key)
    if localized is None or not localized.strip():
        raise RuntimeError(f"Missing zh-CN prompt for registered key: {key}")
    return localized


def register(
    *,
    key: str,
    category: str,
    title: str,
    description: str,
    default: str,
    variables: tuple[str, ...] = (),
    order: int = 0,
) -> None:
    if not key or not default.strip():
        raise ValueError("Prompt key and default content are required")
    effective_default = _localized_default(key, default)
    missing = [name for name in variables if "{" + name + "}" not in effective_default]
    if missing:
        placeholders = ", ".join("{" + name + "}" for name in missing)
        raise ValueError(f"Localized prompt {key!r} must retain placeholder(s): {placeholders}")
    _registry[key] = PromptDefinition(
        key=key,
        category=category,
        title=title,
        description=description,
        default=effective_default,
        variables=variables,
        order=order,
    )


def ensure_catalog_loaded() -> None:
    global _catalog_loaded
    if _catalog_loaded:
        return
    for module in _PROMPT_MODULES:
        import_module(module)
    if settings.system_language == "zh-CN":
        extra = sorted(set(ZH_CN_PROMPTS) - set(_registry))
        if extra:
            raise RuntimeError(f"Unknown zh-CN prompt key(s): {', '.join(extra)}")
    _catalog_loaded = True


def definitions() -> tuple[PromptDefinition, ...]:
    ensure_catalog_loaded()
    category_order = {"extraction": 0, "review": 1, "governance": 2, "validation": 3}
    return tuple(sorted(
        _registry.values(),
        key=lambda item: (category_order.get(item.category, 99), item.order, item.title.lower()),
    ))


def definition(key: str) -> PromptDefinition | None:
    ensure_catalog_loaded()
    return _registry.get(key)


def require_definition(key: str) -> PromptDefinition:
    item = definition(key)
    if item is None:
        raise KeyError(key)
    return item


def validate_content(item: PromptDefinition, content: str) -> None:
    if not content.strip():
        raise ValueError("Prompt content cannot be empty")
    if len(content) > 100_000:
        raise ValueError("Prompt content cannot exceed 100,000 characters")
    missing = [name for name in item.variables if "{" + name + "}" not in content]
    if missing:
        placeholders = ", ".join("{" + name + "}" for name in missing)
        raise ValueError(f"Prompt must retain required placeholder(s): {placeholders}")


def load_overrides(session: Session, ks_id: int) -> dict[str, str]:
    ensure_catalog_loaded()
    rows = session.exec(
        select(KnowledgePromptOverride).where(
            KnowledgePromptOverride.knowledge_system_id == ks_id,
        )
    ).all()
    return {row.prompt_key: row.content for row in rows if row.prompt_key in _registry}


def snapshot(session: Session, ks_id: int) -> dict:
    """Return an immutable, content-addressed snapshot of every effective prompt."""
    overrides = load_overrides(session, ks_id)
    prompts = {}
    for item in definitions():
        content = overrides.get(item.key, item.default)
        prompts[item.key] = {
            "content": content,
            "sha256": hashlib.sha256(content.encode("utf-8")).hexdigest(),
            "overridden": item.key in overrides,
        }
    return {"prompts": prompts}


def set_ks_prompts(session: Session, ks_id: int) -> Token:
    """Snapshot this knowledge system's prompts for the current task/job."""
    return _active_overrides.set(load_overrides(session, ks_id))


@contextmanager
def use_ks_prompts(session: Session, ks_id: int) -> Iterator[None]:
    token = set_ks_prompts(session, ks_id)
    try:
        yield
    finally:
        _active_overrides.reset(token)


def get(key: str) -> str:
    item = require_definition(key)
    overrides = _active_overrides.get()
    if overrides is not None and key in overrides:
        return overrides[key]
    return item.default


def render(key: str, **values: object) -> str:
    item = require_definition(key)
    missing = [name for name in item.variables if name not in values]
    if missing:
        raise ValueError(f"Missing prompt variable(s): {', '.join(missing)}")
    content = get(key)
    for name in item.variables:
        content = content.replace("{" + name + "}", str(values[name]))
    return content
