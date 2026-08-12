"""Read-only RDF projections for published immutable releases."""
from __future__ import annotations

import hashlib
import json
import logging
import shutil
import threading
from pathlib import Path

from pyoxigraph import NamedNode, Store
from sqlalchemy import delete, insert
from sqlmodel import Session, select

from app.config import settings
from app.db.database import engine
from app.db.models import (
    ExportJob,
    KnowledgeSystem,
    OntologyRelease,
    ReleaseDeployment,
    ReleaseStatementProvenance,
    utcnow,
)
from app.ontology import release_store, store

logger = logging.getLogger(__name__)

_serving_store: Store | None = None
_store_lock = threading.Lock()
_deployment_lock = threading.Lock()


def get_store() -> Store:
    global _serving_store
    if _serving_store is None:
        with _store_lock:
            if _serving_store is None:
                _serving_store = Store(str(settings.serving_oxigraph_dir))
    return _serving_store


def graph_iris(ks: KnowledgeSystem, release: OntologyRelease) -> dict[str, str]:
    root = f"urn:ontopilot:release-service:{ks.public_id}:{release.id}"
    return {
        "tbox": f"{root}:tbox",
        "vocabulary": f"{root}:vocabulary",
        "abox": f"{root}:abox",
    }


def deployment_for(session: Session, release_id: int) -> ReleaseDeployment | None:
    return session.exec(
        select(ReleaseDeployment).where(ReleaseDeployment.release_id == release_id)
    ).first()


def ensure_deployment(
    session: Session,
    ks: KnowledgeSystem,
    release: OntologyRelease,
) -> ReleaseDeployment:
    deployment = deployment_for(session, release.id)
    graphs = graph_iris(ks, release)
    if deployment is None:
        deployment = ReleaseDeployment(
            knowledge_system_id=ks.id,
            release_id=release.id,
            tbox_graph_iri=graphs["tbox"],
            vocabulary_graph_iri=graphs["vocabulary"],
            abox_graph_iri=graphs["abox"],
        )
    else:
        deployment.status = "provisioning"
        deployment.error = None
        deployment.stopped_at = None
        deployment.tbox_graph_iri = graphs["tbox"]
        deployment.vocabulary_graph_iri = graphs["vocabulary"]
        deployment.abox_graph_iri = graphs["abox"]
    session.add(deployment)
    session.commit()
    session.refresh(deployment)
    return deployment


def deployment_out(deployment: ReleaseDeployment | None) -> dict | None:
    if deployment is None:
        return None
    return {
        "id": deployment.id,
        "status": deployment.status,
        "statement_count": deployment.statement_count,
        "provenance_count": deployment.provenance_count,
        "error": deployment.error,
        "activated_at": deployment.activated_at.isoformat() if deployment.activated_at else None,
        "stopped_at": deployment.stopped_at.isoformat() if deployment.stopped_at else None,
    }


def _verify_artifacts(root: Path, manifest: dict) -> None:
    entries = [
        item
        for layer in manifest.get("layers", {}).values()
        for item in layer.get("files", [])
    ] + list(manifest.get("provenance", []))
    for item in entries:
        path = root / item["name"]
        if not path.is_file():
            raise FileNotFoundError(f"Release artifact is missing: {item['name']}")
        digest = hashlib.sha256()
        with path.open("rb") as handle:
            for chunk in iter(lambda: handle.read(1024 * 1024), b""):
                digest.update(chunk)
        if digest.hexdigest() != item.get("sha256"):
            raise ValueError(f"Release artifact checksum mismatch: {item['name']}")


def _index_provenance(
    session: Session,
    release: OntologyRelease,
    root: Path,
) -> int:
    session.exec(
        delete(ReleaseStatementProvenance).where(
            ReleaseStatementProvenance.release_id == release.id
        )
    )
    total = 0
    batch: list[dict] = []
    for item in release.manifest.get("provenance", []):
        layer = "tbox" if item["name"].startswith("tbox-") else "abox"
        key_name = "axiom_key" if layer == "tbox" else "fact_key"
        with (root / item["name"]).open("r", encoding="utf-8") as handle:
            for line in handle:
                payload = json.loads(line)
                statement_key = payload.get(key_name)
                if not statement_key:
                    continue
                batch.append({
                    "knowledge_system_id": release.knowledge_system_id,
                    "release_id": release.id,
                    "layer": layer,
                    "statement_key": statement_key,
                    "payload": payload,
                })
                total += 1
                if len(batch) >= 1_000:
                    session.execute(insert(ReleaseStatementProvenance), batch)
                    batch.clear()
    if batch:
        session.execute(insert(ReleaseStatementProvenance), batch)
    return total


def _clear_graphs(deployment: ReleaseDeployment) -> None:
    with store.use_store(get_store()):
        rdf_store = store.get_store()
        for graph_iri in (
            deployment.tbox_graph_iri,
            deployment.vocabulary_graph_iri,
            deployment.abox_graph_iri,
        ):
            if graph_iri:
                rdf_store.clear_graph(NamedNode(graph_iri))


