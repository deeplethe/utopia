"""Immutable release artifacts and streaming, uncompressed N-Quads exports."""
from __future__ import annotations

import hashlib
import json
import re
import shutil
from contextlib import ExitStack
from collections.abc import Callable, Iterable, Iterator
from pathlib import Path

from pyoxigraph import Quad, RdfFormat, parse
from sqlmodel import Session, select

from app.config import settings
from app.db.models import (
    AboxProvenance,
    AuditEvent,
    AxiomProvenance,
    Chunk,
    Document,
    ExtractionJob,
    KnowledgeSystem,
)
from app.ontology import skos, store

_SAFE = re.compile(r"[^A-Za-z0-9._-]+")


def safe_name(value: str) -> str:
    return _SAFE.sub("-", value.strip()).strip(".-") or "artifact"


def graph_layers(ks: KnowledgeSystem) -> dict[str, str]:
    return {
        "tbox": ks.graph_iri,
        "vocabulary": skos.graph_iri_for(ks),
        "abox": f"{ks.graph_iri.rstrip('/')}/abox",
    }


def _nquad_line(quad: Quad) -> bytes:
    return f"{quad.subject} {quad.predicate} {quad.object} {quad.graph_name} .\n".encode("utf-8")


def write_graph_shards(
    graph_iri: str,
    output_dir: Path,
    prefix: str,
    *,
    shard_size: int = 100_000,
    progress: Callable[[int], None] | None = None,
) -> list[dict]:
    """Write deterministic-size, uncompressed N-Quads shards using constant graph memory."""
    output_dir.mkdir(parents=True, exist_ok=True)
    shard_size = max(1, shard_size)
    files: list[dict] = []
    handle = None
    digest = None
    path = None
    in_shard = 0
    total = 0

    def close_shard() -> None:
        nonlocal handle, digest, path, in_shard
        if handle is None or digest is None or path is None:
            return
        handle.close()
        files.append({
            "name": path.name,
            "statements": in_shard,
            "bytes": path.stat().st_size,
            "sha256": digest.hexdigest(),
        })
        handle = digest = path = None
        in_shard = 0

    try:
        for quad in store.iter_quads(graph_iri):
            if handle is None:
                path = output_dir / f"{safe_name(prefix)}-{len(files) + 1:05d}.nq"
                handle = path.open("wb")
                digest = hashlib.sha256()
            line = _nquad_line(quad)
            handle.write(line)
            digest.update(line)
            in_shard += 1
            total += 1
            if total % 10_000 == 0 and progress:
                progress(total)
            if in_shard >= shard_size:
                close_shard()
        close_shard()
    except Exception:
        if handle is not None:
            handle.close()
        raise
    if not files:
        path = output_dir / f"{safe_name(prefix)}-00001.nq"
        path.write_bytes(b"")
        files.append({
            "name": path.name,
            "statements": 0,
            "bytes": 0,
            "sha256": hashlib.sha256(b"").hexdigest(),
        })
    if progress:
        progress(total)
    return files


def _iter_file_quads(path: Path, batch_bytes: int = 2 * 1024 * 1024) -> Iterator[Quad]:
    """Parse N-Quads incrementally so restore does not load a shard into memory."""
    buffer = bytearray()
    with path.open("rb") as handle:
        for line in handle:
            buffer.extend(line)
            if len(buffer) >= batch_bytes:
                yield from parse(bytes(buffer), format=RdfFormat.N_QUADS)
                buffer.clear()
    if buffer:
        yield from parse(bytes(buffer), format=RdfFormat.N_QUADS)


def iter_artifact_quads(root: Path, files: Iterable[dict]) -> Iterator[Quad]:
    for item in files:
        yield from _iter_file_quads(root / item["name"])


def _source_context(session: Session, rows: list) -> tuple[dict, dict, dict]:
    chunk_ids = {row.chunk_id for row in rows if row.chunk_id is not None}
    job_ids = {row.job_id for row in rows if row.job_id is not None}
    chunks = {
        row.id: row for row in session.exec(select(Chunk).where(Chunk.id.in_(chunk_ids))).all()
    } if chunk_ids else {}
    doc_ids = {row.document_id for row in chunks.values()}
    documents = {
        row.id: row for row in session.exec(select(Document).where(Document.id.in_(doc_ids))).all()
    } if doc_ids else {}
    jobs = {
        row.id: row for row in session.exec(select(ExtractionJob).where(ExtractionJob.id.in_(job_ids))).all()
    } if job_ids else {}
    return chunks, documents, jobs


