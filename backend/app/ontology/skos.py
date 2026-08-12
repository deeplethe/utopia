"""RDF-native controlled vocabularies for a knowledge system.

Each knowledge system gets a third named graph (beside its TBox and ABox) containing SKOS
ConceptSchemes and Concepts. Semantic content stays portable RDF; SQL is used only for the
human review workflow in :class:`TermProposal`.
"""
from __future__ import annotations

import re
import unicodedata
from datetime import datetime, timezone
from uuid import uuid4

from pyoxigraph import Literal, NamedNode

from app.ontology import store, vocab

SKOS = "http://www.w3.org/2004/02/skos/core#"
DCTERMS = "http://purl.org/dc/terms/"
ONTOPILOT = "http://ontopilot.local/vocab#"

SKOS_CONCEPT_SCHEME = NamedNode(SKOS + "ConceptScheme")
SKOS_CONCEPT = NamedNode(SKOS + "Concept")
SKOS_IN_SCHEME = NamedNode(SKOS + "inScheme")
SKOS_PREF_LABEL = NamedNode(SKOS + "prefLabel")
SKOS_ALT_LABEL = NamedNode(SKOS + "altLabel")
SKOS_HIDDEN_LABEL = NamedNode(SKOS + "hiddenLabel")
SKOS_BROADER = NamedNode(SKOS + "broader")
SKOS_RELATED = NamedNode(SKOS + "related")
SKOS_NOTATION = NamedNode(SKOS + "notation")
SKOS_DEFINITION = NamedNode(SKOS + "definition")

DCTERMS_TITLE = NamedNode(DCTERMS + "title")
DCTERMS_DESCRIPTION = NamedNode(DCTERMS + "description")
DCTERMS_CREATED = NamedNode(DCTERMS + "created")
DCTERMS_MODIFIED = NamedNode(DCTERMS + "modified")

OP_DEFAULT_LANGUAGE = NamedNode(ONTOPILOT + "defaultLanguage")
OP_STATUS = NamedNode(ONTOPILOT + "status")
OP_MAPS_TO = NamedNode(ONTOPILOT + "mapsTo")
OP_ORIGIN = NamedNode(ONTOPILOT + "origin")

_SCHEME_PREDS = {
    DCTERMS_TITLE, DCTERMS_DESCRIPTION, DCTERMS_MODIFIED, OP_DEFAULT_LANGUAGE, OP_ORIGIN,
}
_CONCEPT_PREDS = {
    SKOS_IN_SCHEME, SKOS_PREF_LABEL, SKOS_ALT_LABEL, SKOS_HIDDEN_LABEL, SKOS_BROADER,
    SKOS_RELATED, SKOS_NOTATION, SKOS_DEFINITION, DCTERMS_MODIFIED, OP_STATUS, OP_MAPS_TO,
    OP_ORIGIN,
}


class VocabularyValidationError(ValueError):
    pass


def graph_iri_for(ks) -> str:
    return f"{ks.graph_iri.rstrip('/')}/vocabulary"


def normalize_label(value: str) -> str:
    value = unicodedata.normalize("NFKC", value or "").casefold().strip()
    return re.sub(r"\s+", " ", value)


def _now() -> str:
    return datetime.now(timezone.utc).isoformat()


def _local(iri: str) -> str:
    return iri.rsplit("#", 1)[-1].rstrip("/").rsplit("/", 1)[-1]


def _literal(value: str, language: str | None = None) -> Literal:
    cleaned = (value or "").strip()
    if not cleaned:
        raise VocabularyValidationError("Label values cannot be empty")
    try:
        return Literal(cleaned, language=(language or "").strip() or None)
    except ValueError as exc:
        raise VocabularyValidationError(f"Invalid language tag: {language}") from exc


def _label(term: Literal) -> dict:
    return {"value": term.value, "language": term.language or ""}


def _labels(values: list[dict] | None, *, required: bool = False) -> list[dict]:
    out: list[dict] = []
    seen: set[tuple[str, str]] = set()
    for item in values or []:
        value = str(item.get("value", "")).strip()
        language = str(item.get("language", "")).strip()
        if not value:
            continue
        _literal(value, language)
        key = (normalize_label(value), language.casefold())
        if key not in seen:
            seen.add(key)
            out.append({"value": value, "language": language})
    if required and not out:
        raise VocabularyValidationError("At least one preferred label is required")
    return out


