"""Token-authenticated, release-fixed read-only knowledge service."""
from __future__ import annotations

from dataclasses import dataclass

from fastapi import APIRouter, Depends, Header, HTTPException, Query, Response
from pyoxigraph import NamedNode, QueryBoolean, QuerySolutions
from sqlmodel import Session, select

from app.api.external import (
    ExternalAccess,
    SparqlRequest,
    _FORBIDDEN_SPARQL,
    _query_form,
    _sparql_code,
    _term_json,
    external_access,
)
from app.config import settings
from app.db.database import get_session
from app.db.models import OntologyRelease, ReleaseDeployment
from app.ontology import abox, abox_provenance, release_service, schema, skos, store, vocab

router = APIRouter(prefix="/api/v1/knowledge-systems", tags=["published release api"])


@dataclass
class ServedReleaseAccess:
    external: ExternalAccess
    release: OntologyRelease
    deployment: ReleaseDeployment
    pinned: bool

    def require(self, scope: str) -> None:
        self.external.require(scope)


def _deployment_for_release(session: Session, release: OntologyRelease) -> ReleaseDeployment:
    deployment = release_service.deployment_for(session, release.id)
    if deployment is None or deployment.status in {"stopping", "stopped", "failed"}:
        raise HTTPException(status_code=410, detail="Release service is not available")
    if deployment.status != "active":
        raise HTTPException(
            status_code=503,
            detail="Release service is being provisioned",
            headers={"Retry-After": "2"},
        )
    return deployment


def served_release_access(
    public_id: str,
    version: str | None = None,
    authorization: str | None = Header(default=None, alias="Authorization"),
    session: Session = Depends(get_session),
) -> ServedReleaseAccess:
    access = external_access(public_id, authorization, session)
    if version is not None:
        release = session.exec(
            select(OntologyRelease).where(
                OntologyRelease.knowledge_system_id == access.knowledge_system.id,
                OntologyRelease.version == version,
            )
        ).first()
        if release is None:
            raise HTTPException(status_code=404, detail="Release not found")
        if release.status == "deleted":
            raise HTTPException(status_code=410, detail="Release has been deleted")
        if release.status != "published":
            raise HTTPException(status_code=404, detail="Published release not found")
        return ServedReleaseAccess(access, release, _deployment_for_release(session, release), True)

    release = session.exec(
        select(OntologyRelease)
        .where(
            OntologyRelease.knowledge_system_id == access.knowledge_system.id,
            OntologyRelease.status == "published",
        )
        .order_by(OntologyRelease.published_at.desc())
    ).first()
    if release is None:
        raise HTTPException(status_code=404, detail="No published release is available")
    return ServedReleaseAccess(access, release, _deployment_for_release(session, release), False)


def _headers(response: Response, access: ServedReleaseAccess) -> None:
    response.headers["X-OntoPilot-Release"] = access.release.version
    digest = access.release.manifest.get("manifest_file", {}).get("sha256")
    if digest:
        response.headers["ETag"] = f'"{digest}"'
    response.headers["Cache-Control"] = (
        "private, max-age=31536000, immutable"
        if access.pinned
        else "private, no-cache"
    )


def _labels(tbox_graph_iri: str) -> tuple[dict[str, str], dict[str, str]]:
    view = schema.build_view(tbox_graph_iri)
    class_labels = {item["iri"]: item.get("label") or item["iri"] for item in view["classes"]}
    property_labels = {
        item["iri"]: item.get("label") or item["iri"]
        for item in view["object_properties"] + view["data_properties"]
    }
    return class_labels, property_labels


@router.get("/{public_id}/published")
@router.get("/{public_id}/releases/{version}")
def get_release_metadata(
    response: Response,
    access: ServedReleaseAccess = Depends(served_release_access),
) -> dict:
    _headers(response, access)
    ks = access.external.knowledge_system
    deployment = access.deployment
    with store.use_store(release_service.get_store()):
        terminology = skos.build_view(deployment.vocabulary_graph_iri)
    return {
        "id": ks.public_id,
        "name": ks.name,
        "description": ks.description,
        "base_iri": ks.base_iri,
        "release": {
            "id": access.release.id,
            "version": access.release.version,
            "published_at": access.release.published_at.isoformat() if access.release.published_at else None,
            "manifest_sha256": access.release.manifest.get("manifest_file", {}).get("sha256"),
        },
        "stats": {
            "statements": deployment.statement_count,
            "controlled_terms": terminology["stats"]["concept_count"],
        },
        "scopes": access.external.token.scopes,
    }


