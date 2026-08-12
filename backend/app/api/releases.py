"""Versioned ontology releases and asynchronous layer exports."""
from __future__ import annotations

import json
import logging
import re
import shutil
from contextlib import ExitStack
from pathlib import Path
from uuid import uuid4

from fastapi import APIRouter, BackgroundTasks, Depends, HTTPException, Query
from fastapi.responses import FileResponse
from pydantic import BaseModel, Field
from sqlalchemy import delete, func
from sqlmodel import Session, select

from app import audit
from app.api.conflicts import sync_conflicts
from app.api.knowledge import refresh_ks_stats
from app.config import settings
from app.db.database import engine, get_session
from app.db.models import (
    AboxProvenance,
    AxiomProvenance,
    Chunk,
    Conflict,
    EntityResolution,
    ExportJob,
    ExtractionJob,
    KnowledgeSystem,
    OntologyRelease,
    ReleaseDeployment,
    TermProposal,
    User,
    utcnow,
)
from app.ontology import abox_validate, release_service, release_store, store
from app.permissions import extraction_active, ks_owner, ks_reader, ks_writer
from app.security import current_user

logger = logging.getLogger(__name__)
router = APIRouter(prefix="/api/knowledge", tags=["releases"])
_VERSION = re.compile(r"^[0-9A-Za-z][0-9A-Za-z._-]{0,63}$")


class CreateReleaseRequest(BaseModel):
    version: str | None = None
    title: str = ""
    notes: str = ""
    shard_size: int = Field(default=100_000, ge=1_000, le=5_000_000)


class ReviewReleaseRequest(BaseModel):
    note: str = ""


class PublishReleaseRequest(BaseModel):
    note: str = ""


class ExportRequest(BaseModel):
    layer: str = "bundle"
    release_id: int | None = None
    shard_size: int = Field(default=100_000, ge=1_000, le=5_000_000)


def _release_out(
    row: OntologyRelease,
    deployment: ReleaseDeployment | None = None,
    public_id: str | None = None,
) -> dict:
    service_url = None
    if public_id and deployment is not None:
        service_url = f"/api/v1/knowledge-systems/{public_id}/releases/{row.version}"
    return {
        "id": row.id,
        "knowledge_system_id": row.knowledge_system_id,
        "version": row.version,
        "status": row.status,
        "title": row.title,
        "notes": row.notes,
        "manifest": row.manifest,
        "created_by": row.created_by_name,
        "reviewed_by": row.reviewed_by_name or None,
        "published_by": row.published_by_name or None,
        "created_at": row.created_at.isoformat(),
        "reviewed_at": row.reviewed_at.isoformat() if row.reviewed_at else None,
        "published_at": row.published_at.isoformat() if row.published_at else None,
        "deployment": release_service.deployment_out(deployment),
        "service_url": service_url,
    }


def _export_out(row: ExportJob) -> dict:
    return {
        "id": row.id,
        "knowledge_system_id": row.knowledge_system_id,
        "release_id": row.release_id,
        "layer": row.layer,
        "format": row.format,
        "status": row.status,
        "shard_size": row.shard_size,
        "processed_statements": row.processed_statements,
        "total_statements": row.total_statements,
        "files": row.files,
        "error": row.error,
        "created_by": row.created_by_name,
        "created_at": row.created_at.isoformat(),
        "started_at": row.started_at.isoformat() if row.started_at else None,
        "finished_at": row.finished_at.isoformat() if row.finished_at else None,
    }


def _next_version(session: Session, ks_id: int) -> str:
    versions = session.exec(
        select(OntologyRelease.version).where(
            OntologyRelease.knowledge_system_id == ks_id,
            OntologyRelease.published_at.is_not(None),
        )
    ).all()
    numbers = [int(item[1:]) for item in versions if re.fullmatch(r"v\d+", item)]
    return f"v{max(numbers, default=0) + 1}"


def _draft_version(release_id: int) -> str:
    return f"draft-{release_id}"


def normalize_unpublished_versions(session: Session, ks_id: int | None = None) -> int:
    """Move legacy pre-publication vN reservations onto internal draft identifiers."""
    query = select(OntologyRelease).where(OntologyRelease.published_at.is_(None))
    if ks_id is not None:
        query = query.where(OntologyRelease.knowledge_system_id == ks_id)
    changed = 0
    for release in session.exec(query).all():
        if release.id is None:
            continue
        internal = _draft_version(release.id)
        if release.version == internal:
            continue
        release.version = internal
        session.add(release)
        changed += 1
    if changed:
        session.commit()
    return changed