def _first_literal(values: list, fallback: str = "") -> str:
    return next((value.value for value in values if isinstance(value, Literal)), fallback)


def _subject_values(graph_iri: str) -> dict[str, list[tuple[NamedNode, object]]]:
    values: dict[str, list[tuple[NamedNode, object]]] = {}
    for subject, predicate, obj in store.read_triples(graph_iri):
        if isinstance(subject, NamedNode) and isinstance(predicate, NamedNode):
            values.setdefault(subject.value, []).append((predicate, obj))
    return values


def build_view(graph_iri: str) -> dict:
    subjects = _subject_values(graph_iri)
    scheme_iris: set[str] = set()
    concept_iris: set[str] = set()
    for iri, pairs in subjects.items():
        for predicate, obj in pairs:
            if predicate == vocab.RDF_TYPE and obj == SKOS_CONCEPT_SCHEME:
                scheme_iris.add(iri)
            elif predicate == vocab.RDF_TYPE and obj == SKOS_CONCEPT:
                concept_iris.add(iri)

    schemes: list[dict] = []
    for iri in scheme_iris:
        pairs = subjects.get(iri, [])
        titles = [_label(obj) for pred, obj in pairs if pred == DCTERMS_TITLE and isinstance(obj, Literal)]
        descriptions = [_label(obj) for pred, obj in pairs if pred == DCTERMS_DESCRIPTION and isinstance(obj, Literal)]
        schemes.append({
            "iri": iri,
            "title": titles[0]["value"] if titles else _local(iri),
            "titles": titles,
            "description": descriptions[0]["value"] if descriptions else "",
            "descriptions": descriptions,
            "default_language": _first_literal([obj for pred, obj in pairs if pred == OP_DEFAULT_LANGUAGE], "zh-CN"),
            "origin": _first_literal([obj for pred, obj in pairs if pred == OP_ORIGIN], "manual"),
            "created_at": _first_literal([obj for pred, obj in pairs if pred == DCTERMS_CREATED]),
            "modified_at": _first_literal([obj for pred, obj in pairs if pred == DCTERMS_MODIFIED]),
            "concept_count": 0,
        })

    concepts: list[dict] = []
    for iri in concept_iris:
        pairs = subjects.get(iri, [])
        pref = [_label(obj) for pred, obj in pairs if pred == SKOS_PREF_LABEL and isinstance(obj, Literal)]
        alt = [_label(obj) for pred, obj in pairs if pred == SKOS_ALT_LABEL and isinstance(obj, Literal)]
        hidden = [_label(obj) for pred, obj in pairs if pred == SKOS_HIDDEN_LABEL and isinstance(obj, Literal)]
        schemes_for_concept = [obj.value for pred, obj in pairs if pred == SKOS_IN_SCHEME and isinstance(obj, NamedNode)]
        broader = [obj.value for pred, obj in pairs if pred == SKOS_BROADER and isinstance(obj, NamedNode)]
        related = [obj.value for pred, obj in pairs if pred == SKOS_RELATED and isinstance(obj, NamedNode)]
        mapped = [obj.value for pred, obj in pairs if pred == OP_MAPS_TO and isinstance(obj, NamedNode)]
        concept = {
            "iri": iri,
            "scheme_iri": schemes_for_concept[0] if schemes_for_concept else "",
            "pref_labels": pref,
            "alt_labels": alt,
            "hidden_labels": hidden,
            "display_label": pref[0]["value"] if pref else _local(iri),
            "description": _first_literal([obj for pred, obj in pairs if pred == SKOS_DEFINITION]),
            "notation": _first_literal([obj for pred, obj in pairs if pred == SKOS_NOTATION]),
            "broader": broader,
            "related": related,
            "mapped_entity_iri": mapped[0] if mapped else None,
            "status": _first_literal([obj for pred, obj in pairs if pred == OP_STATUS], "active"),
            "origin": _first_literal([obj for pred, obj in pairs if pred == OP_ORIGIN], "manual"),
            "created_at": _first_literal([obj for pred, obj in pairs if pred == DCTERMS_CREATED]),
            "modified_at": _first_literal([obj for pred, obj in pairs if pred == DCTERMS_MODIFIED]),
        }
        concepts.append(concept)

    concept_by_iri = {concept["iri"]: concept for concept in concepts}
    for concept in concepts:
        concept["broader_labels"] = [
            concept_by_iri.get(iri, {"display_label": _local(iri)})["display_label"]
            for iri in concept["broader"]
        ]
        concept["related_labels"] = [
            concept_by_iri.get(iri, {"display_label": _local(iri)})["display_label"]
            for iri in concept["related"]
        ]
    counts: dict[str, int] = {}
    for concept in concepts:
        counts[concept["scheme_iri"]] = counts.get(concept["scheme_iri"], 0) + 1
    for scheme in schemes:
        scheme["concept_count"] = counts.get(scheme["iri"], 0)

    schemes.sort(key=lambda item: item["title"].casefold())
    concepts.sort(key=lambda item: item["display_label"].casefold())
    return {
        "schemes": schemes,
        "concepts": concepts,
        "stats": {
            "scheme_count": len(schemes),
            "concept_count": len(concepts),
            "label_count": sum(
                len(concept["pref_labels"]) + len(concept["alt_labels"]) + len(concept["hidden_labels"])
                for concept in concepts
            ),
            "mapped_count": sum(1 for concept in concepts if concept["mapped_entity_iri"]),
            "unmapped_count": sum(1 for concept in concepts if not concept["mapped_entity_iri"]),
        },
    }


