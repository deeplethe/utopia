"""Authenticated MCP server mounted into the OntoPilot FastAPI process.

The transport starts with the normal backend. Every bearer token is bound to one user and one
knowledge system; tools re-check the live membership role before reading or changing anything.
"""
from __future__ import annotations

import asyncio
import json
import secrets
import threading
from contextlib import contextmanager
from datetime import datetime, timedelta
from typing import Any
from urllib.parse import urlparse

from fastapi import BackgroundTasks, HTTPException
from fastapi.encoders import jsonable_encoder
from mcp.server.auth.middleware.auth_context import get_access_token
from mcp.server.auth.provider import AccessToken
from mcp.server.auth.settings import AuthSettings
from mcp.server.fastmcp import FastMCP
from mcp.server.fastmcp.exceptions import ToolError
from mcp.server.transport_security import TransportSecuritySettings
from mcp.types import ToolAnnotations
from pyoxigraph import NamedNode, QueryBoolean, QuerySolutions
from sqlalchemy import func
from sqlmodel import Session, select

from app import audit, mcp_tokens
from app.api import abox as abox_api
from app.api import conflicts as conflicts_api
from app.api import extraction as extraction_api
from app.api import history as history_api
from app.api import releases as releases_api
from app.api import resolution as resolution_api
from app.api import vocabulary as vocabulary_api
from app.api.external import _FORBIDDEN_SPARQL, _query_form, _sparql_code, _term_json
from app.api.knowledge import refresh_ks_stats
from app.api.ontology import _edit_summary
from app.config import settings
from app.db.database import engine
from app.db.models import (
    AuditEvent,
    Conflict,
    Document,
    EntityResolution,
    KnowledgeSystem,
    McpUserToken,
    OntologyRelease,
    TermProposal,
    User,
    utcnow,
)
from app.ontology import abox, abox_validate, editor, schema, skos, statement_provenance, store, vocab
from app.permissions import effective_role, extraction_active

_ROLE_RANK = {"viewer": 1, "editor": 2, "owner": 3}
_DESTRUCTIVE_ONTOLOGY_OPS = {
    "delete_class",
    "delete_property",
    "delete_axiom",
    "merge_classes",
    "merge_properties",
}


class OntoPilotTokenVerifier:
    """Verify hashed MCP user tokens and return identity claims to the tool context."""

    async def verify_token(self, plaintext: str) -> AccessToken | None:
        with Session(engine) as session:
            row = session.exec(
                select(McpUserToken).where(McpUserToken.token_hash == mcp_tokens.digest(plaintext))
            ).first()
            if row is None or mcp_tokens.status(row) != "active":
                return None
            user = session.get(User, row.user_id)
            ks = session.get(KnowledgeSystem, row.knowledge_system_id)
            if user is None or not user.active or ks is None:
                return None
            role = effective_role(session, ks, user)
            if role is None or "mcp:read" not in row.scopes:
                return None
            now = utcnow()
            if row.last_used_at is None or mcp_tokens.aware(row.last_used_at) < now - timedelta(minutes=5):
                row.last_used_at = now
                session.add(row)
                session.commit()
            return AccessToken(
                token=plaintext,
                client_id=f"ontopilot-user-{user.id}",
                scopes=row.scopes,
                expires_at=int(mcp_tokens.aware(row.expires_at).timestamp()),
                resource=settings.mcp_public_url,
                subject=str(user.id),
                claims={
                    "token_id": row.id,
                    "user_id": user.id,
                    "knowledge_system_id": ks.id,
                },
            )


@contextmanager
def _principal(required_scope: str = "mcp:read", min_role: str = "viewer"):
    access = get_access_token()
    claims = access.claims if access else None
    if access is None or not claims:
        raise ToolError("A valid OntoPilot MCP user token is required")
    with Session(engine, expire_on_commit=False) as session:
        row = session.get(McpUserToken, claims.get("token_id"))
        user = session.get(User, claims.get("user_id"))
        ks = session.get(KnowledgeSystem, claims.get("knowledge_system_id"))
        if (
            row is None
            or user is None
            or ks is None
            or not user.active
            or row.user_id != user.id
            or row.knowledge_system_id != ks.id
            or mcp_tokens.status(row) != "active"
        ):
            raise ToolError("The MCP user token is expired, revoked, or no longer valid")
        if required_scope not in row.scopes:
            raise ToolError(f'The MCP token lacks required scope "{required_scope}"')
        role = effective_role(session, ks, user)
        if role is None or _ROLE_RANK[role] < _ROLE_RANK[min_role]:
            raise ToolError(f"This operation requires the {min_role} role")
        yield session, user, ks, role, row


