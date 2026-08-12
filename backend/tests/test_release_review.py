from __future__ import annotations

import json
from pathlib import Path

from fastapi import BackgroundTasks
from sqlalchemy.pool import StaticPool
from sqlmodel import Session, SQLModel, create_engine

from app.api import releases
from app.db.models import KnowledgeSystem, OntologyRelease, User


def test_review_rechecks_quality_gate_instead_of_using_capture_result(monkeypatch) -> None:
    database = create_engine(
        "sqlite://",
        connect_args={"check_same_thread": False},
        poolclass=StaticPool,
    )
    SQLModel.metadata.create_all(database)
    clear_gate = {
        "open_conflict_errors": 0,
        "unresolved_entities": 0,
        "pending_terminology": 0,
        "validation_errors": 0,
        "blocking": 0,
    }
    monkeypatch.setattr(releases, "_quality_gate", lambda _session, _ks: clear_gate)

    with Session(database) as session:
        user = User(username="reviewer", password_hash="unused")
        ks = KnowledgeSystem(name="Example")
        session.add(user)
        session.add(ks)
        session.commit()
        session.refresh(user)
        session.refresh(ks)
        release = OntologyRelease(
            knowledge_system_id=ks.id,
            version="v1",
            status="draft",
            manifest={
                "capture_status": "ready",
                "quality_gate": {
                    **clear_gate,
                    "pending_terminology": 2,
                    "blocking": 2,
                },
            },
        )
        session.add(release)
        session.commit()
        session.refresh(release)

        result = releases.review_release(
            release.id,
            releases.ReviewReleaseRequest(),
            ks,
            user,
            session,
        )

        assert result["status"] == "reviewed"
        assert result["quality_gate"] == clear_gate
        stored = session.get(OntologyRelease, release.id)
        assert stored is not None
        assert stored.manifest["quality_gate"] == clear_gate


def test_publish_assigns_first_public_version_after_deleted_draft(tmp_path: Path) -> None:
    database = create_engine(
        "sqlite://",
        connect_args={"check_same_thread": False},
        poolclass=StaticPool,
    )
    SQLModel.metadata.create_all(database)

    with Session(database) as session:
        user = User(username="publisher", password_hash="unused")
        ks = KnowledgeSystem(name="Example", public_id="example")
        session.add(user)
        session.add(ks)
        session.commit()
        session.refresh(user)
        session.refresh(ks)
        deleted = OntologyRelease(
            knowledge_system_id=ks.id,
            version="v1",
            status="deleted",
            manifest={"capture_status": "deleted"},
        )
        reviewed = OntologyRelease(
            knowledge_system_id=ks.id,
            version="v2",
            status="reviewed",
            snapshot_dir=str(tmp_path),
            manifest={"capture_status": "ready"},
        )
        session.add(deleted)
        session.add(reviewed)
        session.commit()
        session.refresh(deleted)
        session.refresh(reviewed)

        result = releases.publish_release(
            reviewed.id,
            releases.PublishReleaseRequest(),
            BackgroundTasks(),
            ks,
            user,
            session,
        )

        assert result["status"] == "published"
        assert result["version"] == "v1"
        assert session.get(OntologyRelease, deleted.id).version == f"draft-{deleted.id}"
        assert json.loads((tmp_path / "manifest.json").read_text(encoding="utf-8"))["version"] == "v1"