def get_scheme(graph_iri: str, iri: str) -> dict | None:
    return next((item for item in build_view(graph_iri)["schemes"] if item["iri"] == iri), None)


def get_concept(graph_iri: str, iri: str) -> dict | None:
    return next((item for item in build_view(graph_iri)["concepts"] if item["iri"] == iri), None)


def _remove_predicates(graph_iri: str, subject: NamedNode, predicates: set[NamedNode]) -> None:
    for predicate in predicates:
        store.remove_pattern(graph_iri, subject, predicate, None)


def create_scheme(graph_iri: str, data: dict) -> dict:
    title = str(data.get("title", "")).strip()
    if not title:
        raise VocabularyValidationError("Vocabulary title is required")
    language = str(data.get("default_language", "zh-CN")).strip() or "zh-CN"
    description = str(data.get("description", "")).strip()
    origin = str(data.get("origin", "manual")).strip() or "manual"
    iri = str(data.get("iri") or f"{graph_iri}#scheme-{uuid4().hex[:12]}")
    if get_scheme(graph_iri, iri):
        raise VocabularyValidationError("A vocabulary with this IRI already exists")
    now = _now()
    node = NamedNode(iri)
    triples = [
        (node, vocab.RDF_TYPE, SKOS_CONCEPT_SCHEME),
        (node, DCTERMS_TITLE, _literal(title, language)),
        (node, OP_DEFAULT_LANGUAGE, Literal(language)),
        (node, OP_ORIGIN, Literal(origin)),
        (node, DCTERMS_CREATED, Literal(now)),
        (node, DCTERMS_MODIFIED, Literal(now)),
    ]
    if description:
        triples.append((node, DCTERMS_DESCRIPTION, _literal(description, language)))
    store.add_triples(graph_iri, triples)
    return get_scheme(graph_iri, iri)


def update_scheme(graph_iri: str, iri: str, data: dict) -> dict:
    existing = get_scheme(graph_iri, iri)
    if not existing:
        raise VocabularyValidationError("Vocabulary not found")
    title = str(data.get("title", existing["title"])).strip()
    if not title:
        raise VocabularyValidationError("Vocabulary title is required")
    language = str(data.get("default_language", existing["default_language"])).strip() or "zh-CN"
    description = str(data.get("description", existing["description"])).strip()
    origin = str(data.get("origin", existing.get("origin", "manual"))).strip() or "manual"
    node = NamedNode(iri)
    _remove_predicates(graph_iri, node, _SCHEME_PREDS)
    triples = [
        (node, DCTERMS_TITLE, _literal(title, language)),
        (node, OP_DEFAULT_LANGUAGE, Literal(language)),
        (node, OP_ORIGIN, Literal(origin)),
        (node, DCTERMS_MODIFIED, Literal(_now())),
    ]
    if description:
        triples.append((node, DCTERMS_DESCRIPTION, _literal(description, language)))
    store.add_triples(graph_iri, triples)
    return get_scheme(graph_iri, iri)