def _call(function, *args, **kwargs):
    try:
        return function(*args, **kwargs)
    except HTTPException as exc:
        detail = exc.detail
        if isinstance(detail, (dict, list)):
            detail = json.dumps(detail, ensure_ascii=False)
        raise ToolError(str(detail)) from exc
    except ToolError:
        raise
    except Exception as exc:  # noqa: BLE001
        raise ToolError(str(exc)) from exc


def _json(value: Any) -> Any:
    return jsonable_encoder(value)


def _bounded_limit(value: int, maximum: int) -> int:
    if value < 1 or value > maximum:
        raise ToolError(f"limit must be between 1 and {maximum}")
    return value


def _operations(value: list[dict[str, Any]]) -> list[dict[str, Any]]:
    if not value or len(value) > 50:
        raise ToolError("operations must contain between 1 and 50 edits")
    if len(json.dumps(value, ensure_ascii=False)) > 200_000:
        raise ToolError("operations payload is too large")
    for index, operation in enumerate(value):
        if not isinstance(operation, dict) or not operation.get("op"):
            raise ToolError(f"operations[{index}] must contain an op")
    return value


def _background(background: BackgroundTasks) -> None:
    """Run FastAPI background tasks without holding the MCP response open."""

    if not background.tasks:
        return

    def runner() -> None:
        asyncio.run(background())

    threading.Thread(target=runner, name="ontopilot-mcp-background", daemon=True).start()


_resource_url = settings.mcp_public_url.rstrip("/")
_issuer_url = _resource_url.rsplit("/mcp", 1)[0] or _resource_url
_public = urlparse(_resource_url)
_public_host = _public.hostname or "localhost"
_allowed_hosts = list(dict.fromkeys([
    _public.netloc,
    _public_host,
    f"{_public_host}:*",
    "localhost:*",
    "127.0.0.1:*",
    "[::1]:*",
]))
_allowed_origins = list(dict.fromkeys([
    f"{_public.scheme}://{_public.netloc}",
    f"{_public.scheme}://{_public_host}:*",
    "http://localhost:*",
    "http://127.0.0.1:*",
    "http://[::1]:*",
]))
mcp = FastMCP(
    "OntoPilot",
    instructions=(
        "Operate the knowledge system bound to the bearer token. Read evidence before proposing "
        "changes, call preview_ontology_changes before apply_ontology_changes, and never represent "
        "a mutable workspace edit as a published release. Destructive and lifecycle tools require "
        "explicit confirmation."
    ),
    token_verifier=OntoPilotTokenVerifier(),
    auth=AuthSettings(
        issuer_url=_issuer_url,
        resource_server_url=_resource_url,
        required_scopes=["mcp:read"],
    ),
    streamable_http_path="/mcp",
    stateless_http=True,
    json_response=True,
    max_request_body_size=1024 * 1024,
    transport_security=TransportSecuritySettings(
        enable_dns_rebinding_protection=True,
        allowed_hosts=_allowed_hosts,
        allowed_origins=_allowed_origins,
    ),
)


