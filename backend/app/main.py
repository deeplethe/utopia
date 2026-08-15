"""OntoPilot FastAPI application entry point."""
from __future__ import annotations

import logging
from contextlib import asynccontextmanager

from fastapi import FastAPI, Request
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import JSONResponse

from app.api import (
    abox, agent, auth, conflicts, documents, external, extraction, history, knowledge, ontology, providers, published,
    prompts, rdf_import, releases, resolution, settings_api, tokens, vocabulary,
)
from app.api import mcp_tokens as mcp_tokens_api
from app.config import settings
from app.db.database import init_db
from app.mcp_server import mcp, mcp_app
from app.ontology.store import CaptureBusy

logging.basicConfig(level=logging.INFO)


def _bootstrap_admin() -> None:
    """Seed the first admin from settings when the user table is empty, then backfill
    ownership so pre-auth knowledge systems (owner_id=NULL) belong to the first admin —
    otherwise regular users would be locked out and only admins (who see all) could reach them."""
    from sqlmodel import Session, select

    from app.db.database import engine
    from app.db.models import KnowledgeSystem, User
    from app.security import hash_password, validate_new_password

    with Session(engine) as session:
        admin = session.exec(select(User).where(User.is_admin == True)).first()  # noqa: E712
        if admin is None and session.exec(select(User)).first() is None:
            try:
                validate_new_password(settings.admin_password, bootstrap=True)
            except ValueError as exc:
                raise RuntimeError(
                    "An empty installation requires a strong ADMIN_PASSWORD in backend/.env "
                    f"before the first start: {exc}"
                ) from exc
            admin = User(
                username=settings.admin_username,
                password_hash=hash_password(settings.admin_password),
                is_admin=True,
            )
            session.add(admin)
            session.commit()
            session.refresh(admin)
            logging.info("seeded initial admin user %r", settings.admin_username)

        if admin is not None:
            orphans = session.exec(
                select(KnowledgeSystem).where(KnowledgeSystem.owner_id == None)  # noqa: E711
            ).all()
            if orphans:
                for ks in orphans:
                    ks.owner_id = admin.id
                    session.add(ks)
                session.commit()
                logging.warning("assigned %d owner-less knowledge system(s) to admin %r", len(orphans), admin.username)


def _seed_demo_data() -> None:
    if not settings.seed_demo_data:
        return
    from sqlmodel import Session, select

    from app.db.database import engine
    from app.db.models import User
    from app.demo import seed

    with Session(engine) as session:
        owner = session.exec(select(User).where(User.is_admin == True)).first()  # noqa: E712
        if owner:
            try:
                seed(session, owner)
            except ValueError:
                return


def _backfill_document_ks() -> None:
    """Bind pre-existing documents (knowledge_system_id=NULL) to a KS.

    Ownership is inferred from provenance: a doc is bound to the KS its chunks contributed
    the most axioms to. Docs that never produced any axioms (uploaded but not extracted)
    fall back to the earliest KS so they remain visible; if no KS exists they stay unbound."""
    from collections import Counter

    from sqlmodel import Session, select

    from app.db.database import engine
    from app.db.models import AxiomProvenance, Chunk, Document, KnowledgeSystem

    with Session(engine) as session:
        unbound = session.exec(select(Document).where(Document.knowledge_system_id == None)).all()  # noqa: E711
        if not unbound:
            return
        first_ks = session.exec(select(KnowledgeSystem).order_by(KnowledgeSystem.id)).first()
        bound = 0
        for doc in unbound:
            chunk_ids = [c.id for c in session.exec(select(Chunk).where(Chunk.document_id == doc.id)).all()]
            ks_id = None
            if chunk_ids:
                prov = session.exec(
                    select(AxiomProvenance).where(AxiomProvenance.chunk_id.in_(chunk_ids))
                ).all()
                if prov:
                    ks_id = Counter(p.knowledge_system_id for p in prov).most_common(1)[0][0]
            if ks_id is None and first_ks is not None:
                ks_id = first_ks.id
            if ks_id is not None:
                doc.knowledge_system_id = ks_id
                session.add(doc)
                bound += 1
        if bound:
            session.commit()
            logging.warning("bound %d pre-existing document(s) to a knowledge system", bound)


def _reset_stale_jobs() -> None:
    """Mark extraction jobs left 'pending'/'running' by a previous process (crash/restart)
    as failed. Otherwise extraction_active() would report them forever and permanently lock
    every mutating endpoint for that KS."""
    from sqlmodel import Session, select

    from app.db.database import engine
    from app.db.models import (
        AgentConversation,
        AgentTurn,
        ExportJob,
        ExtractionJob,
        OntologyRelease,
        ReleaseDeployment,
        utcnow,
    )

    with Session(engine) as session:
        stale_agent_turns = session.exec(
            select(AgentTurn).where(AgentTurn.status == "running")
        ).all()
        stale_conversation_ids: set[int] = set()
        for turn in stale_agent_turns:
            now = utcnow()
            turn.status = "failed"
            turn.error = "Interrupted by a server restart"
            turn.updated_at = now
            stale_conversation_ids.add(turn.conversation_id)
            session.add(turn)
        for conversation_id in stale_conversation_ids:
            conversation = session.get(AgentConversation, conversation_id)
            if conversation is not None:
                conversation.updated_at = utcnow()
                session.add(conversation)
        if stale_agent_turns:
            session.commit()
            logging.warning(
                "reset %d stale Agent turn(s) left running by a previous process",
                len(stale_agent_turns),
            )
        stale = session.exec(
            select(ExtractionJob).where(ExtractionJob.status.in_(("pending", "running")))
        ).all()
        for job in stale:
            job.status = "failed"
            job.error = "Interrupted by a server restart"
            job.finished_at = utcnow()
            session.add(job)
        if stale:
            session.commit()
            logging.warning("reset %d stale extraction job(s) left running by a previous process", len(stale))
        stale_exports = session.exec(
            select(ExportJob).where(ExportJob.status.in_(("pending", "running")))
        ).all()
        for job in stale_exports:
            job.status = "failed"
            job.error = "Interrupted by a server restart"
            job.finished_at = utcnow()
            session.add(job)
        releases = session.exec(select(OntologyRelease)).all()
        changed_releases = 0
        for release in releases:
            if release.manifest.get("capture_status") in {"pending", "running"}:
                release.manifest = {
                    "capture_status": "failed",
                    "error": "Interrupted by a server restart",
                }
                session.add(release)
                changed_releases += 1
        if stale_exports or changed_releases:
            session.commit()
        stale_deployments = session.exec(
            select(ReleaseDeployment).where(
                ReleaseDeployment.status.in_(("provisioning", "stopping"))
            )
        ).all()
        for deployment in stale_deployments:
            deployment.status = "failed" if deployment.status == "provisioning" else "stopped"
            deployment.error = "Interrupted by a server restart"
            deployment.stopped_at = utcnow()
            session.add(deployment)
        if stale_deployments:
            session.commit()