def _capture_release(release_id: int, shard_size: int) -> None:
    with Session(engine) as session:
        release = session.get(OntologyRelease, release_id)
        if not release:
            return
        if release.status == "deleted":
            return
        ks = session.get(KnowledgeSystem, release.knowledge_system_id)
        if not ks:
            return
        requested_version = release.manifest.get("requested_version")
        release.manifest = {
            "capture_status": "running",
            **({"requested_version": requested_version} if requested_version else {}),
        }
        session.add(release)
        session.commit()
        try:
            root, manifest = release_store.capture_release(session, ks, release.version, shard_size)
            session.refresh(release)
            if release.status == "deleted":
                shutil.rmtree(root, ignore_errors=True)
                release.snapshot_dir = ""
                release.manifest = {"capture_status": "deleted"}
                session.add(release)
                session.commit()
                return
            manifest["quality_gate"] = _quality_gate(session, ks)
            manifest["capture_status"] = "ready"
            if requested_version:
                manifest["requested_version"] = requested_version
            release.snapshot_dir = str(root)
            release.manifest = manifest
            session.add(release)
            session.commit()
        except Exception as exc:  # noqa: BLE001
            logger.exception("release %s capture failed", release_id)
            session.rollback()
            release = session.get(OntologyRelease, release_id)
            if release:
                release.manifest = {
                    "capture_status": "deleted" if release.status == "deleted" else "failed",
                    **({"error": str(exc)} if release.status != "deleted" else {}),
                    **({"requested_version": requested_version} if requested_version else {}),
                }
                session.add(release)
                session.commit()


def _set_export_progress(job_id: int, count: int) -> None:
    with Session(engine) as session:
        row = session.get(ExportJob, job_id)
        if row:
            row.processed_statements = count
            session.add(row)
            session.commit()


def _run_export(job_id: int) -> None:
    with Session(engine) as session:
        job = session.get(ExportJob, job_id)
        if not job:
            return
        ks = session.get(KnowledgeSystem, job.knowledge_system_id)
        if not ks:
            return
        job.status = "running"
        job.started_at = utcnow()
        session.add(job)
        session.commit()
        output_dir = settings.export_dir / ks.public_id / str(job.id)
        try:
            if output_dir.exists():
                shutil.rmtree(output_dir)
            output_dir.mkdir(parents=True)
            layers = ("tbox", "vocabulary", "abox") if job.layer == "bundle" else (job.layer,)
            files: list[dict] = []
            total = 0
            release = session.get(OntologyRelease, job.release_id) if job.release_id else None
            if job.release_id and (not release or release.knowledge_system_id != ks.id):
                raise ValueError("Release not found")
            if release and release.status == "deleted":
                raise ValueError("Release has been deleted")
            if release and release.manifest.get("capture_status") != "ready":
                raise ValueError("Release snapshot is not ready")

            graph_map = release_store.graph_layers(ks)
            with ExitStack() as locks:
                if not release:
                    for layer in layers:
                        locks.enter_context(store.read_lock(graph_map[layer]))
                for layer in layers:
                    if release:
                        layer_files = release_store.copy_release_layer(
                            Path(release.snapshot_dir), release.manifest, layer, output_dir,
                        )
                    else:
                        graph_iri = graph_map[layer]
                        offset = total
                        layer_files = release_store.write_graph_shards(
                            graph_iri,
                            output_dir,
                            layer,
                            shard_size=job.shard_size if layer == "abox" else 1_000_000,
                            progress=lambda count, base=offset: _set_export_progress(job_id, base + count),
                        )
                    for item in layer_files:
                        item["layer"] = layer
                    files.extend(layer_files)
                    total += sum(item["statements"] for item in layer_files)

            if job.layer == "bundle":
                if release:
                    for item in release.manifest.get("provenance", []):
                        source = Path(release.snapshot_dir) / item["name"]
                        shutil.copyfile(source, output_dir / item["name"])
                        files.append({**item, "layer": "provenance"})
                else:
                    files.extend({**item, "layer": "provenance"} for item in release_store.write_provenance(session, ks.id, output_dir))

            manifest = {
                "knowledge_system": {"id": ks.public_id, "name": ks.name},
                "release_id": release.id if release else None,
                "release_version": release.version if release else None,
                "layer": job.layer,
                "format": "application/n-quads",
                "compression": "none",
                "files": files,
            }
            manifest_file = release_store.write_manifest(output_dir, manifest)
            files.append({**manifest_file, "layer": "manifest"})
            job.output_dir = str(output_dir)
            job.files = files
            job.total_statements = total
            job.processed_statements = total
            job.status = "completed"
            job.finished_at = utcnow()
            session.add(job)
            session.commit()
        except Exception as exc:  # noqa: BLE001
            logger.exception("export job %s failed", job_id)
            session.rollback()
            job = session.get(ExportJob, job_id)
            if job:
                job.status = "failed"
                job.error = str(exc)
                job.finished_at = utcnow()
                session.add(job)
                session.commit()