@mcp.tool(
    annotations=ToolAnnotations(readOnlyHint=True, destructiveHint=False, idempotentHint=True),
    structured_output=True,
)
def get_workspace_context() -> dict[str, Any]:
    """Get the bound workspace, current user role, graph statistics, and governance blockers."""

    with _principal() as (session, user, ks, role, token):
        validation = abox_validate.validate(ks.graph_iri, abox_api.abox_iri_for(ks))
        validation_items = validation.get("violations") or validation.get("items") or []
        counts = {
            "open_conflicts": session.exec(
                select(func.count(Conflict.id)).where(
                    Conflict.knowledge_system_id == ks.id, Conflict.status == "open"
                )
            ).one(),
            "pending_entity_resolution": session.exec(
                select(func.count(EntityResolution.id)).where(
                    EntityResolution.knowledge_system_id == ks.id,
                    EntityResolution.status == "pending",
                )
            ).one(),
            "pending_terminology": session.exec(
                select(func.count(TermProposal.id)).where(
                    TermProposal.knowledge_system_id == ks.id, TermProposal.status == "pending"
                )
            ).one(),
            "validation_violations": len(validation_items),
        }
        return {
            "knowledge_system": {
                "id": ks.id,
                "public_id": ks.public_id,
                "name": ks.name,
                "description": ks.description,
                "base_iri": ks.base_iri,
                "stats": {
                    "classes": ks.class_count,
                    "properties": ks.property_count,
                    "axioms": ks.axiom_count,
                },
            },
            "actor": {"id": user.id, "username": user.username, "role": role},
            "token_scopes": token.scopes,
            "review_counts": counts,
            "extraction_active": extraction_active(session, ks.id),
        }


@mcp.tool(
    annotations=ToolAnnotations(readOnlyHint=True, destructiveHint=False, idempotentHint=True),
    structured_output=True,
)
def get_ontology() -> dict[str, Any]:
    """Read the current mutable TBox as structured classes, properties, axioms, and labels."""

    with _principal() as (_, _user, ks, _role, _token):
        view = schema.build_view(ks.graph_iri)
        view["knowledge_system"] = {"id": ks.id, "name": ks.name, "base_iri": ks.base_iri}
        return _json(view)


@mcp.tool(
    annotations=ToolAnnotations(readOnlyHint=True, destructiveHint=False, idempotentHint=True),
    structured_output=True,
)
def search_ontology(query: str, limit: int = 25) -> dict[str, Any]:
    """Search TBox classes and properties by label, IRI, or description."""

    q = query.strip().casefold()
    if not q:
        raise ToolError("query is required")
    limit = _bounded_limit(limit, 100)
    with _principal() as (_, _user, ks, _role, _token):
        view = schema.build_view(ks.graph_iri)
        hits = []
        for kind, key in (
            ("class", "classes"),
            ("object_property", "object_properties"),
            ("data_property", "data_properties"),
        ):
            for item in view.get(key, []):
                haystack = " ".join(
                    str(item.get(field) or "") for field in ("label", "iri", "comment", "description")
                ).casefold()
                if q in haystack:
                    hits.append({"kind": kind, **item})
        return {"items": _json(hits[:limit]), "total": len(hits), "truncated": len(hits) > limit}


@mcp.tool(
    annotations=ToolAnnotations(readOnlyHint=True, destructiveHint=False, idempotentHint=True),
    structured_output=True,
)
def list_documents(limit: int = 100, offset: int = 0) -> dict[str, Any]:
    """List source documents and their parsing/extraction state for evidence planning."""

    limit = _bounded_limit(limit, 500)
    if offset < 0:
        raise ToolError("offset cannot be negative")
    with _principal() as (session, _user, ks, _role, _token):
        total = session.exec(
            select(func.count(Document.id)).where(Document.knowledge_system_id == ks.id)
        ).one()
        rows = session.exec(
            select(Document).where(Document.knowledge_system_id == ks.id)
            .order_by(Document.uploaded_at.desc()).limit(limit).offset(offset)
        ).all()
        return {"items": _json(rows), "total": total}


@mcp.tool(
    annotations=ToolAnnotations(readOnlyHint=True, destructiveHint=False, idempotentHint=True),
    structured_output=True,
)
def list_vocabulary_concepts(
    query: str | None = None,
    scheme_iri: str | None = None,
    status: str | None = None,
    limit: int = 100,
    offset: int = 0,
) -> dict[str, Any]:
    """Browse and search SKOS concepts in the mutable workspace vocabulary."""

    limit = _bounded_limit(limit, 1000)
    if offset < 0:
        raise ToolError("offset cannot be negative")
    with _principal() as (_, _user, ks, _role, _token):
        return _json(
            skos.list_concepts(
                skos.graph_iri_for(ks),
                scheme_iri=scheme_iri,
                q=query,
                status=status,
                limit=limit,
                offset=offset,
            )
        )