def delete_scheme(graph_iri: str, iri: str) -> int:
    view = build_view(graph_iri)
    concept_iris = [item["iri"] for item in view["concepts"] if item["scheme_iri"] == iri]
    removed = sum(store.remove_entity(graph_iri, concept_iri) for concept_iri in concept_iris)
    return removed + store.remove_entity(graph_iri, iri)


def _validate_concept(graph_iri: str, data: dict, *, exclude_iri: str | None = None) -> dict:
    view = build_view(graph_iri)
    scheme_iri = str(data.get("scheme_iri", "")).strip()
    if not any(scheme["iri"] == scheme_iri for scheme in view["schemes"]):
        raise VocabularyValidationError("Vocabulary scheme not found")
    pref = _labels(data.get("pref_labels"), required=True)
    alt = _labels(data.get("alt_labels"))
    hidden = _labels(data.get("hidden_labels"))

    pref_languages: set[str] = set()
    for label in pref:
        language = label["language"].casefold()
        if language in pref_languages:
            raise VocabularyValidationError("A concept may have only one preferred label per language")
        pref_languages.add(language)

    incoming = {
        (normalize_label(label["value"]), label["language"].casefold())
        for label in pref + alt + hidden
    }
    if len(incoming) != len(pref) + len(alt) + len(hidden):
        raise VocabularyValidationError("The same label cannot be preferred, alternative, or hidden twice")
    for concept in view["concepts"]:
        if concept["iri"] == exclude_iri or concept["scheme_iri"] != scheme_iri:
            continue
        existing = {
            (normalize_label(label["value"]), label["language"].casefold())
            for label in concept["pref_labels"] + concept["alt_labels"] + concept["hidden_labels"]
        }
        overlap = incoming & existing
        if overlap:
            duplicate = next(iter(overlap))[0]
            raise VocabularyValidationError(
                f'Label "{duplicate}" is already used by concept "{concept["display_label"]}"'
            )

    broader = list(dict.fromkeys(str(iri).strip() for iri in data.get("broader", []) if str(iri).strip()))
    related = list(dict.fromkeys(str(iri).strip() for iri in data.get("related", []) if str(iri).strip()))
    concept_by_iri = {concept["iri"]: concept for concept in view["concepts"]}
    current_iri = exclude_iri
    for relation_iri in broader + related:
        target = concept_by_iri.get(relation_iri)
        if not target or target["scheme_iri"] != scheme_iri:
            raise VocabularyValidationError("Broader and related concepts must exist in the same vocabulary")
        if current_iri and relation_iri == current_iri:
            raise VocabularyValidationError("A concept cannot relate to itself")

    if current_iri:
        adjacency = {concept["iri"]: set(concept["broader"]) for concept in view["concepts"]}
        adjacency[current_iri] = set(broader)

        def reaches(start: str, target: str) -> bool:
            seen: set[str] = set()
            stack = [start]
            while stack:
                node = stack.pop()
                if node == target:
                    return True
                if node in seen:
                    continue
                seen.add(node)
                stack.extend(adjacency.get(node, ()))
            return False

        if any(reaches(parent, current_iri) for parent in broader):
            raise VocabularyValidationError("Broader relations cannot form a cycle")

    status = str(data.get("status", "active")).strip() or "active"
    if status not in {"active", "deprecated"}:
        raise VocabularyValidationError("Status must be active or deprecated")
    mapped_value = data.get("mapped_entity_iri")
    mapped_entity_iri = str(mapped_value).strip() if mapped_value is not None else ""
    if mapped_entity_iri:
        try:
            NamedNode(mapped_entity_iri)
        except ValueError as exc:
            raise VocabularyValidationError("Ontology mapping must be an absolute IRI") from exc
    origin = str(data.get("origin", "manual")).strip() or "manual"
    if origin not in {"manual", "extraction", "agent"}:
        raise VocabularyValidationError("Origin must be manual, extraction, or agent")
    return {
        "scheme_iri": scheme_iri,
        "pref_labels": pref,
        "alt_labels": alt,
        "hidden_labels": hidden,
        "description": str(data.get("description", "")).strip(),
        "notation": str(data.get("notation", "")).strip(),
        "broader": broader,
        "related": related,
        "mapped_entity_iri": mapped_entity_iri or None,
        "status": status,
        "origin": origin,
    }


