"""Owner-managed, knowledge-system-scoped API tokens."""
from __future__ import annotations

from datetime import datetime, timedelta

from fastapi import APIRouter, Depends, HTTPException, Response
from pydantic import BaseModel, Field
from sqlmodel import Session, select

from app import access_tokens, audit, token_secrets
from app.db.database import get_session
from app.db.models import KnowledgeApiToken, KnowledgeSystem, User, utcnow
from app.permissions import ks_owner
from app.security import current_user

router = APIRouter(prefix="/api/knowledge", tags=["api access"])


class TokenCreate(BaseModel):
    name: str = Field(min_length=1, max_length=80)
    scopes: list[str] = Field(default_factory=lambda: list(access_tokens.DEFAULT_TOKEN_SCOPES))
    expires_in_days: int | None = Field(default=90, ge=1, le=3650)


class TokenOut(BaseModel):
    id: int
    name: str
    token_prefix: str
    scopes: list[str]
    status: str
    created_at: datetime
    expires_at: datetime | None
    last_used_at: datetime | None
    revoked_at: datetime | None
    can_reveal: bool


class TokenCreated(TokenOut):
    token: str


class TokenRevealed(BaseModel):
    token: str


def _out(row: KnowledgeApiToken) -> TokenOut:
    token_status = access_tokens.status(row)
    return TokenOut(
        id=row.id,
        name=row.name,
        token_prefix=row.token_prefix,
        scopes=row.scopes,
        status=token_status,
        created_at=row.created_at,
        expires_at=row.expires_at,
        last_used_at=row.last_used_at,
        revoked_at=row.revoked_at,
        can_reveal=token_status == "active" and bool(row.secret_ciphertext),
    )


@router.get("/{ks_id}/tokens", response_model=list[TokenOut])
def list_tokens(
    ks: KnowledgeSystem = Depends(ks_owner), session: Session = Depends(get_session),
) -> list[TokenOut]:
    rows = session.exec(
        select(KnowledgeApiToken)
        .where(KnowledgeApiToken.knowledge_system_id == ks.id)
        .order_by(KnowledgeApiToken.created_at.desc(), KnowledgeApiToken.id.desc())
    ).all()
    return [_out(row) for row in rows]


@router.post("/{ks_id}/tokens", response_model=TokenCreated)
def create_token(
    body: TokenCreate,
    response: Response,
    ks: KnowledgeSystem = Depends(ks_owner),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> TokenCreated:
    name = body.name.strip()
    if not name:
        raise HTTPException(status_code=400, detail="Token name cannot be empty")
    invalid = access_tokens.unknown_scopes(body.scopes)
    if invalid:
        raise HTTPException(status_code=400, detail=f"Unknown token scopes: {', '.join(invalid)}")
    scopes = access_tokens.normalize_scopes(body.scopes)
    if not scopes:
        raise HTTPException(status_code=400, detail="Select at least one token scope")
    if "provenance:read" in scopes and "instances:read" not in scopes:
        raise HTTPException(
            status_code=400,
            detail='Scope "provenance:read" requires "instances:read"',
        )

    plaintext = access_tokens.mint(ks.public_id)
    row = KnowledgeApiToken(
        knowledge_system_id=ks.id,
        name=name,
        token_prefix=plaintext[:18],
        token_hash=access_tokens.digest(plaintext),
        secret_ciphertext=token_secrets.encrypt(plaintext),
        scopes=scopes,
        created_by=user.id,
        expires_at=(utcnow() + timedelta(days=body.expires_in_days)) if body.expires_in_days else None,
    )
    session.add(row)
    session.commit()
    session.refresh(row)
    audit.record(
        session,
        ks_id=ks.id,
        action="token.create",
        summary=f'Created API token "{name}"',
        actor_id=user.id,
        actor_name=user.username,
        detail={"token_id": row.id, "prefix": row.token_prefix, "scopes": scopes},
    )
    response.headers["Cache-Control"] = "no-store"
    response.headers["Pragma"] = "no-cache"
    return TokenCreated(**_out(row).model_dump(), token=plaintext)


@router.post("/{ks_id}/tokens/{token_id}/reveal", response_model=TokenRevealed)
def reveal_token(
    token_id: int,
    response: Response,
    ks: KnowledgeSystem = Depends(ks_owner),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> TokenRevealed:
    row = session.get(KnowledgeApiToken, token_id)
    if not row or row.knowledge_system_id != ks.id:
        raise HTTPException(status_code=404, detail="API token not found")
    if access_tokens.status(row) != "active":
        raise HTTPException(status_code=409, detail="Only active API tokens can be revealed")
    if not row.secret_ciphertext:
        raise HTTPException(
            status_code=409,
            detail="This legacy token cannot be recovered; create a replacement token",
        )
    try:
        plaintext = token_secrets.decrypt(row.secret_ciphertext)
    except token_secrets.TokenSecretUnavailable as exc:
        raise HTTPException(
            status_code=409,
            detail="This token cannot be decrypted with the current server key; rotate it",
        ) from exc
    if access_tokens.digest(plaintext) != row.token_hash:
        raise HTTPException(status_code=409, detail="Stored token secret failed verification; rotate it")

    response.headers["Cache-Control"] = "no-store"
    response.headers["Pragma"] = "no-cache"
    audit.record(
        session,
        ks_id=ks.id,
        action="token.reveal",
        summary=f'Revealed API token "{row.name}"',
        actor_id=user.id,
        actor_name=user.username,
        detail={"token_id": row.id, "prefix": row.token_prefix},
    )
    return TokenRevealed(token=plaintext)


@router.delete("/{ks_id}/tokens/{token_id}", response_model=TokenOut)
def revoke_token(
    token_id: int,
    ks: KnowledgeSystem = Depends(ks_owner),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> TokenOut:
    row = session.get(KnowledgeApiToken, token_id)
    if not row or row.knowledge_system_id != ks.id:
        raise HTTPException(status_code=404, detail="API token not found")
    if row.revoked_at is None:
        row.revoked_at = utcnow()
        row.secret_ciphertext = None
        session.add(row)
        session.commit()
        session.refresh(row)
        audit.record(
            session,
            ks_id=ks.id,
            action="token.revoke",
            summary=f'Revoked API token "{row.name}"',
            actor_id=user.id,
            actor_name=user.username,
            detail={"token_id": row.id, "prefix": row.token_prefix},
        )
    return _out(row)