@mcp.tool(
    annotations=ToolAnnotations(readOnlyHint=True, destructiveHint=False, idempotentHint=True),
    structured_output=True,
)
def resolve_term(query: str, language: str | None = None, limit: int = 10) -> dict[str, Any]:
    """Resolve a preferred, alternative, or hidden SKOS label to controlled concepts."""

    if not query.strip():
        raise ToolError("query is required")
    limit = _bounded_limit(limit, 100)
    with _principal() as (_, _user, ks, _role, _token):
        return _json(skos.resolve(skos.graph_iri_for(ks), query, language=language, limit=limit))


@mcp.tool(
    annotations=ToolAnnotations(readOnlyHint=True, destructiveHint=False, idempotentHint=True),
    structured_output=True,
)
def list_individuals(
    class_iri: str | None = None,
    query: str | None = None,
    limit: int = 20,
    offset: int = 0,
) -> dict[str, Any]:
    """Search and paginate ABox individuals, optionally restricted to a class."""

    limit = _bounded_limit(limit, 200)
    if offset < 0:
        raise ToolError("offset cannot be negative")
    with _principal() as (_, _user, ks, _role, _token):
        class_labels, _ = abox_api._labels(ks)
        items, total = abox.list_individuals(
            abox_api.abox_iri_for(ks),
            class_labels,
            class_iri=class_iri,
            q=query,
            limit=limit,
            offset=offset,
        )
        return {"items": _json(items), "total": total}


@mcp.tool(
    annotations=ToolAnnotations(readOnlyHint=True, destructiveHint=False, idempotentHint=True),
    structured_output=True,
)
def get_individual(iri: str) -> dict[str, Any]:
    """Read one individual with types, assertions, and source evidence."""

    with _principal() as (session, _user, ks, _role, _token):
        return _json(_call(abox_api.get_individual, iri, ks, session))


@mcp.tool(
    annotations=ToolAnnotations(readOnlyHint=True, destructiveHint=False, idempotentHint=True),
    structured_output=True,
)
def query_knowledge(sparql: str, max_rows: int = 100) -> dict[str, Any]:
    """Run bounded read-only SPARQL SELECT or ASK over workspace TBox, ABox, and SKOS."""

    query = sparql.strip()
    if not query:
        raise ToolError("sparql is required")
    if len(query) > settings.external_query_max_chars:
        raise ToolError("SPARQL query is too large")
    if _query_form(query) not in {"SELECT", "ASK"}:
        raise ToolError("Only SPARQL SELECT and ASK queries are allowed")
    if _FORBIDDEN_SPARQL.search(_sparql_code(query)):
        raise ToolError("SERVICE, FROM, GRAPH, and update operations are not allowed")
    max_rows = _bounded_limit(max_rows, settings.external_query_max_rows)
    with _principal() as (_, _user, ks, _role, _token):
        graphs = [
            NamedNode(ks.graph_iri),
            NamedNode(abox_api.abox_iri_for(ks)),
            NamedNode(skos.graph_iri_for(ks)),
        ]
        try:
            result = store.get_store().query(
                query,
                base_iri=ks.base_iri,
                prefixes={
                    "rdf": vocab.RDF,
                    "rdfs": vocab.RDFS,
                    "owl": vocab.OWL,
                    "xsd": vocab.XSD,
                    "skos": skos.SKOS,
                    "dcterms": skos.DCTERMS,
                    "onto": ks.base_iri,
                },
                default_graph=graphs,
                named_graphs=[],
            )
        except (SyntaxError, ValueError, OSError) as exc:
            raise ToolError(f"Invalid SPARQL query: {exc}") from exc
        if isinstance(result, QueryBoolean):
            return {"head": {}, "boolean": bool(result)}
        if not isinstance(result, QuerySolutions):
            raise ToolError("Only SPARQL SELECT and ASK results are supported")
        variables = [variable.value for variable in result.variables]
        rows = []
        truncated = False
        for index, solution in enumerate(result):
            if index >= max_rows:
                truncated = True
                break
            rows.append({
                variable: _term_json(solution[variable])
                for variable in variables
                if solution[variable] is not None
            })
        return {
            "head": {"vars": variables},
            "results": {"bindings": rows},
            "truncated": truncated,
            "max_rows": max_rows,
        }


