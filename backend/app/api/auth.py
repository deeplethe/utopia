"""Login / logout / current-user endpoints."""
from __future__ import annotations

from fastapi import APIRouter, Cookie, Depends, HTTPException, Response
from pydantic import BaseModel
from sqlmodel import Session, select

from app.config import settings
from app.db.database import get_session
from app.db.models import AuthSession, KnowledgeSystem, KSGrant, McpUserToken, User
from app.security import (
    create_session,
    current_user,
    delete_session,
    hash_password,
    require_admin,
    verify_password,
)

router = APIRouter(prefix="/api/auth", tags=["auth"])


class LoginRequest(BaseModel):
    username: str
    password: str


class UserOut(BaseModel):
    id: int
    username: str
    display_name: str | None = None
    is_admin: bool
    active: bool


def user_out(u: User) -> UserOut:
    return UserOut(
        id=u.id,
        username=u.username,
        display_name=u.display_name,
        is_admin=u.is_admin,
        active=u.active,
    )


@router.post("/login", response_model=UserOut)
def login(body: LoginRequest, response: Response, session: Session = Depends(get_session)) -> UserOut:
    user = session.exec(select(User).where(User.username == body.username)).first()
    # Generic error — do not reveal whether the username exists.
    if not user or not user.active or not verify_password(body.password, user.password_hash):
        raise HTTPException(status_code=401, detail="Incorrect username or password")
    token = create_session(session, user.id)
    response.set_cookie(
        settings.session_cookie, token,
        httponly=True, samesite="lax", secure=settings.cookie_secure,
        max_age=settings.session_ttl_hours * 3600, path="/",
    )
    return user_out(user)


@router.post("/logout")
def logout(
    response: Response,
    session: Session = Depends(get_session),
    token: str | None = Cookie(default=None, alias=settings.session_cookie),
) -> dict:
    if token:
        delete_session(session, token)
    response.delete_cookie(settings.session_cookie, path="/")
    return {"ok": True}


@router.get("/me", response_model=UserOut)
def me(user: User = Depends(current_user)) -> UserOut:
    return user_out(user)


class UpdateMe(BaseModel):
    """Self-service profile update. ``display_name`` sets/clears the nickname; changing the
    password requires proving knowledge of the current one."""

    display_name: str | None = None
    current_password: str | None = None
    new_password: str | None = None


@router.patch("/me", response_model=UserOut)
def update_me(
    body: UpdateMe,
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> UserOut:
    if body.display_name is not None:
        name = body.display_name.strip()
        user.display_name = name or None  # empty string clears it -> falls back to username
    if body.new_password is not None:
        if not verify_password(body.current_password or "", user.password_hash):
            raise HTTPException(status_code=403, detail="Current password is incorrect")
        if len(body.new_password) < 6:
            raise HTTPException(status_code=400, detail="New password must be at least 6 characters")
        user.password_hash = hash_password(body.new_password)
    session.add(user)
    session.commit()
    session.refresh(user)
    return user_out(user)


# --------------------------------------------------------------------------- #
# User management (admin only)
# --------------------------------------------------------------------------- #
class CreateUser(BaseModel):
    username: str
    password: str
    is_admin: bool = False


class UpdateUser(BaseModel):
    password: str | None = None
    is_admin: bool | None = None
    active: bool | None = None


@router.get("/users", response_model=list[UserOut])
def list_users(_: User = Depends(require_admin), session: Session = Depends(get_session)) -> list[UserOut]:
    users = session.exec(select(User).order_by(User.id)).all()
    return [user_out(u) for u in users]


@router.post("/users", response_model=UserOut)
def create_user(
    body: CreateUser, _: User = Depends(require_admin), session: Session = Depends(get_session)
) -> UserOut:
    username = body.username.strip()
    if not username:
        raise HTTPException(status_code=400, detail="Username is required")
    if not body.password:
        raise HTTPException(status_code=400, detail="Password is required")
    if session.exec(select(User).where(User.username == username)).first():
        raise HTTPException(status_code=409, detail="Username already exists")
    user = User(username=username, password_hash=hash_password(body.password), is_admin=body.is_admin)
    session.add(user)
    session.commit()
    session.refresh(user)
    return user_out(user)


@router.patch("/users/{uid}", response_model=UserOut)
def update_user(
    uid: int,
    body: UpdateUser,
    admin: User = Depends(require_admin),
    session: Session = Depends(get_session),
) -> UserOut:
    user = session.get(User, uid)
    if not user:
        raise HTTPException(status_code=404, detail="User not found")
    # Guard: don't let an admin strip their own admin flag or deactivate themselves
    # (would lock them out mid-request); and never demote/deactivate the last admin.
    if body.is_admin is False and user.is_admin:
        others = session.exec(select(User).where(User.is_admin == True, User.id != uid)).all()  # noqa: E712
        if not others:
            raise HTTPException(status_code=400, detail="Can't remove the last admin")
    if body.active is False and user.id == admin.id:
        raise HTTPException(status_code=400, detail="You can't deactivate yourself")

    if body.password:
        user.password_hash = hash_password(body.password)
    if body.is_admin is not None:
        user.is_admin = body.is_admin
    if body.active is not None:
        user.active = body.active
        if body.active is False:  # revoke live sessions on deactivation
            for s in session.exec(select(AuthSession).where(AuthSession.user_id == uid)).all():
                session.delete(s)
            for token in session.exec(select(McpUserToken).where(McpUserToken.user_id == uid)).all():
                session.delete(token)
    session.add(user)
    session.commit()
    session.refresh(user)
    return user_out(user)


@router.delete("/users/{uid}")
def delete_user(
    uid: int, admin: User = Depends(require_admin), session: Session = Depends(get_session)
) -> dict:
    user = session.get(User, uid)
    if not user:
        raise HTTPException(status_code=404, detail="User not found")
    if user.id == admin.id:
        raise HTTPException(status_code=400, detail="You can't delete yourself")
    owned = session.exec(select(KnowledgeSystem).where(KnowledgeSystem.owner_id == uid)).all()
    if owned:
        names = "、".join(k.name for k in owned[:5])
        raise HTTPException(
            status_code=409,
            detail=f"This user owns {len(owned)} knowledge system(s) ({names}…); transfer or delete them before deleting the user",
        )
    for g in session.exec(select(KSGrant).where(KSGrant.user_id == uid)).all():
        session.delete(g)
    for s in session.exec(select(AuthSession).where(AuthSession.user_id == uid)).all():
        session.delete(s)
    for token in session.exec(select(McpUserToken).where(McpUserToken.user_id == uid)).all():
        session.delete(token)
    session.delete(user)
    session.commit()
    return {"deleted": uid}
