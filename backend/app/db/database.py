"""SQLite engine + session management."""
from __future__ import annotations

from collections.abc import Iterator

from sqlmodel import Session, SQLModel, create_engine

from app.config import settings

# check_same_thread=False so the engine can be shared across FastAPI's threadpool;
# timeout gives SQLite a busy-wait window so background-writes + polling-reads don't
# collide with "database is locked".
engine = create_engine(
    settings.db_url,
    echo=False,
    connect_args={"check_same_thread": False, "timeout": 30},
)


def init_db() -> None:
    """Create tables. Import models so they are registered on SQLModel.metadata."""
    from app.db import models  # noqa: F401  (side-effect: registers tables)

    SQLModel.metadata.create_all(engine)
    _migrate()


def _migrate() -> None:
    """Additive column migrations for SQLite (create_all does not ALTER existing tables)."""
    from sqlalchemy import text

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
        ],
        "document": [
            ("folder", "TEXT NOT NULL DEFAULT '/'"),
            ("knowledge_system_id", "INTEGER"),
            ("tbox_extracted_at", "TIMESTAMP"),
            ("abox_extracted_at", "TIMESTAMP"),
        ],
        "knowledgesystem": [
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
        ],
        "provider": [
            ("model", "TEXT NOT NULL DEFAULT ''"),
            ("kind", "TEXT NOT NULL DEFAULT 'llm'"),
            ("last_test_ok", "BOOLEAN"),
            ("last_tested_at", "TIMESTAMP"),
        ],
    }
    with engine.begin() as conn:
        for table, cols in additions.items():
            existing = {row[1] for row in conn.execute(text(f"PRAGMA table_info({table})"))}
            for name, ddl in cols:
                if name not in existing:
                    conn.execute(text(f"ALTER TABLE {table} ADD COLUMN {name} {ddl}"))

        # Documents are now bound per-KS, so the same bytes can appear in multiple KS —
        # relax the old global-unique index on document.sha256 to a plain index, and add a
        # composite index for the per-KS dedup lookup.
        # unknown_classes was added nullable; the model field is a non-Optional dict, so
        # backfill pre-existing NULL rows to an empty JSON object.
        conn.execute(text("UPDATE extractionjob SET unknown_classes = '{}' WHERE unknown_classes IS NULL"))

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