@mcp.tool(
    annotations=ToolAnnotations(readOnlyHint=True, destructiveHint=False, idempotentHint=True),
    structured_output=True,
)
def list_review_items(
    queue: str,
    status: str = "all",
    query: str | None = None,
    limit: int = 50,
    offset: int = 0,
) -> dict[str, Any]:
    """List conflict, entity-resolution, terminology, or validation review items."""

    limit = _bounded_limit(limit, 1000)
    if offset < 0:
        raise ToolError("offset cannot be negative")
    with _principal() as (session, _user, ks, _role, _token):
        if queue == "conflicts":
            items = _call(conflicts_api.list_conflicts, status, None, ks, session)
            if query:
                q = query.casefold()
                items = [item for item in items if q in f"{item.title} {item.description}".casefold()]
            return {"items": _json(items[offset:offset + limit]), "total": len(items)}
        if queue == "entity_resolution":
            if status in {"all", "pending"}:
                return _json(_call(resolution_api.get_queue, query, limit, offset, ks, session))
            return _json(
                _call(resolution_api.get_decisions, status, query, limit, offset, ks, session)
            )
        if queue == "terminology":
            return _json(
                _call(vocabulary_api.list_proposals, status, query, limit, offset, ks, session)
            )
        if queue == "validation":
            validation = abox_validate.validate(ks.graph_iri, abox_api.abox_iri_for(ks))
            items = validation.get("violations") or validation.get("items") or []
            if query:
                q = query.casefold()
                items = [item for item in items if q in json.dumps(item, ensure_ascii=False).casefold()]
            return {"items": _json(items[offset:offset + limit]), "total": len(items)}
        raise ToolError("queue must be conflicts, entity_resolution, terminology, or validation")


@mcp.tool(
    annotations=ToolAnnotations(readOnlyHint=True, destructiveHint=False, idempotentHint=True),
    structured_output=True,
)
def get_history(limit: int = 50, offset: int = 0) -> dict[str, Any]:
    """Read the audited change history for the bound knowledge system."""

    limit = _bounded_limit(limit, 500)
    if offset < 0:
        raise ToolError("offset cannot be negative")
    with _principal() as (session, _user, ks, _role, _token):
        return _json(_call(history_api.get_history, None, None, limit, offset, ks, session))


@mcp.tool(
    annotations=ToolAnnotations(readOnlyHint=True, destructiveHint=False, idempotentHint=True),
    structured_output=True,
)
def list_releases() -> dict[str, Any]:
    """List immutable release drafts, published versions, and deployment state."""

    with _principal() as (session, _user, ks, _role, _token):
        return _json(_call(releases_api.list_releases, ks, session))


@mcp.tool(
    annotations=ToolAnnotations(readOnlyHint=True, destructiveHint=False, idempotentHint=True),
    structured_output=True,
)
def preview_ontology_changes(operations: list[dict[str, Any]]) -> dict[str, Any]:
    """Validate a structured ontology change set and return its exact RDF diff without saving it."""

    operations = _operations(operations)
    with _principal() as (session, _user, ks, role, _token):
        if extraction_active(session, ks.id):
            raise ToolError("An extraction is in progress")
        abox_iri = abox_api.abox_iri_for(ks)
        results: list[str] = []
        added = removed = abox_added = abox_removed = b""
        try:
            with store.capture(ks.graph_iri, revert_on_error=True) as cap, store.capture(
                abox_iri, revert_on_error=True
            ) as acap:
                try:
                    results = [editor.apply_edit(ks.graph_iri, ks.base_iri, op) for op in operations]
                    added, removed = cap.diff()
                    abox_added, abox_removed = acap.diff()
                    resulting_stats = schema.build_view(ks.graph_iri).get("stats", {})
                finally:
                    acap.revert()
                    cap.revert()
        except (editor.EditError, KeyError, ValueError) as exc:
            raise ToolError(f"Change set is invalid: {exc}") from exc
        destructive = [op["op"] for op in operations if op["op"] in _DESTRUCTIVE_ONTOLOGY_OPS]
        return {
            "valid": True,
            "actor_role": role,
            "operations": operations,
            "results": results,
            "destructive_operations": destructive,
            "requires_confirmation": bool(destructive),
            "diff": {
                "tbox_added": added.decode("utf-8"),
                "tbox_removed": removed.decode("utf-8"),
                "abox_added": abox_added.decode("utf-8"),
                "abox_removed": abox_removed.decode("utf-8"),
                "counts": {
                    "tbox_added": len(store.load_triples(added)),
                    "tbox_removed": len(store.load_triples(removed)),
                    "abox_added": len(store.load_triples(abox_added)),
                    "abox_removed": len(store.load_triples(abox_removed)),
                },
            },
            "resulting_stats": resulting_stats,
        }