def _concept_triples(iri: str, data: dict, *, created_at: str | None = None) -> list[tuple]:
    node = NamedNode(iri)
    now = _now()
    triples: list[tuple] = [
        (node, vocab.RDF_TYPE, SKOS_CONCEPT),
        (node, SKOS_IN_SCHEME, NamedNode(data["scheme_iri"])),
        (node, OP_STATUS, Literal(data["status"])),
        (node, OP_ORIGIN, Literal(data.get("origin", "manual"))),
        (node, DCTERMS_MODIFIED, Literal(now)),
    ]
    if created_at:
        triples.append((node, DCTERMS_CREATED, Literal(created_at)))
    for label in data["pref_labels"]:
        triples.append((node, SKOS_PREF_LABEL, _literal(label["value"], label["language"])))
    for label in data["alt_labels"]:
        triples.append((node, SKOS_ALT_LABEL, _literal(label["value"], label["language"])))
    for label in data["hidden_labels"]:
        triples.append((node, SKOS_HIDDEN_LABEL, _literal(label["value"], label["language"])))
    if data["description"]:
        language = data["pref_labels"][0]["language"]
        triples.append((node, SKOS_DEFINITION, _literal(data["description"], language)))
    if data["notation"]:
        triples.append((node, SKOS_NOTATION, Literal(data["notation"])))
    for parent in data["broader"]:
        triples.append((node, SKOS_BROADER, NamedNode(parent)))
    for related in data["related"]:
        triples.append((node, SKOS_RELATED, NamedNode(related)))
        triples.append((NamedNode(related), SKOS_RELATED, node))
    if data["mapped_entity_iri"]:
        triples.append((node, OP_MAPS_TO, NamedNode(data["mapped_entity_iri"])))
    return triples


def create_concept(graph_iri: str, data: dict) -> dict:
    cleaned = _validate_concept(graph_iri, data)
    iri = str(data.get("iri") or f"{graph_iri}#concept-{uuid4().hex[:16]}")
    if get_concept(graph_iri, iri):
        raise VocabularyValidationError("A concept with this IRI already exists")
    store.add_triples(graph_iri, _concept_triples(iri, cleaned, created_at=_now()))
    return get_concept(graph_iri, iri)


def update_concept(graph_iri: str, iri: str, data: dict) -> dict:
    existing = get_concept(graph_iri, iri)
    if not existing:
        raise VocabularyValidationError("Concept not found")
    source = dict(data)
    source.setdefault("origin", existing.get("origin", "manual"))
    cleaned = _validate_concept(graph_iri, source, exclude_iri=iri)
    node = NamedNode(iri)
    _remove_predicates(graph_iri, node, _CONCEPT_PREDS)
    store.remove_pattern(graph_iri, None, SKOS_RELATED, node)
    store.add_triples(graph_iri, _concept_triples(iri, cleaned))
    return get_concept(graph_iri, iri)


def delete_concept(graph_iri: str, iri: str) -> int:
    return store.remove_entity(graph_iri, iri)


def list_concepts(
    graph_iri: str, *, scheme_iri: str | None = None, q: str | None = None,
    status: str | None = None, mapping: str | None = None, origin: str | None = None,
    start_date: str | None = None, end_date: str | None = None,
    limit: int = 100, offset: int = 0,
) -> dict:
    concepts = build_view(graph_iri)["concepts"]
    if scheme_iri:
        concepts = [concept for concept in concepts if concept["scheme_iri"] == scheme_iri]
    if status:
        concepts = [concept for concept in concepts if concept["status"] == status]
    if mapping == "mapped":
        concepts = [concept for concept in concepts if concept["mapped_entity_iri"]]
    elif mapping == "standalone":
        concepts = [concept for concept in concepts if not concept["mapped_entity_iri"]]
    if origin:
        concepts = [concept for concept in concepts if concept["origin"] == origin]
    if start_date or end_date:
        concepts = [
            concept for concept in concepts
            if (
                (not start_date or (concept["modified_at"] or concept["created_at"])[:10] >= start_date)
                and (not end_date or (concept["modified_at"] or concept["created_at"])[:10] <= end_date)
            )
        ]
    if q and q.strip():
        term = normalize_label(q)
        concepts = [
            concept for concept in concepts
            if term in normalize_label(" ".join(
                [concept["description"], concept["notation"]]
                + [label["value"] for label in concept["pref_labels"] + concept["alt_labels"] + concept["hidden_labels"]]
            ))
        ]
    return {"items": concepts[offset: offset + limit], "total": len(concepts)}


