from __future__ import annotations

from contextlib import asynccontextmanager
from datetime import timedelta

import pytest
from fastapi import FastAPI, HTTPException
from fastapi.testclient import TestClient
from mcp.server.auth.middleware.auth_context import auth_context_var
from mcp.server.auth.middleware.bearer_auth import AuthenticatedUser
from mcp.server.auth.provider import AccessToken
from pyoxigraph import Store
from sqlalchemy.pool import StaticPool
from sqlmodel import Session, SQLModel, create_engine

from app import mcp_server, mcp_tokens
from app.api import mcp_tokens as token_api
from app.db.models import KSGrant, KnowledgeSystem, McpUserToken, User, utcnow
from app.ontology import schema, store


def _database():
    database = create_engine(
        "sqlite://",
        connect_args={"check_same_thread": False},
        poolclass=StaticPool,
    )
    SQLModel.metadata.create_all(database)
    return database


def test_mcp_user_token_is_bound_to_live_role() -> None:
    database = _database()
    with Session(database) as session:
        owner = User(username="owner", password_hash="unused")
        viewer = User(username="viewer", password_hash="unused")
        session.add(owner)
        session.add(viewer)
        session.commit()
        session.refresh(owner)
        session.refresh(viewer)
        ks = KnowledgeSystem(name="Example", public_id="example", owner_id=owner.id)
        session.add(ks)
        session.commit()
        session.refresh(ks)
        session.add(KSGrant(knowledge_system_id=ks.id, user_id=viewer.id, role="viewer"))
        session.commit()

        created = token_api.create_mcp_token(token_api.CreateMcpToken(), ks, owner, session)
        assert created["token"].startswith("opm_example_")
        assert created["scopes"] == ["mcp:read", "mcp:write", "mcp:manage"]

        with pytest.raises(HTTPException) as denied:
            token_api.create_mcp_token(
                token_api.CreateMcpToken(scopes=["mcp:read", "mcp:write"]),
                ks,
                viewer,
                session,
            )
        assert denied.value.status_code == 403

        revoked = token_api.revoke_mcp_token(created["id"], ks, owner, session)
        assert revoked["status"] == "revoked"


def test_preview_is_clean_and_apply_uses_user_identity(monkeypatch) -> None:
    database = _database()
    monkeypatch.setattr(mcp_server, "engine", database)
    monkeypatch.setattr(store, "_store", Store())
    store._graph_locks.clear()
    store._recorders.clear()

    with Session(database) as session:
        owner = User(username="agent-owner", password_hash="unused")
        session.add(owner)
        session.commit()
        session.refresh(owner)
        ks = KnowledgeSystem(
            name="Example",
            public_id="example",
            owner_id=owner.id,
            graph_iri="urn:example:tbox",
            base_iri="urn:example:onto:",
        )
        session.add(ks)
        session.commit()
        session.refresh(ks)
        plaintext = mcp_tokens.mint(ks.public_id)
        token = McpUserToken(
            knowledge_system_id=ks.id,
            user_id=owner.id,
            name="test",
            token_prefix=plaintext[:24],
            token_hash=mcp_tokens.digest(plaintext),
            scopes=["mcp:read", "mcp:write", "mcp:manage"],
            expires_at=utcnow() + timedelta(hours=1),
        )
        session.add(token)
        session.commit()
        session.refresh(token)
        token_id = token.id
        user_id = owner.id
        ks_id = ks.id
        graph_iri = ks.graph_iri
        token_scopes = list(token.scopes)
        token_expires_at = token.expires_at

    access = AccessToken(
        token=plaintext,
        client_id="test",
        scopes=token_scopes,
        expires_at=int(token_expires_at.timestamp()),
        claims={
            "token_id": token_id,
            "user_id": user_id,
            "knowledge_system_id": ks_id,
        },
    )
    context_token = auth_context_var.set(AuthenticatedUser(access))
    try:
        operation = {"op": "add_class", "label": "Pump", "comment": "A fluid machine"}
        preview = mcp_server.preview_ontology_changes([operation])
        assert preview["valid"] is True
        assert preview["diff"]["counts"]["tbox_added"] > 0
        assert schema.build_view(graph_iri)["classes"] == []

        applied = mcp_server.apply_ontology_changes([operation], "Accepted chat suggestion")
        assert applied["applied"] == 1
        classes = schema.build_view(graph_iri)["classes"]
        assert [item["label"] for item in classes] == ["Pump"]
        with Session(database) as session:
            event = session.get(mcp_server.AuditEvent, applied["audit_event_id"])
            assert event is not None
            assert event.actor_name == "agent-owner"
            assert event.detail["reason"] == "Accepted chat suggestion"
    finally:
        auth_context_var.reset(context_token)


def test_streamable_http_lists_authenticated_tools(monkeypatch) -> None:
    database = _database()
    monkeypatch.setattr(mcp_server, "engine", database)
    with Session(database) as session:
        owner = User(username="protocol-owner", password_hash="unused")
        session.add(owner)
        session.commit()
        session.refresh(owner)
        ks = KnowledgeSystem(name="Protocol", public_id="protocol", owner_id=owner.id)
        session.add(ks)
        session.commit()
        session.refresh(ks)
        plaintext = mcp_tokens.mint(ks.public_id)
        session.add(McpUserToken(
            knowledge_system_id=ks.id,
            user_id=owner.id,
            name="protocol",
            token_prefix=plaintext[:24],
            token_hash=mcp_tokens.digest(plaintext),
            scopes=["mcp:read", "mcp:write", "mcp:manage"],
            expires_at=utcnow() + timedelta(hours=1),
        ))
        session.commit()

    @asynccontextmanager
    async def lifespan(_app):
        async with mcp_server.mcp.session_manager.run():
            yield

    app = FastAPI(lifespan=lifespan)
    app.mount("/", mcp_server.mcp_app)
    headers = {
        "Host": "localhost",
        "Authorization": f"Bearer {plaintext}",
        "Accept": "application/json, text/event-stream",
        "Content-Type": "application/json",
    }
    initialize = {
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "test", "version": "1"},
        },
    }
    with TestClient(app) as client:
        assert client.post("/mcp", headers=headers, json=initialize).status_code == 200
        response = client.post(
            "/mcp",
            headers={**headers, "MCP-Protocol-Version": "2025-06-18"},
            json={"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}},
        )
    assert response.status_code == 200
    names = {item["name"] for item in response.json()["result"]["tools"]}
    assert {"get_ontology", "preview_ontology_changes", "apply_ontology_changes"} <= names
    assert len(names) == 20