@mcp.tool(
    annotations=ToolAnnotations(readOnlyHint=False, destructiveHint=True, idempotentHint=False),
    structured_output=True,
)
def apply_ontology_changes(
    operations: list[dict[str, Any]],
    reason: str,
    confirm_destructive: bool = False,
) -> dict[str, Any]:
    """Atomically apply a previewed TBox change set to the mutable workspace and audit it."""

    operations = _operations(operations)
    reason = reason.strip()
    if not reason:
        raise ToolError("reason is required for audited modifications")
    destructive = [op["op"] for op in operations if op["op"] in _DESTRUCTIVE_ONTOLOGY_OPS]
    if destructive and not confirm_destructive:
        raise ToolError("confirm_destructive=true is required for delete or merge operations")
    with _principal("mcp:write", "editor") as (session, user, ks, _role, _token):
        if extraction_active(session, ks.id):
            raise ToolError("An extraction is in progress")
        abox_iri = abox_api.abox_iri_for(ks)
        try:
            with store.capture(ks.graph_iri, revert_on_error=True) as cap, store.capture(
                abox_iri, revert_on_error=True
            ) as acap:
                results = [editor.apply_edit(ks.graph_iri, ks.base_iri, op) for op in operations]
            added, removed = cap.diff()
            abox_added, abox_removed = acap.diff()
        except (editor.EditError, KeyError, ValueError) as exc:
            raise ToolError(f"Change set failed: {exc}") from exc
        refresh_ks_stats(session, ks)
        open_conflicts = conflicts_api.sync_conflicts(session, ks, semantic=False)
        group_id = secrets.token_hex(8)
        summary = (
            _edit_summary(operations[0])
            if len(operations) == 1
            else f"Applied {len(operations)} ontology edits through MCP"
        )
        event = audit.record(
            session,
            ks_id=ks.id,
            action="mcp.ontology.change",
            summary=summary,
            actor_id=user.id,
            actor_name=user.username,
            detail={"source": "mcp", "reason": reason, "operations": operations},
            added=added,
            removed=removed,
            group_id=group_id,
        )
        statement_provenance.record_tbox_diff(session, ks.id, added, removed, event)
        if abox_added or abox_removed:
            audit.record(
                session,
                ks_id=ks.id,
                action="mcp.ontology.change",
                summary=f"{summary} — cascaded to instances",
                actor_id=user.id,
                actor_name=user.username,
                detail={"source": "mcp", "reason": reason, "operations": operations},
                added=abox_added,
                removed=abox_removed,
                graph=abox_iri,
                group_id=group_id,
            )
        return {
            "applied": len(operations),
            "results": results,
            "audit_event_id": event.id,
            "open_conflicts": len(open_conflicts),
            "view": _json(schema.build_view(ks.graph_iri)),
        }


