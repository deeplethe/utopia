"""Stable, token-authenticated, read-only API for external knowledge consumers."""
from __future__ import annotations

import re
from dataclasses import dataclass
from datetime import timedelta

from fastapi import APIRouter, Depends, Header, HTTPException, Query, Response
from pydantic import BaseModel, Field
from pyoxigraph import BlankNode, Literal, NamedNode, QueryBoolean, QuerySolutions
from sqlmodel import Session, select

from app import access_tokens
from app.api.knowledge import GRAPH_ROOT
from app.config import settings
from app.db.database import get_session
from app.db.models import KnowledgeApiToken, KnowledgeSystem, utcnow
from app.ontology import abox, abox_provenance, schema, skos, store, vocab

router = APIRouter(prefix="/api/v1/knowledge-systems", tags=["external query api"])


@dataclass
class ExternalAccess:
    knowledge_system: KnowledgeSystem
    token: KnowledgeApiToken
    session: Session

    def require(self, scope: str) -> None:
        if scope not in self.token.scopes:
            raise HTTPException(status_code=403, detail=f'Token lacks required scope "{scope}"')


def _unauthorized() -> HTTPException:
    return HTTPException(
        status_code=401,
        detail="Invalid or expired API token",
        headers={"WWW-Authenticate": "Bearer"},
    )


def external_access(
    public_id: str,
    authorization: str | None = Header(default=None, alias="Authorization"),
    session: Session = Depends(get_session),
) -> ExternalAccess:
    if not authorization:
        raise _unauthorized()
    scheme, separator, plaintext = authorization.partition(" ")
    if not separator or scheme.lower() != "bearer" or not plaintext.strip():
        raise _unauthorized()
    row = session.exec(
        select(KnowledgeApiToken).where(
            KnowledgeApiToken.token_hash == access_tokens.digest(plaintext.strip())
        )
    ).first()
    if not row or access_tokens.status(row) != "active":
        raise _unauthorized()
    ks = session.get(KnowledgeSystem, row.knowledge_system_id)
    if not ks or ks.public_id != public_id:
        raise _unauthorized()

    now = utcnow()
    if row.last_used_at is None or access_tokens.aware(row.last_used_at) < now - timedelta(minutes=5):
        row.last_used_at = now
        session.add(row)
        session.commit()
    return ExternalAccess(knowledge_system=ks, token=row, session=session)


def _abox_iri(ks: KnowledgeSystem) -> str:
    return f"{GRAPH_ROOT}/{ks.id}/abox"


def _vocabulary_iri(ks: KnowledgeSystem) -> str:
    return f"{GRAPH_ROOT}/{ks.id}/vocabulary"


def _labels(ks: KnowledgeSystem) -> tuple[dict[str, str], dict[str, str]]:
    view = schema.build_view(ks.graph_iri)
    class_labels = {c["iri"]: c.get("label") or c["iri"] for c in view["classes"]}
    property_labels = {
        p["iri"]: p.get("label") or p["iri"]
        for p in view["object_properties"] + view["data_properties"]
    }
    return class_labels, property_labels


@router.get("/{public_id}")
def get_public_metadata(access: ExternalAccess = Depends(external_access)) -> dict:
    ks = access.knowledge_system
    return {
        "id": ks.public_id,
        "name": ks.name,
        "description": ks.description,
        "base_iri": ks.base_iri,
        "stats": {
            "classes": ks.class_count,
            "properties": ks.property_count,
            "axioms": ks.axiom_count,
            "controlled_terms": skos.build_view(_vocabulary_iri(ks))["stats"]["concept_count"],
        },
        "scopes": access.token.scopes,
    }


@router.get("/{public_id}/ontology")
def get_public_ontology(access: ExternalAccess = Depends(external_access)) -> dict:
    access.require("ontology:read")
    view = schema.build_view(access.knowledge_system.graph_iri)
    view["knowledge_system"] = {
        "id": access.knowledge_system.public_id,
        "name": access.knowledge_system.name,
        "base_iri": access.knowledge_system.base_iri,
    }
    return view


@router.get("/{public_id}/export")
def export_public_ontology(
    fmt: str = "turtle", access: ExternalAccess = Depends(external_access),
) -> Response:
    access.require("ontology:read")
    if fmt not in store.EXPORT_FORMATS:
        raise HTTPException(status_code=400, detail=f"Unsupported format: {fmt}")
    _, media_type, _ = store.EXPORT_FORMATS[fmt]
    return Response(
        content=store.serialize_graph(access.knowledge_system.graph_iri, fmt),
        media_type=media_type,
    )


@router.get("/{public_id}/classes")
def list_public_classes(access: ExternalAccess = Depends(external_access)) -> dict:
    access.require("instances:read")
    class_labels, _ = _labels(access.knowledge_system)
    counts = abox.counts_by_class(_abox_iri(access.knowledge_system))
    classes = [
        {"iri": iri, "label": label, "count": counts.get(iri, 0)}
        for iri, label in class_labels.items()
    ]
    classes.sort(key=lambda item: (-item["count"], item["label"]))
    return {"classes": classes, "total": sum(counts.values())}


@router.get("/{public_id}/individuals")
def list_public_individuals(
    class_iri: str | None = None,
    q: str | None = None,
    limit: int = Query(default=20, ge=1, le=200),
    offset: int = Query(default=0, ge=0),
    access: ExternalAccess = Depends(external_access),
) -> dict:
    access.require("instances:read")
    class_labels, _ = _labels(access.knowledge_system)
    items, total = abox.list_individuals(
        _abox_iri(access.knowledge_system),
        class_labels,
        class_iri=class_iri,
        q=q,
        limit=limit,
        offset=offset,
    )
    return {"items": items, "total": total}


