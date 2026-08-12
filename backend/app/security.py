"""Authentication: password hashing, server-side sessions, and request dependencies."""
from __future__ import annotations

import secrets
from datetime import timedelta, timezone

import bcrypt
from fastapi import Cookie, Depends, HTTPException
from sqlmodel import Session

from app.config import settings
from app.db.database import get_session
from app.db.models import AuthSession, User, utcnow


def hash_password(password: str) -> str:
    return bcrypt.hashpw(password.encode("utf-8"), bcrypt.gensalt()).decode("utf-8")


def verify_password(password: str, password_hash: str) -> bool:
    try:
        return bcrypt.checkpw(password.encode("utf-8"), password_hash.encode("utf-8"))
    except Exception:  # noqa: BLE001  (malformed hash etc.)
        return False


def create_session(session: Session, user_id: int) -> str:
    token = secrets.token_urlsafe(32)
    session.add(AuthSession(
        token=token,
        user_id=user_id,
        expires_at=utcnow() + timedelta(hours=settings.session_ttl_hours),
    ))
    session.commit()
    return token


def delete_session(session: Session, token: str) -> None:
    row = session.get(AuthSession, token)
    if row:
        session.delete(row)
        session.commit()


def _aware(dt):
    return dt.replace(tzinfo=timezone.utc) if dt.tzinfo is None else dt


def current_user(
    session: Session = Depends(get_session),
    token: str | None = Cookie(default=None, alias=settings.session_cookie),
) -> User:
    if not token:
        raise HTTPException(status_code=401, detail="Not authenticated")
    row = session.get(AuthSession, token)
    if not row or _aware(row.expires_at) < utcnow():
        if row:
            session.delete(row)
            session.commit()
        raise HTTPException(status_code=401, detail="Session expired")
    user = session.get(User, row.user_id)
    if not user or not user.active:
        raise HTTPException(status_code=401, detail="User inactive")
    return user


def require_admin(user: User = Depends(current_user)) -> User:
    if not user.is_admin:
        raise HTTPException(status_code=403, detail="Admin only")
    return user