@mcp.tool(
    annotations=ToolAnnotations(readOnlyHint=False, destructiveHint=True, idempotentHint=False),
    structured_output=True,
)
def apply_instance_change(
    action: str,
    payload: dict[str, Any],
    confirm_destructive: bool = False,
) -> dict[str, Any]:
    """Create/delete an individual or add/remove an ABox assertion in the mutable workspace."""

    with _principal("mcp:write", "editor") as (session, user, ks, _role, _token):
        if action == "create_individual":
            body = abox_api.CreateIndividual.model_validate(payload)
            return _json(_call(abox_api.create_individual, body, ks, user, session))
        if action == "delete_individual":
            if not confirm_destructive:
                raise ToolError("confirm_destructive=true is required to delete an individual")
            body = abox_api.IndividualRef.model_validate(payload)
            return _json(_call(abox_api.delete_individual, body, ks, user, session))
        if action in {"add_assertion", "remove_assertion"}:
            body = abox_api.Assertion.model_validate(payload)
            if action == "remove_assertion" and not confirm_destructive:
                raise ToolError("confirm_destructive=true is required to remove an assertion")
            function = abox_api.add_assertion if action == "add_assertion" else abox_api.remove_assertion
            return _json(_call(function, body, ks, user, session))
        raise ToolError(
            "action must be create_individual, delete_individual, add_assertion, or remove_assertion"
        )


@mcp.tool(
    annotations=ToolAnnotations(readOnlyHint=False, destructiveHint=True, idempotentHint=False),
    structured_output=True,
)
def apply_vocabulary_change(
    action: str,
    payload: dict[str, Any] | None = None,
    iri: str | None = None,
    confirm_destructive: bool = False,
) -> dict[str, Any]:
    """Create, update, delete, or synchronize SKOS schemes and concepts."""

    payload = payload or {}
    with _principal("mcp:write", "editor") as (session, user, ks, _role, _token):
        if action == "create_scheme":
            return _json(_call(vocabulary_api.create_scheme, vocabulary_api.SchemeIn.model_validate(payload), ks, user, session))
        if action == "update_scheme":
            if not iri:
                raise ToolError("iri is required")
            return _json(_call(vocabulary_api.update_scheme, iri, vocabulary_api.SchemeIn.model_validate(payload), ks, user, session))
        if action == "delete_scheme":
            if not iri or not confirm_destructive:
                raise ToolError("iri and confirm_destructive=true are required")
            return _json(_call(vocabulary_api.delete_scheme, iri, ks, user, session))
        if action == "create_concept":
            return _json(_call(vocabulary_api.create_concept, vocabulary_api.ConceptIn.model_validate(payload), ks, user, session))
        if action == "update_concept":
            if not iri:
                raise ToolError("iri is required")
            return _json(_call(vocabulary_api.update_concept, iri, vocabulary_api.ConceptIn.model_validate(payload), ks, user, session))
        if action == "delete_concept":
            if not iri or not confirm_destructive:
                raise ToolError("iri and confirm_destructive=true are required")
            return _json(_call(vocabulary_api.delete_concept, iri, ks, user, session))
        if action == "sync_from_ontology":
            return _json(_call(vocabulary_api.sync_vocabulary, ks, user, session))
        raise ToolError(
            "action must be create_scheme, update_scheme, delete_scheme, create_concept, "
            "update_concept, delete_concept, or sync_from_ontology"
        )


@mcp.tool(
    annotations=ToolAnnotations(readOnlyHint=False, destructiveHint=True, idempotentHint=False),
    structured_output=True,
)
def decide_review_item(
    queue: str,
    item_id: int,
    action: str,
    payload: dict[str, Any] | None = None,
    confirm: bool = False,
) -> dict[str, Any]:
    """Resolve or dismiss a governance queue item using the same audited application services."""

    if not confirm:
        raise ToolError("confirm=true is required to apply a review decision")
    payload = payload or {}
    with _principal("mcp:write", "editor") as (session, user, ks, _role, _token):
        if queue == "conflicts":
            if action == "resolve":
                body = conflicts_api.ResolveRequest.model_validate(payload)
                return _json(_call(conflicts_api.resolve_conflict, item_id, body, ks, user, session))
            if action == "dismiss":
                return _json(_call(conflicts_api.dismiss_conflict, item_id, ks, user, session))
        elif queue == "entity_resolution" and action in {"match", "new"}:
            body = resolution_api.ResolveRequest(
                action=action,
                individual_iri=payload.get("individual_iri"),
            )
            return _json(_call(resolution_api.resolve, item_id, body, ks, user, session))
        elif queue == "terminology":
            body = vocabulary_api.ProposalDecision.model_validate(payload)
            if action == "accept":
                return _json(_call(vocabulary_api.accept_proposal, item_id, body, ks, user, session))
            if action == "reject":
                return _json(_call(vocabulary_api.reject_proposal, item_id, body, ks, user, session))
        elif queue == "validation" and action == "fix":
            body = abox_api.FixRequest.model_validate(payload)
            return _json(_call(abox_api.fix_violation, body, ks, user, session))
        raise ToolError("Unsupported queue/action combination")