@router.get("/{public_id}/individual")
def get_public_individual(
    iri: str, access: ExternalAccess = Depends(external_access),
) -> dict:
    access.require("instances:read")
    ks = access.knowledge_system
    class_labels, property_labels = _labels(ks)
    individual = abox.get_individual(_abox_iri(ks), iri, class_labels, property_labels)
    if individual is None:
        raise HTTPException(status_code=404, detail="Individual not found")
    if "provenance:read" not in access.token.scopes:
        return individual

    keys = [abox_provenance.ind_key(iri)]
    keys += [abox_provenance.data_key(iri, item["prop"], item["value"])
             for item in individual["data_assertions"]]
    keys += [abox_provenance.obj_key(iri, item["prop"], item["target"])
             for item in individual["object_assertions"]]
    sources = abox_provenance.sources_for(access.session, ks.id, keys)
    individual["sources"] = sources.get(abox_provenance.ind_key(iri), [])
    for item in individual["data_assertions"]:
        item["sources"] = sources.get(abox_provenance.data_key(iri, item["prop"], item["value"]), [])
    for item in individual["object_assertions"]:
        item["sources"] = sources.get(abox_provenance.obj_key(iri, item["prop"], item["target"]), [])
    return individual


@router.get("/{public_id}/vocabulary/schemes")
def list_public_vocabularies(access: ExternalAccess = Depends(external_access)) -> dict:
    access.require("vocabulary:read")
    view = skos.build_view(_vocabulary_iri(access.knowledge_system))
    return {"items": view["schemes"], "total": len(view["schemes"]), "stats": view["stats"]}


@router.get("/{public_id}/vocabulary/concepts")
def list_public_concepts(
    scheme_iri: str | None = None,
    q: str | None = None,
    status: str | None = "active",
    limit: int = Query(default=100, ge=1, le=1000),
    offset: int = Query(default=0, ge=0),
    access: ExternalAccess = Depends(external_access),
) -> dict:
    access.require("vocabulary:read")
    return skos.list_concepts(
        _vocabulary_iri(access.knowledge_system), scheme_iri=scheme_iri, q=q,
        status=status, limit=limit, offset=offset,
    )


@router.get("/{public_id}/vocabulary/resolve")
def resolve_public_term(
    q: str = Query(min_length=1),
    language: str | None = None,
    limit: int = Query(default=10, ge=1, le=100),
    access: ExternalAccess = Depends(external_access),
) -> dict:
    access.require("vocabulary:read")
    return skos.resolve(_vocabulary_iri(access.knowledge_system), q, language=language, limit=limit)


@router.get("/{public_id}/vocabulary/export")
def export_public_vocabulary(
    fmt: str = "turtle", access: ExternalAccess = Depends(external_access),
) -> Response:
    access.require("vocabulary:read")
    if fmt not in store.EXPORT_FORMATS:
        raise HTTPException(status_code=400, detail=f"Unsupported format: {fmt}")
    _, media_type, _ = store.EXPORT_FORMATS[fmt]
    return Response(
        content=store.serialize_graph(_vocabulary_iri(access.knowledge_system), fmt),
        media_type=media_type,
    )


class SparqlRequest(BaseModel):
    query: str = Field(min_length=1)
    max_rows: int = Field(default=100, ge=1)


_QUERY_FORM = re.compile(
    r"(?<![\w?:$-])(?:SELECT|ASK|CONSTRUCT|DESCRIBE)\b",
    flags=re.IGNORECASE,
)
_FORBIDDEN_SPARQL = re.compile(
    r"(?<![\w?:$-])(?:SERVICE|FROM|GRAPH|LOAD|INSERT|DELETE|CLEAR|CREATE|DROP|COPY|MOVE|ADD|WITH)\b",
    flags=re.IGNORECASE,
)
_SPARQL_LITERAL_OR_IRI = re.compile(
    r'"""(?:\\.|(?!""").)*"""|\'\'\'(?:\\.|(?!\'\'\').)*\'\'\'|'
    r'"(?:\\.|[^"\\])*"|\'(?:\\.|[^\'\\])*\'|<[^>]*>',
    flags=re.DOTALL,
)


def _query_form(query: str) -> str | None:
    match = _QUERY_FORM.search(_sparql_code(query))
    return match.group().upper() if match else None


def _sparql_code(query: str) -> str:
    without_values = _SPARQL_LITERAL_OR_IRI.sub(lambda match: " " * len(match.group()), query)
    return re.sub(r"(?m)#.*$", "", without_values)


def _term_json(term) -> dict:
    if isinstance(term, NamedNode):
        return {"type": "uri", "value": term.value}
    if isinstance(term, BlankNode):
        return {"type": "bnode", "value": term.value}
    if isinstance(term, Literal):
        value = {"type": "literal", "value": term.value}
        if term.language:
            value["xml:lang"] = term.language
        elif term.datatype and term.datatype.value != f"{vocab.XSD}string":
            value["datatype"] = term.datatype.value
        return value
    raise TypeError(f"Unsupported RDF term: {type(term).__name__}")


@router.post("/{public_id}/query")
def query_public_graph(
    body: SparqlRequest, access: ExternalAccess = Depends(external_access),
) -> dict:
    access.require("query:read")
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

    ks = access.knowledge_system
    graphs = [NamedNode(ks.graph_iri), NamedNode(_abox_iri(ks)), NamedNode(_vocabulary_iri(ks))]
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