def write_provenance(session: Session, ks_id: int, output_dir: Path) -> list[dict]:
    """Export normalized statement provenance with exact prompt snapshots and review history."""
    output_dir.mkdir(parents=True, exist_ok=True)
    files: list[dict] = []
    audit_rows = session.exec(
        select(AuditEvent).where(AuditEvent.knowledge_system_id == ks_id).order_by(AuditEvent.id)
    ).all()

    for layer, model, key_name in (
        ("tbox", AxiomProvenance, "axiom_key"),
        ("abox", AboxProvenance, "fact_key"),
    ):
        rows = session.exec(select(model).where(model.knowledge_system_id == ks_id)).all()
        chunks, documents, jobs = _source_context(session, rows)
        path = output_dir / f"{layer}-provenance.jsonl"
        digest = hashlib.sha256()
        count = 0
        with path.open("wb") as handle:
            for row in rows:
                key = getattr(row, key_name)
                chunk = chunks.get(row.chunk_id)
                document = documents.get(chunk.document_id) if chunk else None
                job = jobs.get(row.job_id)
                key_parts = [part for part in str(key).split("|")[1:] if len(part) >= 3]
                reviews = [
                    {
                        "id": event.id,
                        "action": event.action,
                        "actor": event.actor_name,
                        "summary": event.summary,
                        "created_at": event.created_at.isoformat(),
                    }
                    for event in audit_rows
                    if any(part in json.dumps(event.detail, ensure_ascii=False) for part in key_parts)
                ][-20:]
                payload = {
                    "layer": layer,
                    key_name: key,
                    "method": row.method,
                    "actor": row.actor_name,
                    "audit_event_id": row.audit_event_id,
                    "chunk": {
                        "id": chunk.id,
                        "index": chunk.idx,
                        "text": chunk.text,
                    } if chunk else None,
                    "document": {
                        "id": document.id,
                        "filename": document.original_filename,
                        "sha256": document.sha256,
                    } if document else None,
                    "extraction": {
                        "job_id": job.id,
                        "model": job.model,
                        "prompt_snapshot": job.prompt_snapshot,
                    } if job else None,
                    "reviews": ([row.review_record] if row.review_record else []) + reviews,
                    "recorded_at": row.created_at.isoformat(),
                }
                line = (json.dumps(payload, ensure_ascii=False, separators=(",", ":")) + "\n").encode("utf-8")
                handle.write(line)
                digest.update(line)
                count += 1
        files.append({
            "name": path.name,
            "records": count,
            "bytes": path.stat().st_size,
            "sha256": digest.hexdigest(),
        })
    return files


def write_manifest(output_dir: Path, manifest: dict) -> dict:
    path = output_dir / "manifest.json"
    content = json.dumps(manifest, ensure_ascii=False, indent=2).encode("utf-8")
    path.write_bytes(content)
    return {"name": path.name, "bytes": len(content), "sha256": hashlib.sha256(content).hexdigest()}


def finalize_release_version(snapshot_dir: Path, manifest: dict, version: str) -> dict:
    """Assign the public version when a reviewed draft is published.

    Capture/runtime fields live in the database manifest and are intentionally omitted from
    the portable artifact, matching the original capture format.
    """
    updated = {**manifest, "version": version}
    portable = {
        key: value
        for key, value in updated.items()
        if key not in {"capture_status", "quality_gate", "manifest_file", "requested_version"}
    }
    updated["manifest_file"] = write_manifest(snapshot_dir, portable)
    return updated


def capture_release(session: Session, ks: KnowledgeSystem, version: str, shard_size: int = 100_000) -> tuple[Path, dict]:
    root = settings.release_dir / ks.public_id / safe_name(version)
    if root.exists():
        shutil.rmtree(root)
    root.mkdir(parents=True)
    layers: dict[str, dict] = {}
    graph_map = graph_layers(ks)
    with ExitStack() as locks:
        for graph_iri in graph_map.values():
            locks.enter_context(store.read_lock(graph_iri))
        for layer, graph_iri in graph_map.items():
            files = write_graph_shards(
                graph_iri, root, layer,
                shard_size=shard_size if layer == "abox" else 1_000_000,
            )
            layers[layer] = {
                "graph_iri": graph_iri,
                "statements": sum(item["statements"] for item in files),
                "files": files,
            }
    provenance = write_provenance(session, ks.id, root)
    manifest = {
        "schema": "https://deeplethe.github.io/ontopilot/release-manifest/v1",
        "knowledge_system": {"id": ks.public_id, "name": ks.name, "base_iri": ks.base_iri},
        "version": version,
        "format": "application/n-quads",
        "compression": "none",
        "layers": layers,
        "provenance": provenance,
        "source_audit_event_id": session.exec(
            select(AuditEvent.id)
            .where(AuditEvent.knowledge_system_id == ks.id)
            .order_by(AuditEvent.id.desc())
        ).first(),
    }
    manifest_file = write_manifest(root, manifest)
    manifest["manifest_file"] = manifest_file
    return root, manifest


def copy_release_layer(snapshot_dir: Path, manifest: dict, layer: str, output_dir: Path) -> list[dict]:
    output_dir.mkdir(parents=True, exist_ok=True)
    files: list[dict] = []
    for item in manifest["layers"][layer]["files"]:
        source = snapshot_dir / item["name"]
        target = output_dir / item["name"]
        shutil.copyfile(source, target)
        files.append(dict(item))
    return files


def semantic_diff(left_root: Path, left: dict, right_root: Path, right: dict, sample_size: int = 20) -> dict:
    result: dict[str, dict] = {}
    for layer in ("tbox", "vocabulary", "abox"):
        def lines(root: Path, manifest: dict) -> Iterator[bytes]:
            for item in manifest["layers"][layer]["files"]:
                with (root / item["name"]).open("rb") as handle:
                    yield from handle

        left_hashes = {hashlib.sha256(line).digest() for line in lines(left_root, left)}
        right_hashes = {hashlib.sha256(line).digest() for line in lines(right_root, right)}
        added_hashes = right_hashes - left_hashes
        removed_hashes = left_hashes - right_hashes
        added: list[str] = []
        removed: list[str] = []
        for line in lines(right_root, right):
            if len(added) >= sample_size:
                break
            if hashlib.sha256(line).digest() in added_hashes:
                added.append(line.decode("utf-8").strip())
        for line in lines(left_root, left):
            if len(removed) >= sample_size:
                break
            if hashlib.sha256(line).digest() in removed_hashes:
                removed.append(line.decode("utf-8").strip())
        result[layer] = {
            "added": len(added_hashes),
            "removed": len(removed_hashes),
            "added_sample": added,
            "removed_sample": removed,
        }
    return {"layers": result}
