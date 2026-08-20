"""SQLite engine + session management."""
from __future__ import annotations

from collections.abc import Iterator
from uuid import uuid4

from sqlalchemy import event
from sqlmodel import Session, SQLModel, create_engine

from app.config import settings

# check_same_thread=False so the engine can be shared across FastAPI's threadpool;
# timeout gives SQLite a busy-wait window so background-writes + polling-reads don't
# collide with "database is locked".
engine = create_engine(
    settings.db_url,
    echo=False,
    connect_args={"check_same_thread": False, "timeout": 30}
    if settings.db_url.startswith("sqlite") else {},
    pool_pre_ping=True,
)


@event.listens_for(engine, "connect")
def _configure_sqlite(dbapi_connection, _connection_record) -> None:
    """Use WAL and an explicit busy timeout for concurrent extraction jobs."""
    if not settings.db_url.startswith("sqlite"):
        return
    cursor = dbapi_connection.cursor()
    try:
        cursor.execute("PRAGMA journal_mode=WAL")
        cursor.execute("PRAGMA synchronous=NORMAL")
        cursor.execute("PRAGMA busy_timeout=30000")
    finally:
        cursor.close()


def init_db() -> None:
    """Create tables. Import models so they are registered on SQLModel.metadata."""
    from app.db import models  # noqa: F401  (side-effect: registers tables)

    SQLModel.metadata.create_all(engine)
    _migrate()