@router.get("/{public_id}/published/manifest")
@router.get("/{public_id}/releases/{version}/manifest")
def get_release_manifest(
    response: Response,
    access: ServedReleaseAccess = Depends(served_release_access),
) -> dict:
    access.require("ontology:read")
    _headers(response, access)
    return access.release.manifest


@router.get("/{public_id}/published/ontology")
@router.get("/{public_id}/releases/{version}/ontology")
def get_release_ontology(
    response: Response,
    access: ServedReleaseAccess = Depends(served_release_access),
) -> dict:
    access.require("ontology:read")
    _headers(response, access)
    with store.use_store(release_service.get_store()):
        view = schema.build_view(access.deployment.tbox_graph_iri)
    view["knowledge_system"] = {
        "id": access.external.knowledge_system.public_id,
        "name": access.external.knowledge_system.name,
        "base_iri": access.external.knowledge_system.base_iri,
        "release": access.release.version,
    }
    return view


@router.get("/{public_id}/published/export")
@router.get("/{public_id}/releases/{version}/export")
def export_release_ontology(
    fmt: str = "turtle",
    access: ServedReleaseAccess = Depends(served_release_access),
) -> Response:
    access.require("ontology:read")
    if fmt not in store.EXPORT_FORMATS:
        raise HTTPException(status_code=400, detail=f"Unsupported format: {fmt}")
    _, media_type, _ = store.EXPORT_FORMATS[fmt]
    with store.use_store(release_service.get_store()):
        content = store.serialize_graph(access.deployment.tbox_graph_iri, fmt)
    response = Response(content=content, media_type=media_type)
    _headers(response, access)
    return response


@router.get("/{public_id}/published/classes")
@router.get("/{public_id}/releases/{version}/classes")
def list_release_classes(
    response: Response,
    access: ServedReleaseAccess = Depends(served_release_access),
) -> dict:
    access.require("instances:read")
    _headers(response, access)
    with store.use_store(release_service.get_store()):
        class_labels, _ = _labels(access.deployment.tbox_graph_iri)
        counts = abox.counts_by_class(access.deployment.abox_graph_iri)
    classes = [
        {"iri": iri, "label": label, "count": counts.get(iri, 0)}
        for iri, label in class_labels.items()
    ]
    classes.sort(key=lambda item: (-item["count"], item["label"]))
    return {"classes": classes, "total": sum(counts.values())}


@router.get("/{public_id}/published/individuals")
@router.get("/{public_id}/releases/{version}/individuals")
def list_release_individuals(
    response: Response,
    class_iri: str | None = None,
    q: str | None = None,
    limit: int = Query(default=20, ge=1, le=200),
    offset: int = Query(default=0, ge=0),
    access: ServedReleaseAccess = Depends(served_release_access),
) -> dict:
    access.require("instances:read")
    _headers(response, access)
    with store.use_store(release_service.get_store()):
        class_labels, _ = _labels(access.deployment.tbox_graph_iri)
        items, total = abox.list_individuals(
            access.deployment.abox_graph_iri,
            class_labels,
            class_iri=class_iri,
            q=q,
            limit=limit,
            offset=offset,
        )
    return {"items": items, "total": total}


@router.get("/{public_id}/published/individual")
@router.get("/{public_id}/releases/{version}/individual")
def get_release_individual(
    iri: str,
    response: Response,
    access: ServedReleaseAccess = Depends(served_release_access),
) -> dict:
    access.require("instances:read")
    _headers(response, access)
    with store.use_store(release_service.get_store()):
        class_labels, property_labels = _labels(access.deployment.tbox_graph_iri)
        individual = abox.get_individual(
            access.deployment.abox_graph_iri,
            iri,
            class_labels,
            property_labels,
        )
    if individual is None:
        raise HTTPException(status_code=404, detail="Individual not found")
    if "provenance:read" not in access.external.token.scopes:
        return individual
    keys = [abox_provenance.ind_key(iri)]
    keys += [
        abox_provenance.data_key(iri, item["prop"], item["value"])
        for item in individual["data_assertions"]
    ]
    keys += [
        abox_provenance.obj_key(iri, item["prop"], item["target"])
        for item in individual["object_assertions"]
    ]
    sources = release_service.abox_sources(access.external.session, access.release.id, keys)
    individual["sources"] = sources.get(abox_provenance.ind_key(iri), [])
    for item in individual["data_assertions"]:
        item["sources"] = sources.get(
            abox_provenance.data_key(iri, item["prop"], item["value"]),
            [],
        )
    for item in individual["object_assertions"]:
        item["sources"] = sources.get(
            abox_provenance.obj_key(iri, item["prop"], item["target"]),
            [],
        )
    return individual


