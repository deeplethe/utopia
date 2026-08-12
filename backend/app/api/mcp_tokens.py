"""Create and revoke user-delegated MCP credentials for one knowledge system."""
from __future__ import annotations

from datetime import timedelta

from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel, Field
from sqlmodel import Session, select

from app import mcp_tokens
from app.config import settings
from app.db.database import get_session
from app.db.models import KnowledgeSystem, McpUserToken, User, utcnow
from app.permissions import effective_role, ks_reader
from app.security import current_user

router = APIRouter(prefix="/api/knowledge", tags=["mcp access"])


class CreateMcpToken(BaseModel):
    name: str = "Agent session"
    scopes: list[str] | None = None
    expires_in_minutes: int | None = Field(default=None, ge=5)


def _out(row: McpUserToken) -> dict:
    return {
        "id": row.id,
        "name": row.name,
        "token_prefix": row.token_prefix,
        "scopes": row.scopes,
        "status": mcp_tokens.status(row),
        "created_at": row.created_at.isoformat(),
        "expires_at": row.expires_at.isoformat(),
        "last_used_at": row.last_used_at.isoformat() if row.last_used_at else None,
    }


@router.get("/{ks_id}/mcp/tokens")
def list_mcp_tokens(
    ks: KnowledgeSystem = Depends(ks_reader),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict:
    rows = session.exec(
        select(McpUserToken).where(
            McpUserToken.knowledge_system_id == ks.id,
            McpUserToken.user_id == user.id,
        ).order_by(McpUserToken.id.desc())
    ).all()
    return {
        "endpoint": settings.mcp_public_url,
        "supported_scopes": mcp_tokens.MCP_TOKEN_SCOPES,
        "items": [_out(row) for row in rows],
    }


@router.post("/{ks_id}/mcp/tokens")
def create_mcp_token(
    body: CreateMcpToken,
    ks: KnowledgeSystem = Depends(ks_reader),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict:
    name = body.name.strip()
    if not name:
        raise HTTPException(status_code=400, detail="Token name is required")
    role = effective_role(session, ks, user)
    allowed = mcp_tokens.allowed_scopes(role or "")
    requested = body.scopes if body.scopes is not None else allowed
    unknown = mcp_tokens.unknown_scopes(requested)
    if unknown:
        raise HTTPException(status_code=400, detail=f"Unknown MCP scopes: {', '.join(unknown)}")
    scopes = mcp_tokens.normalize_scopes(requested)
    denied = [scope for scope in scopes if scope not in allowed]
    if denied:
        raise HTTPException(
            status_code=403,
            detail=f"Role {role} cannot grant MCP scopes: {', '.join(denied)}",
        )
    if "mcp:read" not in scopes:
        raise HTTPException(status_code=400, detail="mcp:read is required")
    ttl = body.expires_in_minutes or settings.mcp_token_ttl_minutes
    if ttl > settings.mcp_max_token_ttl_minutes:
        raise HTTPException(
            status_code=400,
            detail=f"Token lifetime cannot exceed {settings.mcp_max_token_ttl_minutes} minutes",
        )

    plaintext = mcp_tokens.mint(ks.public_id)
    row = McpUserToken(
        knowledge_system_id=ks.id,
        user_id=user.id,
        name=name,
        token_prefix=plaintext[:24],
        token_hash=mcp_tokens.digest(plaintext),
        scopes=scopes,
        expires_at=utcnow() + timedelta(minutes=ttl),
    )
    session.add(row)
    session.commit()
    session.refresh(row)
    return {**_out(row), "token": plaintext, "endpoint": settings.mcp_public_url}


@router.delete("/{ks_id}/mcp/tokens/{token_id}")
def revoke_mcp_token(
    token_id: int,
    ks: KnowledgeSystem = Depends(ks_reader),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict:
    row = session.get(McpUserToken, token_id)
    if not row or row.knowledge_system_id != ks.id:
        raise HTTPException(status_code=404, detail="MCP token not found")
    role = effective_role(session, ks, user)
    if row.user_id != user.id and role != "owner":
        raise HTTPException(status_code=403, detail="You may only revoke your own MCP tokens")
    if row.revoked_at is None:
        row.revoked_at = utcnow()
        session.add(row)
        session.commit()
        session.refresh(row)
    return _out(row)
