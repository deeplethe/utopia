"""Knowledge-system API token primitives shared by management and external routes."""
from __future__ import annotations

import hashlib
import secrets
from datetime import datetime, timezone

from app.db.models import KnowledgeApiToken

TOKEN_SCOPES: dict[str, str] = {
    "ontology:read": "Read ontology metadata, schema views, and RDF exports",
    "vocabulary:read": "Browse and resolve controlled terminology and export SKOS RDF",
    "instances:read": "Browse and search ABox individuals and assertions",
    "query:read": "Run bounded read-only SPARQL SELECT and ASK queries",
    "provenance:read": "Include source documents, chunks, and evidence snippets",
}
DEFAULT_TOKEN_SCOPES = ["ontology:read", "vocabulary:read", "instances:read", "query:read"]


def mint(public_id: str) -> str:
    return f"opk_{public_id[:10]}_{secrets.token_urlsafe(32)}"


def digest(token: str) -> str:
    return hashlib.sha256(token.encode("utf-8")).hexdigest()


def normalize_scopes(scopes: list[str]) -> list[str]:
    requested = set(scopes)
    return [scope for scope in TOKEN_SCOPES if scope in requested]


def unknown_scopes(scopes: list[str]) -> list[str]:
    return sorted(set(scopes) - TOKEN_SCOPES.keys())


def aware(value: datetime) -> datetime:
    return value.replace(tzinfo=timezone.utc) if value.tzinfo is None else value


def status(token: KnowledgeApiToken, now: datetime | None = None) -> str:
    if token.revoked_at is not None:
        return "revoked"
    current = now or datetime.now(timezone.utc)
    if token.expires_at is not None and aware(token.expires_at) <= current:
        return "expired"
    return "active"