@router.get("/{public_id}/published/vocabulary/schemes")
@router.get("/{public_id}/releases/{version}/vocabulary/schemes")
def list_release_vocabularies(
    response: Response,
    access: ServedReleaseAccess = Depends(served_release_access),
) -> dict:
    access.require("vocabulary:read")
    _headers(response, access)
    with store.use_store(release_service.get_store()):
        view = skos.build_view(access.deployment.vocabulary_graph_iri)
    return {"items": view["schemes"], "total": len(view["schemes"]), "stats": view["stats"]}


@router.get("/{public_id}/published/vocabulary/concepts")
@router.get("/{public_id}/releases/{version}/vocabulary/concepts")
def list_release_concepts(
    response: Response,
    scheme_iri: str | None = None,
    q: str | None = None,
    status: str | None = "active",
    limit: int = Query(default=100, ge=1, le=1000),
    offset: int = Query(default=0, ge=0),
    access: ServedReleaseAccess = Depends(served_release_access),
) -> dict:
    access.require("vocabulary:read")
    _headers(response, access)
    with store.use_store(release_service.get_store()):
        return skos.list_concepts(
            access.deployment.vocabulary_graph_iri,
            scheme_iri=scheme_iri,
            q=q,
            status=status,
            limit=limit,
            offset=offset,
        )


@router.get("/{public_id}/published/vocabulary/resolve")
@router.get("/{public_id}/releases/{version}/vocabulary/resolve")
def resolve_release_term(
    response: Response,
    q: str = Query(min_length=1),
    language: str | None = None,
    limit: int = Query(default=10, ge=1, le=100),
    access: ServedReleaseAccess = Depends(served_release_access),
) -> dict:
    access.require("vocabulary:read")
    _headers(response, access)
    with store.use_store(release_service.get_store()):
        return skos.resolve(
            access.deployment.vocabulary_graph_iri,
            q,
            language=language,
            limit=limit,
        )


@router.get("/{public_id}/published/vocabulary/export")
@router.get("/{public_id}/releases/{version}/vocabulary/export")
def export_release_vocabulary(
    fmt: str = "turtle",
    access: ServedReleaseAccess = Depends(served_release_access),
) -> Response:
    access.require("vocabulary:read")
    if fmt not in store.EXPORT_FORMATS:
        raise HTTPException(status_code=400, detail=f"Unsupported format: {fmt}")
    _, media_type, _ = store.EXPORT_FORMATS[fmt]
    with store.use_store(release_service.get_store()):
        content = store.serialize_graph(access.deployment.vocabulary_graph_iri, fmt)
    response = Response(content=content, media_type=media_type)
    _headers(response, access)
    return response


@router.post("/{public_id}/published/query")
@router.post("/{public_id}/releases/{version}/query")
def query_release_graph(
    body: SparqlRequest,
    response: Response,
    access: ServedReleaseAccess = Depends(served_release_access),
) -> dict:
    access.require("query:read")
    _headers(response, access)
    query = body.query.strip()
    if len(query) > settings.external_query_max_chars:
        raise HTTPException(status_code=413, detail="SPARQL query is too large")
    form = _query_form(query)
    if form not in {"SELECT", "ASK"}:
        raise HTTPException(status_code=400, detail="Only SPARQL SELECT and ASK queries are allowed")
    if _FORBIDDEN_SPARQL.search(_sparql_code(query)):
        raise HTTPException(
            status_code=400,
            detail="SERVICE, FROM, GRAPH, and SPARQL update operations are not allowed",
        )
    ks = access.external.knowledge_system
    graphs = [
        NamedNode(access.deployment.tbox_graph_iri),
        NamedNode(access.deployment.abox_graph_iri),
        NamedNode(access.deployment.vocabulary_graph_iri),
    ]
    try:
        with store.use_store(release_service.get_store()):
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
        raise HTTPException(status_code=400, detail=f"Invalid SPARQL query: {exc}") from exc
    if isinstance(result, QueryBoolean):
        return {"head": {}, "boolean": bool(result)}
    if not isinstance(result, QuerySolutions):
        raise HTTPException(status_code=400, detail="Only SPARQL SELECT and ASK results are supported")
    variables = [variable.value for variable in result.variables]
    row_limit = min(body.max_rows, settings.external_query_max_rows)
    rows: list[dict] = []
    truncated = False
    for index, solution in enumerate(result):
        if index >= row_limit:
            truncated = True
            break
        binding = {}
        for variable in variables:
            term = solution[variable]
            if term is not None:
                binding[variable] = _term_json(term)
        rows.append(binding)
    return {
        "head": {"vars": variables},
        "results": {"bindings": rows},
        "truncated": truncated,
        "max_rows": row_limit,
    }