def _migrate() -> None:
    """Small additive migrations for existing SQLite/PostgreSQL installations."""
    from sqlalchemy import text

    if not settings.db_url.startswith("sqlite"):
        if engine.dialect.name == "postgresql":
            with engine.begin() as conn:
                conn.execute(text(
                    "ALTER TABLE systemconfig ADD COLUMN IF NOT EXISTS extraction_concurrency INTEGER"
                ))
                existed = bool(conn.execute(text(
                    "SELECT 1 FROM information_schema.columns "
                    "WHERE table_schema = current_schema() AND table_name = 'provider' "
                    "AND column_name = 'concurrency_limit'"
                )).scalar())
                conn.execute(text(
                    "ALTER TABLE provider ADD COLUMN IF NOT EXISTS "
                    "concurrency_limit INTEGER NOT NULL DEFAULT 10"
                ))
                if not existed:
                    conn.execute(text(
                        "UPDATE provider SET concurrency_limit = GREATEST(1, LEAST(64, COALESCE("
                        "(SELECT extraction_concurrency FROM systemconfig WHERE id = 1), :fallback)))"
                    ), {"fallback": settings.extraction_concurrency})
                conn.execute(text(
                    "ALTER TABLE agentconversation ADD COLUMN IF NOT EXISTS deleted_at TIMESTAMPTZ"
                ))
                conn.execute(text(
                    "ALTER TABLE agentconversation ADD COLUMN IF NOT EXISTS deleted_by_id INTEGER"
                ))
                conn.execute(text(
                    "ALTER TABLE agentconversation ADD COLUMN IF NOT EXISTS deleted_by_name TEXT"
                ))
                conn.execute(text(
                    "ALTER TABLE entityresolution ADD COLUMN IF NOT EXISTS source_document_id INTEGER"
                ))
                conn.execute(text(
                    "ALTER TABLE entityresolution ADD COLUMN IF NOT EXISTS review_after TIMESTAMPTZ"
                ))
                conn.execute(text(
                    "ALTER TABLE entityresolution ADD COLUMN IF NOT EXISTS updated_at TIMESTAMPTZ"
                ))
                conn.execute(text(
                    "ALTER TABLE aboxprovenance ADD COLUMN IF NOT EXISTS source_document_id INTEGER"
                ))
                conn.execute(text(
                    "ALTER TABLE aboxprovenance ADD COLUMN IF NOT EXISTS source_document_sha256 TEXT"
                ))
                conn.execute(text(
                    "ALTER TABLE axiomprovenance ADD COLUMN IF NOT EXISTS source_document_id INTEGER"
                ))
                conn.execute(text(
                    "ALTER TABLE axiomprovenance ADD COLUMN IF NOT EXISTS source_document_sha256 TEXT"
                ))
                conn.execute(text(
                    "UPDATE entityresolution er SET source_document_id = c.document_id "
                    "FROM chunk c WHERE er.source_document_id IS NULL AND er.source_chunk_id = c.id"
                ))
                conn.execute(text(
                    "UPDATE entityresolution SET updated_at = COALESCE(resolved_at, created_at, NOW()) "
                    "WHERE updated_at IS NULL"
                ))
                conn.execute(text(
                    "UPDATE aboxprovenance ap SET source_document_id = c.document_id, "
                    "source_document_sha256 = d.sha256 FROM chunk c JOIN document d ON d.id = c.document_id "
                    "WHERE ap.chunk_id = c.id AND ap.source_document_id IS NULL"
                ))
                conn.execute(text(
                    "UPDATE axiomprovenance ap SET source_document_id = c.document_id, "
                    "source_document_sha256 = d.sha256 FROM chunk c JOIN document d ON d.id = c.document_id "
                    "WHERE ap.chunk_id = c.id AND ap.source_document_id IS NULL"
                ))
        return

    additions = {
        "user": [
            ("display_name", "TEXT"),
        ],
        "extractionjob": [
            ("total_chunks", "INTEGER NOT NULL DEFAULT 0"),
            ("processed_chunks", "INTEGER NOT NULL DEFAULT 0"),
            ("kind", "TEXT NOT NULL DEFAULT 'tbox'"),
            ("individuals_added", "INTEGER NOT NULL DEFAULT 0"),
            ("assertions_added", "INTEGER NOT NULL DEFAULT 0"),
            ("pending_added", "INTEGER NOT NULL DEFAULT 0"),
            ("unknown_classes", "JSON"),
            ("phase", "TEXT NOT NULL DEFAULT ''"),
            ("terms_added", "INTEGER NOT NULL DEFAULT 0"),
            ("terms_mapped", "INTEGER NOT NULL DEFAULT 0"),
            ("terminology_proposals", "INTEGER NOT NULL DEFAULT 0"),
            ("terminology_error", "TEXT"),
            ("prompt_snapshot", "JSON"),
        ],
        "axiomprovenance": [
            ("method", "TEXT NOT NULL DEFAULT 'extraction'"),
            ("actor_name", "TEXT NOT NULL DEFAULT ''"),
            ("audit_event_id", "INTEGER"),
            ("review_record", "JSON"),
            ("source_document_id", "INTEGER"),
            ("source_document_sha256", "TEXT"),
        ],
        "aboxprovenance": [
            ("method", "TEXT NOT NULL DEFAULT 'extraction'"),
            ("actor_name", "TEXT NOT NULL DEFAULT ''"),
            ("audit_event_id", "INTEGER"),
            ("review_record", "JSON"),
            ("source_document_id", "INTEGER"),
            ("source_document_sha256", "TEXT"),
        ],
        "termproposal": [
            ("extraction_job_id", "INTEGER"),
        ],
        "document": [
            ("folder", "TEXT NOT NULL DEFAULT '/'"),
            ("knowledge_system_id", "INTEGER"),
            ("tbox_extracted_at", "TIMESTAMP"),
            ("abox_extracted_at", "TIMESTAMP"),
        ],
        "knowledgesystem": [
            ("public_id", "TEXT"),
            ("owner_id", "INTEGER"),
            ("llm_model", "TEXT"),
            ("llm_provider_id", "INTEGER"),
            ("embedding_provider_id", "INTEGER"),
            ("embedding_model", "TEXT"),
        ],
        "auditevent": [
            ("added", "BLOB"),
            ("removed", "BLOB"),
            ("graph", "TEXT"),
            ("group_id", "TEXT"),
        ],
        "tboxreconciliation": [
            ("reason", "TEXT"),
        ],
        "systemconfig": [
            ("base_url", "TEXT"),
            ("api_key", "TEXT"),
            ("embedding_model", "TEXT"),
            ("llm_provider_id", "INTEGER"),
            ("embedding_provider_id", "INTEGER"),
            ("extraction_concurrency", "INTEGER"),
        ],
        "provider": [
            ("model", "TEXT NOT NULL DEFAULT ''"),
            ("kind", "TEXT NOT NULL DEFAULT 'llm'"),
            ("concurrency_limit", "INTEGER NOT NULL DEFAULT 10"),
            ("last_test_ok", "BOOLEAN"),
            ("last_tested_at", "TIMESTAMP"),
        ],
        "knowledgeapitoken": [
            ("secret_ciphertext", "TEXT"),
        ],
        "agentconversation": [
            ("deleted_at", "TIMESTAMP"),
            ("deleted_by_id", "INTEGER"),
            ("deleted_by_name", "TEXT"),
        ],
        "entityresolution": [
            ("source_document_id", "INTEGER"),
            ("review_after", "TIMESTAMP"),
            ("updated_at", "TIMESTAMP"),
        ],
    }
    provider_limit_added = False
    with engine.begin() as conn:
        for table, cols in additions.items():
            existing = {row[1] for row in conn.execute(text(f"PRAGMA table_info({table})"))}
            for name, ddl in cols:
                if name not in existing:
                    conn.execute(text(f"ALTER TABLE {table} ADD COLUMN {name} {ddl}"))
                    if table == "provider" and name == "concurrency_limit":
                        provider_limit_added = True

        if provider_limit_added:
            conn.execute(text(
                "UPDATE provider SET concurrency_limit = MAX(1, MIN(64, COALESCE("
                "(SELECT extraction_concurrency FROM systemconfig WHERE id = 1), :fallback)))"
            ), {"fallback": settings.extraction_concurrency})

        # Documents are now bound per-KS, so the same bytes can appear in multiple KS —
        # relax the old global-unique index on document.sha256 to a plain index, and add a
        # composite index for the per-KS dedup lookup.
        # unknown_classes was added nullable; the model field is a non-Optional dict, so
        # backfill pre-existing NULL rows to an empty JSON object.
        conn.execute(text("UPDATE extractionjob SET unknown_classes = '{}' WHERE unknown_classes IS NULL"))
        conn.execute(text("UPDATE extractionjob SET prompt_snapshot = '{}' WHERE prompt_snapshot IS NULL"))
        conn.execute(text("UPDATE axiomprovenance SET review_record = '{}' WHERE review_record IS NULL"))
        conn.execute(text("UPDATE aboxprovenance SET review_record = '{}' WHERE review_record IS NULL"))
        conn.execute(text(
            "UPDATE entityresolution SET source_document_id = ("
            "SELECT document_id FROM chunk WHERE chunk.id = entityresolution.source_chunk_id"
            ") WHERE source_document_id IS NULL AND source_chunk_id IS NOT NULL"
        ))
        conn.execute(text(
            "UPDATE entityresolution SET updated_at = COALESCE(resolved_at, created_at, CURRENT_TIMESTAMP) "
            "WHERE updated_at IS NULL"
        ))
        conn.execute(text(
            "UPDATE aboxprovenance SET source_document_id = ("
            "SELECT document_id FROM chunk WHERE chunk.id = aboxprovenance.chunk_id"
            ") WHERE source_document_id IS NULL AND chunk_id IS NOT NULL"
        ))
        conn.execute(text(
            "UPDATE aboxprovenance SET source_document_sha256 = ("
            "SELECT sha256 FROM document WHERE document.id = aboxprovenance.source_document_id"
            ") WHERE source_document_sha256 IS NULL AND source_document_id IS NOT NULL"
        ))
        conn.execute(text(
            "UPDATE axiomprovenance SET source_document_id = ("
            "SELECT document_id FROM chunk WHERE chunk.id = axiomprovenance.chunk_id"
            ") WHERE source_document_id IS NULL AND chunk_id IS NOT NULL"
        ))
        conn.execute(text(
            "UPDATE axiomprovenance SET source_document_sha256 = ("
            "SELECT sha256 FROM document WHERE document.id = axiomprovenance.source_document_id"
            ") WHERE source_document_sha256 IS NULL AND source_document_id IS NOT NULL"
        ))

        for row in conn.execute(text(
            "SELECT id FROM knowledgesystem WHERE public_id IS NULL OR public_id = ''"
        )):
            conn.execute(
                text("UPDATE knowledgesystem SET public_id = :public_id WHERE id = :id"),
                {"public_id": uuid4().hex, "id": row[0]},
            )
        conn.execute(text(
            "CREATE UNIQUE INDEX IF NOT EXISTS ix_knowledgesystem_public_id "
            "ON knowledgesystem (public_id)"
        ))

        doc_indexes = {row[1] for row in conn.execute(text("PRAGMA index_list(document)"))}
        if "ix_document_sha256" in doc_indexes:
            # Recreate non-unique only if the existing one is unique.
            is_unique = any(
                row[1] == "ix_document_sha256" and row[2] == 1
                for row in conn.execute(text("PRAGMA index_list(document)"))
            )
            if is_unique:
                conn.execute(text("DROP INDEX ix_document_sha256"))
                conn.execute(text("CREATE INDEX ix_document_sha256 ON document (sha256)"))
        conn.execute(
            text("CREATE INDEX IF NOT EXISTS ix_document_ks_sha256 ON document (knowledge_system_id, sha256)")
        )


def get_session() -> Iterator[Session]:
    """FastAPI dependency yielding a DB session.

    ``expire_on_commit=False`` so an ORM object returned by an endpoint still serializes
    correctly after a later ``session.commit()`` (e.g. ``audit.record`` commits after the
    object was built) — otherwise the expired instance serializes to ``{}``. Safe here because
    each request gets its own short-lived session and never relies on post-commit re-reads.
    """
    with Session(engine, expire_on_commit=False) as session:
        yield session