def resolve(graph_iri: str, text: str, *, language: str | None = None, limit: int = 10) -> dict:
    query = normalize_label(text)
    matches: list[dict] = []
    for concept in build_view(graph_iri)["concepts"]:
        if concept["status"] != "active":
            continue
        for kind, labels, exact_score in (
            ("preferred", concept["pref_labels"], 1.0),
            ("alternative", concept["alt_labels"], 0.98),
            ("hidden", concept["hidden_labels"], 0.95),
        ):
            for label in labels:
                if language and label["language"] and label["language"].casefold() != language.casefold():
                    continue
                normalized = normalize_label(label["value"])
                score = exact_score if query == normalized else (0.72 if query and query in normalized else 0.0)
                if score:
                    matches.append({
                        "concept": concept,
                        "matched_label": label,
                        "match_type": kind,
                        "score": score,
                    })
    matches.sort(key=lambda item: (-item["score"], item["concept"]["display_label"].casefold()))
    seen: set[str] = set()
    unique = []
    for match in matches:
        iri = match["concept"]["iri"]
        if iri not in seen:
            seen.add(iri)
            unique.append(match)
    return {"items": unique[:limit], "total": len(unique)}


def mapped_aliases(graph_iri: str) -> dict[str, str]:
    candidates: dict[str, set[str]] = {}
    for concept in build_view(graph_iri)["concepts"]:
        target = concept.get("mapped_entity_iri")
        if concept["status"] != "active" or not target:
            continue
        for label in concept["pref_labels"] + concept["alt_labels"] + concept["hidden_labels"]:
            candidates.setdefault(normalize_label(label["value"]), set()).add(target)
    return {label: next(iter(targets)) for label, targets in candidates.items() if len(targets) == 1}


def mapped_labels_by_entity(graph_iri: str) -> dict[str, list[str]]:
    out: dict[str, list[str]] = {}
    for concept in build_view(graph_iri)["concepts"]:
        target = concept.get("mapped_entity_iri")
        if concept["status"] != "active" or not target:
            continue
        for label in concept["pref_labels"] + concept["alt_labels"]:
            value = label["value"]
            if value not in out.setdefault(target, []):
                out[target].append(value)
    return out


def normalization_labels(graph_iri: str, entity_labels: dict[str, str]) -> dict[str, str]:
    aliases = mapped_aliases(graph_iri)
    return {
        alias: entity_labels[target]
        for alias, target in aliases.items()
        if target in entity_labels
    }


def normalize_ontology_delta(ontology: dict, aliases: dict[str, str]) -> dict:
    if not aliases:
        return ontology

    def canonical(value):
        if not isinstance(value, str):
            return value
        return aliases.get(normalize_label(value), value)

    out = {key: list(value) if isinstance(value, list) else value for key, value in ontology.items()}
    for item in out.get("classes", []) or []:
        if isinstance(item, dict):
            item["label"] = canonical(item.get("label"))
    for key in ("object_properties", "data_properties"):
        for item in out.get(key, []) or []:
            if isinstance(item, dict):
                item["label"] = canonical(item.get("label"))
                item["domain"] = canonical(item.get("domain"))
                if key == "object_properties":
                    item["range"] = canonical(item.get("range"))
    for key, fields in (
        ("subclass_of", ("sub", "super", "child", "parent", "subclass", "superclass")),
        ("disjoint_with", ("a", "b")),
        ("equivalent_class", ("a", "b")),
    ):
        for item in out.get(key, []) or []:
            if isinstance(item, dict):
                for field in fields:
                    if field in item:
                        item[field] = canonical(item[field])
    return out