@mcp.tool(
    annotations=ToolAnnotations(readOnlyHint=False, destructiveHint=False, idempotentHint=False),
    structured_output=True,
)
async def start_extraction(
    mode: str,
    chunk_ids: list[int],
    model: str | None = None,
    agentic_resolution: bool | None = None,
) -> dict[str, Any]:
    """Start TBox, ABox, or combined extraction for selected source chunks."""

    with _principal("mcp:write", "editor") as (session, user, ks, _role, _token):
        body = extraction_api.ExtractRequest(
            chunk_ids=chunk_ids,
            model=model,
            agentic_resolution=agentic_resolution,
        )
        functions = {
            "tbox": extraction_api.run_extraction,
            "abox": extraction_api.run_instance_extraction,
            "both": extraction_api.run_combined_extraction,
        }
        function = functions.get(mode)
        if function is None:
            raise ToolError("mode must be tbox, abox, or both")
        try:
            job = await function(body, ks, user, session)
        except HTTPException as exc:
            raise ToolError(str(exc.detail)) from exc
        return _json(job)


@mcp.tool(
    annotations=ToolAnnotations(readOnlyHint=False, destructiveHint=True, idempotentHint=False),
    structured_output=True,
)
def manage_release(
    action: str,
    release_id: int | None = None,
    payload: dict[str, Any] | None = None,
    confirm: bool = False,
) -> dict[str, Any]:
    """Create, review, publish, roll back, deploy, stop, or delete an immutable release."""

    payload = payload or {}
    with _principal("mcp:manage", "owner") as (session, user, ks, _role, _token):
        background = BackgroundTasks()
        if action == "create_draft":
            body = releases_api.CreateReleaseRequest.model_validate(payload)
            result = _call(releases_api.create_release, body, background, ks, user, session)
        elif action == "review" and release_id is not None:
            body = releases_api.ReviewReleaseRequest.model_validate(payload)
            result = _call(releases_api.review_release, release_id, body, ks, user, session)
        elif action == "publish" and release_id is not None:
            if not confirm:
                raise ToolError("confirm=true is required to publish a release")
            body = releases_api.PublishReleaseRequest.model_validate(payload)
            result = _call(releases_api.publish_release, release_id, body, background, ks, user, session)
        elif action == "rollback" and release_id is not None:
            if not confirm:
                raise ToolError("confirm=true is required to replace the workspace from a release")
            result = _call(releases_api.rollback_release, release_id, ks, user, session)
        elif action == "deploy" and release_id is not None:
            result = _call(releases_api.deploy_release_service, release_id, background, ks, user, session)
        elif action == "stop" and release_id is not None:
            if not confirm:
                raise ToolError("confirm=true is required to stop a release service")
            result = _call(releases_api.stop_release_service, release_id, background, ks, user, session)
        elif action == "delete" and release_id is not None:
            if not confirm:
                raise ToolError("confirm=true is required to delete a release")
            result = _call(releases_api.delete_release, release_id, background, ks, user, session)
        else:
            raise ToolError(
                "action must be create_draft, review, publish, rollback, deploy, stop, or delete"
            )
        _background(background)
        return _json(result)


@mcp.tool(
    annotations=ToolAnnotations(readOnlyHint=False, destructiveHint=True, idempotentHint=False),
    structured_output=True,
)
def rollback_history_event(event_id: int, confirm: bool = False) -> dict[str, Any]:
    """Reverse one audited workspace event. This is an owner-confirmed destructive action."""

    if not confirm:
        raise ToolError("confirm=true is required to roll back history")
    with _principal("mcp:manage", "owner") as (session, user, ks, _role, _token):
        return _json(_call(history_api.rollback, event_id, ks, user, session))


mcp_app = mcp.streamable_http_app()