def _quality_gate(session: Session, ks: KnowledgeSystem) -> dict:
    open_errors = session.exec(
        select(func.count(Conflict.id)).where(
            Conflict.knowledge_system_id == ks.id,
            Conflict.status == "open",
            Conflict.severity == "error",
        )
    ).one()
    unresolved_entities = session.exec(
        select(func.count(EntityResolution.id)).where(
            EntityResolution.knowledge_system_id == ks.id,
            EntityResolution.status == "pending",
        )
    ).one()
    pending_terms = session.exec(
        select(func.count(TermProposal.id)).where(
            TermProposal.knowledge_system_id == ks.id,
            TermProposal.status == "pending",
        )
    ).one()
    validation = abox_validate.validate(ks.graph_iri, release_store.graph_layers(ks)["abox"])
    validation_errors = validation["counts"]["error"]
    return {
        "open_conflict_errors": open_errors,
        "unresolved_entities": unresolved_entities,
        "pending_terminology": pending_terms,
        "validation_errors": validation_errors,
        "blocking": open_errors + unresolved_entities + pending_terms + validation_errors,
    }


@router.get("/{ks_id}/releases")
def list_releases(
    ks: KnowledgeSystem = Depends(ks_reader), session: Session = Depends(get_session),
) -> dict:
    rows = session.exec(
        select(OntologyRelease)
        .where(OntologyRelease.knowledge_system_id == ks.id)
        .order_by(OntologyRelease.created_at.desc())
    ).all()
    deployments = {
        item.release_id: item
        for item in session.exec(
            select(ReleaseDeployment).where(ReleaseDeployment.knowledge_system_id == ks.id)
        ).all()
    }
    return {
        "items": [_release_out(row, deployments.get(row.id), ks.public_id) for row in rows],
        "total": len(rows),
    }