def _backfill_terminology() -> None:
    """Bring existing ontologies onto the automatic controlled-terminology model."""
    if not settings.automatic_terminology:
        return
    from sqlmodel import Session, select

    from app import audit
    from app.db.database import engine
    from app.db.models import KnowledgeSystem
    from app.ontology import skos, store, terminology_sync

    with Session(engine) as session:
        for ks in session.exec(select(KnowledgeSystem).order_by(KnowledgeSystem.id)).all():
            graph_iri = skos.graph_iri_for(ks)
            try:
                with store.capture(graph_iri, revert_on_error=True) as capture:
                    result = terminology_sync.sync_from_ontology(ks)
                added, removed = capture.diff()
                if added or removed:
                    audit.record(
                        session,
                        ks_id=ks.id,
                        action="terminology.sync",
                        summary=(
                            "Backfilled controlled terminology from the ontology: "
                            f"+{result['terms_added']} terms / {result['terms_mapped']} mappings"
                        ),
                        actor_name="system",
                        detail={"startup_backfill": True, **result},
                        added=added,
                        removed=removed,
                        graph=graph_iri,
                    )
            except Exception:  # noqa: BLE001
                session.rollback()
                logging.exception("failed to backfill terminology for knowledge system %s", ks.id)


@asynccontextmanager
async def lifespan(app: FastAPI):
    settings.ensure_dirs()
    init_db()
    # Load the runtime LLM connection/model config (base_url / api_key / model) from the DB over .env.
    from sqlmodel import Session as _Session

    from app import model_config
    from app.db.database import engine as _engine
    with _Session(_engine) as _s:
        model_config.seed_default_provider(_s)  # upgrade: fold legacy/.env connection into a Provider
        model_config.refresh_runtime(_s)
    _bootstrap_admin()
    _seed_demo_data()
    _backfill_document_ks()
    _reset_stale_jobs()
    with _Session(_engine) as _s:
        migrated_drafts = releases.normalize_unpublished_versions(_s)
        if migrated_drafts:
            logging.info("migrated %d unpublished release version(s) to draft identifiers", migrated_drafts)
    # Open the Oxigraph store eagerly so startup fails fast if the dir is locked.
    from app.ontology import release_service, store

    store.get_store()
    release_service.get_store()
    release_service.cleanup_inactive()
    _backfill_terminology()
    async with mcp.session_manager.run():
        yield


app = FastAPI(
    title="OntoPilot API",
    version="0.1.0",
    lifespan=lifespan,
    docs_url="/api/docs",
    redoc_url="/api/redoc",
    openapi_url="/api/openapi.json",
)

app.add_middleware(
    CORSMiddleware,
    allow_origins=settings.cors_origins,
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)

app.include_router(auth.router)
app.include_router(documents.router)
app.include_router(knowledge.router)
app.include_router(ontology.router)
app.include_router(extraction.router)
app.include_router(conflicts.router)
app.include_router(history.router)
app.include_router(abox.router)
app.include_router(resolution.router)
app.include_router(settings_api.router)
app.include_router(providers.router)
app.include_router(tokens.router)
app.include_router(external.router)
app.include_router(published.router)
app.include_router(rdf_import.router)
app.include_router(vocabulary.router)
app.include_router(prompts.router)
app.include_router(releases.router)
app.include_router(mcp_tokens_api.router)
app.include_router(agent.router)


@app.exception_handler(CaptureBusy)
async def _capture_busy_handler(_: Request, exc: CaptureBusy) -> JSONResponse:
    """A concurrent write to the same knowledge system is in progress (e.g. an extraction is
    running). Reject fast with 409 rather than block or corrupt the rollback record."""
    return JSONResponse(
        status_code=409,
        content={"detail": "This knowledge system is being modified by another operation. Please retry in a moment."},
    )


@app.get("/api/health")
def health() -> dict:
    return {
        "status": "ok",
        "system_language": settings.system_language,
        "extract_model": settings.llm_extract_model,
        "has_llm_key": bool(settings.openrouter_api_key),
    }


# Keep the MCP ASGI app last: its exact `/mcp` route remains available without a redirect while
# all FastAPI governance/API routes above retain normal routing and OpenAPI generation.
app.mount("/", mcp_app, name="mcp")