def provision(deployment_id: int) -> None:
    with _deployment_lock, Session(engine) as session:
        deployment = session.get(ReleaseDeployment, deployment_id)
        if deployment is None:
            return
        release = session.get(OntologyRelease, deployment.release_id)
        ks = session.get(KnowledgeSystem, deployment.knowledge_system_id)
        if release is None or ks is None or release.status != "published":
            deployment.status = "stopped"
            deployment.error = "Release is no longer published"
            deployment.stopped_at = utcnow()
            session.add(deployment)
            session.commit()
            _clear_graphs(deployment)
            return
        if release.manifest.get("capture_status") != "ready":
            deployment.status = "failed"
            deployment.error = "Release snapshot is not ready"
            session.add(deployment)
            session.commit()
            return

        root = Path(release.snapshot_dir)
        try:
            _verify_artifacts(root, release.manifest)
            graph_map = {
                "tbox": deployment.tbox_graph_iri,
                "vocabulary": deployment.vocabulary_graph_iri,
                "abox": deployment.abox_graph_iri,
            }
            statement_count = 0
            with store.use_store(get_store()):
                for layer, graph_iri in graph_map.items():
                    files = release.manifest["layers"][layer]["files"]
                    store.replace_graph(
                        graph_iri,
                        release_store.iter_artifact_quads(root, files),
                    )
                    actual = store.count_graph(graph_iri)
                    expected = release.manifest["layers"][layer]["statements"]
                    if actual != expected:
                        raise ValueError(
                            f"Release service graph count mismatch for {layer}: {actual} != {expected}"
                        )
                    statement_count += actual

            provenance_count = _index_provenance(session, release, root)
            session.commit()
            session.refresh(release)
            session.refresh(deployment)
            if release.status != "published" or deployment.status == "stopping":
                _clear_graphs(deployment)
                session.exec(delete(ReleaseStatementProvenance).where(
                    ReleaseStatementProvenance.release_id == release.id
                ))
                deployment.status = "stopped"
                deployment.stopped_at = utcnow()
            else:
                deployment.status = "active"
                deployment.statement_count = statement_count
                deployment.provenance_count = provenance_count
                deployment.error = None
                deployment.activated_at = utcnow()
                deployment.stopped_at = None
            session.add(deployment)
            session.commit()
        except Exception as exc:  # noqa: BLE001
            logger.exception("release deployment %s failed", deployment_id)
            session.rollback()
            deployment = session.get(ReleaseDeployment, deployment_id)
            if deployment is None:
                return
            _clear_graphs(deployment)
            session.exec(delete(ReleaseStatementProvenance).where(
                ReleaseStatementProvenance.release_id == deployment.release_id
            ))
            deployment.status = "failed"
            deployment.error = str(exc)
            session.add(deployment)
            session.commit()


def stop(deployment_id: int) -> None:
    with _deployment_lock, Session(engine) as session:
        deployment = session.get(ReleaseDeployment, deployment_id)
        if deployment is None:
            return
        _clear_graphs(deployment)
        session.exec(delete(ReleaseStatementProvenance).where(
            ReleaseStatementProvenance.release_id == deployment.release_id
        ))
        deployment.status = "stopped"
        deployment.statement_count = 0
        deployment.provenance_count = 0
        deployment.error = None
        deployment.stopped_at = utcnow()
        session.add(deployment)
        session.commit()


def cleanup_inactive() -> None:
    """Remove projections left inaccessible by an interrupted stop/provision operation."""
    with Session(engine) as session:
        deployments = session.exec(
            select(ReleaseDeployment).where(
                ReleaseDeployment.status.in_(("stopped", "failed"))
            )
        ).all()
        for deployment in deployments:
            _clear_graphs(deployment)
            session.exec(delete(ReleaseStatementProvenance).where(
                ReleaseStatementProvenance.release_id == deployment.release_id
            ))
        if deployments:
            session.commit()


def delete_release_data(release_id: int) -> None:
    with Session(engine) as session:
        release = session.get(OntologyRelease, release_id)
        if release is None or release.status != "deleted":
            return
        deployment = deployment_for(session, release.id)
        if deployment is not None:
            stop(deployment.id)
        for job in session.exec(select(ExportJob).where(ExportJob.release_id == release.id)).all():
            if job.output_dir:
                shutil.rmtree(job.output_dir, ignore_errors=True)
            session.delete(job)
        if release.snapshot_dir:
            shutil.rmtree(release.snapshot_dir, ignore_errors=True)
            release.snapshot_dir = ""
            session.add(release)
        session.commit()


def abox_sources(
    session: Session,
    release_id: int,
    fact_keys: list[str],
    snippet_len: int = 240,
) -> dict[str, list[dict]]:
    keys = [key for key in set(fact_keys) if key]
    if not keys:
        return {}
    rows = session.exec(
        select(ReleaseStatementProvenance).where(
            ReleaseStatementProvenance.release_id == release_id,
            ReleaseStatementProvenance.layer == "abox",
            ReleaseStatementProvenance.statement_key.in_(keys),
        )
    ).all()
    result: dict[str, list[dict]] = {}
    for row in rows:
        payload = row.payload
        chunk = payload.get("chunk") or {}
        document = payload.get("document") or {}
        extraction = payload.get("extraction") or {}
        result.setdefault(row.statement_key, []).append({
            "chunk_id": chunk.get("id"),
            "document_id": document.get("id"),
            "document": document.get("filename"),
            "snippet": str(chunk.get("text") or "")[:snippet_len].strip(),
            "job_id": extraction.get("job_id"),
            "model": extraction.get("model"),
            "prompt_snapshot": extraction.get("prompt_snapshot"),
            "method": payload.get("method"),
            "actor": payload.get("actor") or None,
            "review": (payload.get("reviews") or [None])[0],
        })
    return result