@router.post("/{ks_id}/releases")
def create_release(
    body: CreateReleaseRequest,
    background: BackgroundTasks,
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict:
    if extraction_active(session, ks.id):
        raise HTTPException(status_code=409, detail="An extraction is running")
    requested_version = (body.version or "").strip()
    if requested_version and not _VERSION.fullmatch(requested_version):
        raise HTTPException(status_code=400, detail="Version may contain letters, numbers, dots, dashes and underscores")
    row = OntologyRelease(
        knowledge_system_id=ks.id,
        version=f"draft-{uuid4().hex}",
        title=body.title.strip(),
        notes=body.notes.strip(),
        manifest={
            "capture_status": "pending",
            **({"requested_version": requested_version} if requested_version else {}),
        },
        created_by_id=user.id,
        created_by_name=user.username,
    )
    session.add(row)
    session.commit()
    session.refresh(row)
    row.version = _draft_version(row.id)
    session.add(row)
    session.commit()
    session.refresh(row)
    audit.record(
        session,
        ks_id=ks.id,
        action="release.draft",
        summary=f"Created immutable release draft #{row.id}",
        actor_id=user.id,
        actor_name=user.username,
        detail={"release_id": row.id, "requested_version": requested_version or None},
    )
    background.add_task(_capture_release, row.id, body.shard_size)
    return _release_out(row, public_id=ks.public_id)


@router.post("/{ks_id}/releases/{release_id}/review")
def review_release(
    release_id: int,
    body: ReviewReleaseRequest,
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict:
    row = session.get(OntologyRelease, release_id)
    if not row or row.knowledge_system_id != ks.id:
        raise HTTPException(status_code=404, detail="Release not found")
    if row.status != "draft":
        raise HTTPException(status_code=409, detail="Only draft releases can be reviewed")
    if row.manifest.get("capture_status") != "ready":
        raise HTTPException(status_code=409, detail="Release snapshot is not ready")
    # Governance queues can change after the immutable RDF snapshot is captured. Always
    # evaluate their current state when the reviewer retries instead of permanently
    # blocking the draft with the gate result recorded at capture time.
    gate = _quality_gate(session, ks)
    row.manifest = {**row.manifest, "quality_gate": gate}
    if gate["blocking"]:
        session.add(row)
        session.commit()
        raise HTTPException(status_code=409, detail={"message": "Release quality gate failed", "quality_gate": gate})
    row.status = "reviewed"
    row.reviewed_by_id = user.id
    row.reviewed_by_name = user.username
    row.reviewed_at = utcnow()
    session.add(row)
    session.commit()
    audit.record(
        session, ks_id=ks.id, action="release.review", summary=f"Approved release {row.version}",
        actor_id=user.id, actor_name=user.username,
        detail={"release_id": row.id, "version": row.version, "note": body.note, "quality_gate": gate},
    )
    return {**_release_out(row, public_id=ks.public_id), "quality_gate": gate}


@router.post("/{ks_id}/releases/{release_id}/publish")
def publish_release(
    release_id: int,
    body: PublishReleaseRequest,
    background: BackgroundTasks,
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict:
    row = session.get(OntologyRelease, release_id)
    if not row or row.knowledge_system_id != ks.id:
        raise HTTPException(status_code=404, detail="Release not found")
    if row.status != "reviewed":
        raise HTTPException(status_code=409, detail="Only reviewed releases can be published")
    normalize_unpublished_versions(session, ks.id)
    session.refresh(row)
    requested_version = str(row.manifest.get("requested_version") or "").strip()
    version = requested_version or _next_version(session, ks.id)
    if not _VERSION.fullmatch(version):
        raise HTTPException(status_code=400, detail="Version may contain letters, numbers, dots, dashes and underscores")
    existing = session.exec(select(OntologyRelease).where(
        OntologyRelease.knowledge_system_id == ks.id,
        OntologyRelease.version == version,
        OntologyRelease.id != row.id,
    )).first()
    if existing:
        raise HTTPException(status_code=409, detail="That release version already exists")
    row.version = version
    row.manifest = release_store.finalize_release_version(
        Path(row.snapshot_dir), row.manifest, version,
    )
    row.status = "published"
    row.published_by_id = user.id
    row.published_by_name = user.username
    row.published_at = utcnow()
    session.add(row)
    session.commit()
    deployment = release_service.ensure_deployment(session, ks, row)
    audit.record(
        session, ks_id=ks.id, action="release.publish", summary=f"Published release {row.version}",
        actor_id=user.id, actor_name=user.username,
        detail={
            "release_id": row.id,
            "version": row.version,
            "note": body.note,
            "deployment_id": deployment.id,
        },
    )
    background.add_task(release_service.provision, deployment.id)
    return _release_out(row, deployment, ks.public_id)


@router.post("/{ks_id}/releases/{release_id}/deployment")
def deploy_release_service(
    release_id: int,
    background: BackgroundTasks,
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict:
    row = session.get(OntologyRelease, release_id)
    if not row or row.knowledge_system_id != ks.id:
        raise HTTPException(status_code=404, detail="Release not found")
    if row.status == "deleted":
        raise HTTPException(status_code=410, detail="Release has been deleted")
    if row.status != "published":
        raise HTTPException(status_code=409, detail="Only published releases can be served")
    if row.manifest.get("capture_status") != "ready":
        raise HTTPException(status_code=409, detail="Release snapshot is not ready")
    current = release_service.deployment_for(session, row.id)
    if current and current.status in {"active", "provisioning"}:
        return _release_out(row, current, ks.public_id)
    deployment = release_service.ensure_deployment(session, ks, row)
    audit.record(
        session,
        ks_id=ks.id,
        action="release.deploy",
        summary=f"Started service deployment for release {row.version}",
        actor_id=user.id,
        actor_name=user.username,
        detail={"release_id": row.id, "deployment_id": deployment.id},
    )
    background.add_task(release_service.provision, deployment.id)
    return _release_out(row, deployment, ks.public_id)


@router.delete("/{ks_id}/releases/{release_id}/deployment")
def stop_release_service(
    release_id: int,
    background: BackgroundTasks,
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict:
    row = session.get(OntologyRelease, release_id)
    if not row or row.knowledge_system_id != ks.id:
        raise HTTPException(status_code=404, detail="Release not found")
    deployment = release_service.deployment_for(session, row.id)
    if deployment is None or deployment.status == "stopped":
        return _release_out(row, deployment, ks.public_id)
    deployment.status = "stopping"
    session.add(deployment)
    session.commit()
    audit.record(
        session,
        ks_id=ks.id,
        action="release.undeploy",
        summary=f"Stopped service for release {row.version}",
        actor_id=user.id,
        actor_name=user.username,
        detail={"release_id": row.id, "deployment_id": deployment.id},
    )
    background.add_task(release_service.stop, deployment.id)
    return _release_out(row, deployment, ks.public_id)


@router.delete("/{ks_id}/releases/{release_id}")
def delete_release(
    release_id: int,
    background: BackgroundTasks,
    ks: KnowledgeSystem = Depends(ks_owner),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict:
    row = session.get(OntologyRelease, release_id)
    if not row or row.knowledge_system_id != ks.id:
        raise HTTPException(status_code=404, detail="Release not found")
    if row.status == "deleted":
        return _release_out(
            row,
            release_service.deployment_for(session, row.id),
            ks.public_id,
        )
    if row.manifest.get("capture_status") in {"pending", "running"}:
        raise HTTPException(status_code=409, detail="Release snapshot capture is still running")
    active_export = session.exec(
        select(ExportJob).where(
            ExportJob.release_id == row.id,
            ExportJob.status.in_(("pending", "running")),
        )
    ).first()
    if active_export is not None:
        raise HTTPException(status_code=409, detail="A release export is still running")
    previous_status = row.status
    row.status = "deleted"
    deployment = release_service.deployment_for(session, row.id)
    if deployment is not None and deployment.status != "stopped":
        deployment.status = "stopping"
        session.add(deployment)
    session.add(row)
    session.commit()
    audit.record(
        session,
        ks_id=ks.id,
        action="release.delete",
        summary=f"Deleted release {row.version}",
        actor_id=user.id,
        actor_name=user.username,
        detail={
            "release_id": row.id,
            "version": row.version,
            "previous_status": previous_status,
            "manifest_sha256": row.manifest.get("manifest_file", {}).get("sha256"),
        },
    )
    background.add_task(release_service.delete_release_data, row.id)
    return _release_out(row, deployment, ks.public_id)


@router.get("/{ks_id}/releases/diff")
def diff_releases(
    from_id: int = Query(),
    to_id: int = Query(),
    ks: KnowledgeSystem = Depends(ks_reader),
    session: Session = Depends(get_session),
) -> dict:
    left = session.get(OntologyRelease, from_id)
    right = session.get(OntologyRelease, to_id)
    if not left or not right or left.knowledge_system_id != ks.id or right.knowledge_system_id != ks.id:
        raise HTTPException(status_code=404, detail="Release not found")
    if left.status == "deleted" or right.status == "deleted":
        raise HTTPException(status_code=410, detail="A selected release has been deleted")
    if left.manifest.get("capture_status") != "ready" or right.manifest.get("capture_status") != "ready":
        raise HTTPException(status_code=409, detail="Both release snapshots must be ready")
    return {
        "from": {"id": left.id, "version": left.version},
        "to": {"id": right.id, "version": right.version},
        **release_store.semantic_diff(
            Path(left.snapshot_dir), left.manifest, Path(right.snapshot_dir), right.manifest,
        ),
    }


def _restore_provenance(session: Session, ks_id: int, root: Path, manifest: dict) -> None:
    session.exec(delete(AxiomProvenance).where(AxiomProvenance.knowledge_system_id == ks_id))
    session.exec(delete(AboxProvenance).where(AboxProvenance.knowledge_system_id == ks_id))
    for item in manifest.get("provenance", []):
        path = root / item["name"]
        model = AxiomProvenance if item["name"].startswith("tbox-") else AboxProvenance
        key_name = "axiom_key" if model is AxiomProvenance else "fact_key"
        with path.open("r", encoding="utf-8") as handle:
            for line in handle:
                payload = json.loads(line)
                chunk_id = payload.get("chunk", {}).get("id") if payload.get("chunk") else None
                extraction = payload.get("extraction") or {}
                job_id = extraction.get("job_id")
                if chunk_id and not session.get(Chunk, chunk_id):
                    chunk_id = None
                if job_id and not session.get(ExtractionJob, job_id):
                    job_id = None
                session.add(model(
                    knowledge_system_id=ks_id,
                    **{key_name: payload[key_name]},
                    chunk_id=chunk_id,
                    job_id=job_id,
                    method=payload.get("method", "extraction"),
                    actor_name=payload.get("actor", ""),
                    review_record=(payload.get("reviews") or [{}])[0],
                ))
    session.commit()


@router.post("/{ks_id}/releases/{release_id}/rollback")
def rollback_release(
    release_id: int,
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict:
    if extraction_active(session, ks.id):
        raise HTTPException(status_code=409, detail="An extraction is running")
    row = session.get(OntologyRelease, release_id)
    if not row or row.knowledge_system_id != ks.id:
        raise HTTPException(status_code=404, detail="Release not found")
    if row.status == "deleted":
        raise HTTPException(status_code=410, detail="Release has been deleted")
    if row.manifest.get("capture_status") != "ready":
        raise HTTPException(status_code=409, detail="Release snapshot is not ready")
    root = Path(row.snapshot_dir)
    graph_map = release_store.graph_layers(ks)
    with ExitStack() as locks:
        for graph_iri in graph_map.values():
            locks.enter_context(store.read_lock(graph_iri))
        for layer, graph_iri in graph_map.items():
            store.replace_graph(
                graph_iri,
                release_store.iter_artifact_quads(root, row.manifest["layers"][layer]["files"]),
            )
    _restore_provenance(session, ks.id, root, row.manifest)
    session.exec(delete(Conflict).where(Conflict.knowledge_system_id == ks.id))
    session.exec(delete(EntityResolution).where(
        EntityResolution.knowledge_system_id == ks.id,
        EntityResolution.status == "pending",
    ))
    session.exec(delete(TermProposal).where(
        TermProposal.knowledge_system_id == ks.id,
        TermProposal.status == "pending",
    ))
    session.commit()
    refresh_ks_stats(session, ks)
    sync_conflicts(session, ks, semantic=False)
    audit.record(
        session, ks_id=ks.id, action="release.rollback", summary=f"Restored release {row.version}",
        actor_id=user.id, actor_name=user.username,
        detail={"release_id": row.id, "version": row.version},
    )
    return {"restored": row.id, "version": row.version}


@router.get("/{ks_id}/exports")
def list_exports(
    ks: KnowledgeSystem = Depends(ks_reader), session: Session = Depends(get_session),
) -> dict:
    rows = session.exec(
        select(ExportJob).where(ExportJob.knowledge_system_id == ks.id).order_by(ExportJob.created_at.desc())
    ).all()
    return {"items": [_export_out(row) for row in rows], "total": len(rows)}


@router.post("/{ks_id}/exports")
def create_export(
    body: ExportRequest,
    background: BackgroundTasks,
    ks: KnowledgeSystem = Depends(ks_reader),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict:
    if body.layer not in {"tbox", "vocabulary", "abox", "bundle"}:
        raise HTTPException(status_code=400, detail="Unsupported export layer")
    if body.release_id:
        release = session.get(OntologyRelease, body.release_id)
        if not release or release.knowledge_system_id != ks.id:
            raise HTTPException(status_code=404, detail="Release not found")
        if release.status == "deleted":
            raise HTTPException(status_code=410, detail="Release has been deleted")
    row = ExportJob(
        knowledge_system_id=ks.id,
        release_id=body.release_id,
        layer=body.layer,
        shard_size=body.shard_size,
        created_by_id=user.id,
        created_by_name=user.username,
    )
    session.add(row)
    session.commit()
    session.refresh(row)
    background.add_task(_run_export, row.id)
    return _export_out(row)


@router.get("/{ks_id}/exports/{job_id}")
def get_export(
    job_id: int,
    ks: KnowledgeSystem = Depends(ks_reader),
    session: Session = Depends(get_session),
) -> dict:
    row = session.get(ExportJob, job_id)
    if not row or row.knowledge_system_id != ks.id:
        raise HTTPException(status_code=404, detail="Export job not found")
    return _export_out(row)


@router.get("/{ks_id}/exports/{job_id}/files/{filename}")
def download_export_file(
    job_id: int,
    filename: str,
    ks: KnowledgeSystem = Depends(ks_reader),
    session: Session = Depends(get_session),
) -> FileResponse:
    row = session.get(ExportJob, job_id)
    if not row or row.knowledge_system_id != ks.id or row.status != "completed":
        raise HTTPException(status_code=404, detail="Export file not found")
    allowed = {item["name"] for item in row.files}
    if filename not in allowed or Path(filename).name != filename:
        raise HTTPException(status_code=404, detail="Export file not found")
    path = Path(row.output_dir) / filename
    if not path.is_file():
        raise HTTPException(status_code=404, detail="Export file not found")
    media_type = "application/n-quads" if filename.endswith(".nq") else (
        "application/x-ndjson" if filename.endswith(".jsonl") else "application/json"
    )
    return FileResponse(path, filename=filename, media_type=media_type)
