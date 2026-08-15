"""Direct RDF import into a knowledge system's ontology and instance graphs."""
from __future__ import annotations

import hashlib
import logging
import secrets
from contextlib import ExitStack
from pathlib import Path
from typing import Any

from fastapi import APIRouter, Depends, File, Form, HTTPException, UploadFile
from sqlmodel import Session, select

from app import audit
from app.api.conflicts import sync_conflicts
from app.api.knowledge import refresh_ks_stats
from app.config import settings
from app.db.database import get_session
from app.db.models import Conflict, KnowledgeSystem, User
from app.permissions import extraction_active, ks_writer
from app.security import current_user
from app.ontology import (
    abox_validate,
    rdf_import,
    retrieval,
    schema,
    skos,
    statement_provenance,
    store,
    terminology_sync,
    workbench,
)

router = APIRouter(prefix="/api/knowledge", tags=["rdf-import"])
logger = logging.getLogger(__name__)


def _diff_count(data: bytes) -> int:
    return len(store.load_triples(data)) if data else 0


@router.post("/{ks_id}/rdf/import")
def import_rdf(
    file: UploadFile = File(...),
    target: str = Form(default="auto"),
    strategy: str = Form(default="merge"),
    format: str = Form(default="auto"),
    base_iri: str | None = Form(default=None),
    ks: KnowledgeSystem = Depends(ks_writer),
    user: User = Depends(current_user),
    session: Session = Depends(get_session),
) -> dict:
    """Parse RDF locally and write it directly, without invoking document extraction or an LLM."""
    target = target.strip().lower()
    strategy = strategy.strip().lower()
    if target not in {"auto", "tbox", "abox"}:
        raise HTTPException(status_code=400, detail="target must be auto, tbox, or abox")
    if strategy not in {"merge", "replace"}:
        raise HTTPException(status_code=400, detail="strategy must be merge or replace")
    if extraction_active(session, ks.id):
        raise HTTPException(status_code=409, detail="An extraction is in progress; try again after it finishes.")

    data = file.file.read(settings.rdf_import_max_bytes + 1)
    if len(data) > settings.rdf_import_max_bytes:
        raise HTTPException(
            status_code=413,
            detail=f"RDF file exceeds the {settings.rdf_import_max_bytes:,}-byte upload limit",
        )

    filename = Path(file.filename or "import.rdf").name
    source_sha256 = hashlib.sha256(data).hexdigest()
    effective_base_iri = (base_iri or "").strip() or ks.base_iri
    blank_node_scope = hashlib.sha256(
        f"{ks.graph_iri}\0{effective_base_iri}\0{target}\0".encode("utf-8") + data
    ).hexdigest()[:24]
    try:
        parsed = rdf_import.parse_rdf(
            data,
            filename=filename,
            requested_format=format,
            base_iri=effective_base_iri,
            max_triples=settings.rdf_import_max_triples,
            blank_node_scope=blank_node_scope,
        )
        if not parsed.triples:
            raise rdf_import.RdfImportError("The RDF document contains no triples")
        partition = rdf_import.partition_rdf(parsed.triples, target)
    except rdf_import.RdfImportError as exc:
        raise HTTPException(status_code=400, detail=str(exc)) from exc

    abox_iri = f"{ks.graph_iri.rstrip('/')}/abox"
    touch_tbox = bool(partition.tbox) or (strategy == "replace" and target in {"auto", "tbox"})
    touch_abox = bool(partition.abox) or (strategy == "replace" and target in {"auto", "abox"})
    captures: dict[str, Any] = {}
    terminology = {"terms_added": 0, "terms_mapped": 0, "terminology_error": None}
    vocabulary_added = b""
    vocabulary_removed = b""
    vocabulary_graph = skos.graph_iri_for(ks)
    try:
        with ExitStack() as stack:
            # Lock both semantic layers even when only one is written: validation must compare
            # one consistent TBox+ABox snapshot and no concurrent writer may slip between them.
            captures["tbox"] = stack.enter_context(
                store.capture(ks.graph_iri, revert_on_error=True)
            )
            captures["abox"] = stack.enter_context(
                store.capture(abox_iri, revert_on_error=True)
            )
            baseline_errors = workbench.structural_error_signatures(ks.graph_iri)

            if strategy == "replace":
                if touch_tbox:
                    store.clear_graph(ks.graph_iri)
                if touch_abox:
                    store.clear_graph(abox_iri)
            if partition.tbox:
                store.add_triples(ks.graph_iri, partition.tbox)
            if partition.abox:
                store.add_triples(abox_iri, partition.abox)

            new_errors = workbench.new_structural_errors(ks.graph_iri, baseline_errors)
            if new_errors:
                raise HTTPException(
                    status_code=422,
                    detail={
                        "code": "ontology_structural_validation_failed",
                        "message": "The RDF import introduces structural ontology errors.",
                        "new_error_count": len(new_errors),
                        "new_error_signatures": new_errors,
                    },
                )

            tbox_added, tbox_removed = captures["tbox"].diff()
            abox_added, abox_removed = captures["abox"].diff()
            tbox_changed = bool(tbox_added or tbox_removed)
            abox_changed = bool(abox_added or abox_removed)
            if tbox_changed:
                refresh_ks_stats(session, ks, commit=False)
                open_conflicts = sync_conflicts(session, ks, semantic=False, commit=False)
            else:
                open_conflicts = list(session.exec(
                    select(Conflict).where(
                        Conflict.knowledge_system_id == ks.id,
                        Conflict.status == "open",
                    ).order_by(Conflict.severity.desc(), Conflict.id)
                ).all())

            validation = abox_validate.validate(ks.graph_iri, abox_iri)
            detail = {
                "filename": filename,
                "sha256": source_sha256,
                "format": parsed.format,
                "target": target,
                "strategy": strategy,
                "base_iri": effective_base_iri,
                "parsed_triples": len(parsed.triples),
                "tbox_triples": len(partition.tbox),
                "abox_triples": len(partition.abox),
            }
            changed_graphs = int(tbox_changed) + int(abox_changed)
            group_id = secrets.token_hex(8) if changed_graphs > 1 else None
            if tbox_changed:
                tbox_event = audit.record(
                    session, ks_id=ks.id, action="rdf.import",
                    summary=f'Imported RDF ontology from "{filename}"',
                    actor_id=user.id, actor_name=user.username,
                    detail={**detail, "graph_target": "tbox"},
                    added=tbox_added, removed=tbox_removed, graph=ks.graph_iri,
                    group_id=group_id, commit=False,
                )
                statement_provenance.record_tbox_diff(
                    session, ks.id, tbox_added, tbox_removed, tbox_event, commit=False,
                )
            if abox_changed:
                abox_event = audit.record(
                    session, ks_id=ks.id, action="rdf.import",
                    summary=f'Imported RDF instances from "{filename}"',
                    actor_id=user.id, actor_name=user.username,
                    detail={**detail, "graph_target": "abox"},
                    added=abox_added, removed=abox_removed, graph=abox_iri,
                    group_id=group_id, commit=False,
                )
                statement_provenance.record_abox_diff(
                    session, ks.id, abox_added, abox_removed, abox_event, commit=False,
                )
            if changed_graphs:
                session.commit()

        # Terminology is derived from the committed TBox and intentionally isolated from the
        # core import transaction. A sync failure is reported but cannot split TBox/ABox state.
        if tbox_changed and settings.automatic_terminology:
            try:
                with store.capture(vocabulary_graph, revert_on_error=True) as vocabulary_capture:
                    sync_result = terminology_sync.sync_from_ontology(ks)
                    vocabulary_added, vocabulary_removed = vocabulary_capture.diff()
                    if vocabulary_added or vocabulary_removed:
                        audit.record(
                            session, ks_id=ks.id, action="terminology.sync",
                            summary=(
                                f'Synchronized controlled terminology after RDF import "{filename}": '
                                f'+{sync_result["terms_added"]} terms / {sync_result["terms_mapped"]} mappings'
                            ),
                            actor_id=user.id, actor_name=user.username,
                            detail={**detail, **sync_result}, added=vocabulary_added,
                            removed=vocabulary_removed, graph=vocabulary_graph,
                        )
                    terminology.update(sync_result)
            except Exception as exc:  # noqa: BLE001
                logger.exception("terminology sync failed after RDF import for KS %s", ks.id)
                terminology["terminology_error"] = str(exc)
    except HTTPException:
        session.rollback()
        raise
    except Exception:
        session.rollback()
        raise

    if tbox_changed:
        try:
            retrieval.invalidate(ks.graph_iri)
        except Exception:  # noqa: BLE001
            pass

    return {
        **detail,
        "tbox_added": _diff_count(tbox_added),
        "tbox_removed": _diff_count(tbox_removed),
        "abox_added": _diff_count(abox_added),
        "abox_removed": _diff_count(abox_removed),
        "view": schema.build_view(ks.graph_iri),
        "open_conflicts": open_conflicts,
        "validation": {
            "counts": validation["counts"],
            "truncated": validation["truncated"],
        },
        "terminology": terminology,
    }
