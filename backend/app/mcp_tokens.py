"""MCP user-token primitives shared by management routes and the MCP verifier."""
from __future__ import annotations

import hashlib
import secrets
from datetime import datetime, timezone

from app.db.models import McpUserToken

MCP_TOKEN_SCOPES: dict[str, str] = {
    "mcp:read": "Read knowledge, evidence, queues, history, and releases",
    "mcp:write": "Apply content edits and resolve review items",
    "mcp:manage": "Run lifecycle and destructive owner-level operations",
}

_ROLE_SCOPES = {
    "viewer": ["mcp:read"],
    "editor": ["mcp:read", "mcp:write"],
    "owner": list(MCP_TOKEN_SCOPES),
}


def allowed_scopes(role: str) -> list[str]:
    return list(_ROLE_SCOPES.get(role, []))


def mint(public_id: str) -> str:
    return f"opm_{public_id[:10]}_{secrets.token_urlsafe(32)}"


def digest(token: str) -> str:
    return hashlib.sha256(token.encode("utf-8")).hexdigest()


def normalize_scopes(scopes: list[str]) -> list[str]:
    requested = set(scopes)
    return [scope for scope in MCP_TOKEN_SCOPES if scope in requested]


def unknown_scopes(scopes: list[str]) -> list[str]:
    return sorted(set(scopes) - MCP_TOKEN_SCOPES.keys())


def aware(value: datetime) -> datetime:
    return value.replace(tzinfo=timezone.utc) if value.tzinfo is None else value


def status(token: McpUserToken, now: datetime | None = None) -> str:
    if token.revoked_at is not None:
        return "revoked"
    current = now or datetime.now(timezone.utc)
    if aware(token.expires_at) <= current:
        return "expired"
    return "active"
