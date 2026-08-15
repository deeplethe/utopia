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
from app.db.models import Conflict, KSGrant, KnowledgeSystem, McpUserToken, User, utcnow
from app.ontology import editor, schema, store


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
        ontology = mcp_server.get_ontology()
        assert ontology["revision"].startswith("sha256:")

        operation = {"op": "add_class", "label": "Pump", "comment": "A fluid machine"}
        preview = mcp_server.preview_ontology_changes([operation], ontology["revision"])
        assert preview["valid"] is True
        assert preview["dry_run"] is True
        assert preview["base_revision"]
        assert preview["revision"] != preview["base_revision"]
        assert preview["diff"]["counts"]["tbox_added"] > 0
        assert preview["diff"]["tbox_added"]

        compact_preview = mcp_server.preview_ontology_changes(
            [operation],
            ontology["revision"],
            include_rdf_diff=False,
        )
        assert compact_preview["diff"]["counts"] == preview["diff"]["counts"]
        assert compact_preview["diff"]["tbox_added"] == ""
        assert compact_preview["diff"]["tbox_removed"] == ""
        assert compact_preview["diff"]["abox_added"] == ""
        assert compact_preview["diff"]["abox_removed"] == ""
        assert compact_preview["impact"] == preview["impact"]
        assert compact_preview["structural_validation"] == preview["structural_validation"]
        assert schema.build_view(graph_iri)["classes"] == []

        applied = mcp_server.apply_ontology_changes(
            [operation],
            "Accepted chat suggestion",
            preview["base_revision"],
        )
        assert applied["applied"] == 1
        assert applied["base_revision"] == preview["base_revision"]
        assert applied["revision"] == preview["revision"]
        classes = schema.build_view(graph_iri)["classes"]
        assert [item["label"] for item in classes] == ["Pump"]
        with Session(database) as session:
            event = session.get(mcp_server.AuditEvent, applied["audit_event_id"])
            assert event is not None
            assert event.actor_name == "agent-owner"
            assert event.detail["reason"] == "Accepted chat suggestion"
            assert event.detail["base_revision"] == preview["base_revision"]

        stale_operation = {"op": "add_class", "label": "Valve"}
        with pytest.raises(mcp_server.ToolError, match="ontology_revision_conflict"):
            mcp_server.apply_ontology_changes(
                [stale_operation],
                "Apply an outdated preview",
                preview["base_revision"],
            )
        assert [item["label"] for item in schema.build_view(graph_iri)["classes"]] == ["Pump"]

        # A proposal prepared from get_ontology must also be rejected at preview time
        # when another writer changes the graph in between.  Otherwise an agent could
        # unknowingly reinterpret stale entity references against a newer workspace.
        stale_read = mcp_server.get_ontology()
        editor.apply_edit(graph_iri, "urn:example:onto:", {"op": "add_class", "label": "Valve"})
        with pytest.raises(mcp_server.ToolError, match="ontology_revision_conflict"):
            mcp_server.preview_ontology_changes(
                [{"op": "add_class", "label": "Motor"}],
                stale_read["revision"],
            )
        assert [item["label"] for item in schema.build_view(graph_iri)["classes"]] == [
            "Pump",
            "Valve",
        ]
    finally:
        auth_context_var.reset(context_token)


def test_list_conflicts_searches_title_and_detail(monkeypatch) -> None:
    database = _database()
    monkeypatch.setattr(mcp_server, "engine", database)

    with Session(database) as session:
        owner = User(username="review-owner", password_hash="unused")
        session.add(owner)
        session.commit()
        session.refresh(owner)
        ks = KnowledgeSystem(name="Review", public_id="review", owner_id=owner.id)
        session.add(ks)
        session.commit()
        session.refresh(ks)
        plaintext = mcp_tokens.mint(ks.public_id)
        token = McpUserToken(
            knowledge_system_id=ks.id,
            user_id=owner.id,
            name="review",
            token_prefix=plaintext[:24],
            token_hash=mcp_tokens.digest(plaintext),
            scopes=["mcp:read"],
            expires_at=utcnow() + timedelta(hours=1),
        )
        matching = Conflict(
            knowledge_system_id=ks.id,
            signature="range-pressure",
            ctype="range_multi",
            title="Range issue",
            detail="Pump pressure evidence is inconsistent",
        )
        other = Conflict(
            knowledge_system_id=ks.id,
            signature="domain-temperature",
            ctype="domain_multi",
            title="Domain issue",
            detail="Temperature evidence needs review",
        )
        session.add(token)
        session.add(matching)
        session.add(other)
        session.commit()
        session.refresh(token)
        session.refresh(matching)

        token_id = token.id
        matching_id = matching.id
        user_id = owner.id
        ks_id = ks.id
        token_expires_at = token.expires_at

    access = AccessToken(
        token=plaintext,
        client_id="test",
        scopes=["mcp:read"],
        expires_at=int(token_expires_at.timestamp()),
        claims={
            "token_id": token_id,
            "user_id": user_id,
            "knowledge_system_id": ks_id,
        },
    )
    context_token = auth_context_var.set(AuthenticatedUser(access))
    try:
        detail_result = mcp_server.list_review_items(
            queue="conflicts",
            status="open",
            query="pressure evidence",
        )
        assert detail_result["total"] == 1
        assert detail_result["items"][0]["id"] == matching_id

        title_result = mcp_server.list_review_items(
            queue="conflicts",
            status="open",
            query="RANGE ISSUE",
        )
        assert title_result["total"] == 1
        assert title_result["items"][0]["id"] == matching_id
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
    tools = response.json()["result"]["tools"]
    names = {item["name"] for item in tools}
    assert {
        "get_ontology",
        "get_ontology_neighborhood",
        "preview_ontology_changes",
        "apply_ontology_changes",
    } <= names
    assert "get_conflict_context" in names
    assert "get_conflicts_context" in names
    assert len(names) == 23
    preview_tool = next(item for item in tools if item["name"] == "preview_ontology_changes")
    review_tool = next(item for item in tools if item["name"] == "list_review_items")
    apply_tool = next(item for item in tools if item["name"] == "apply_ontology_changes")
    assert set(review_tool["inputSchema"]["properties"]["queue"]["enum"]) == {
        "conflicts", "entity_resolution", "terminology", "validation",
    }
    assert "expected_revision" in preview_tool["inputSchema"]["required"]
    assert "include_rdf_diff" not in preview_tool["inputSchema"]["required"]
    assert preview_tool["inputSchema"]["properties"]["include_rdf_diff"]["default"] is True
    assert "expected_revision" in apply_tool["inputSchema"]["required"]
