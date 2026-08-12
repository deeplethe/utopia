"""Deterministic ontology-to-SKOS synchronization.

The extraction pipeline owns the formal ontology entities. This module creates the lexical
governance layer automatically: every named class/property gets a mapped SKOS concept, exact
existing labels are mapped without an LLM, and class hierarchy becomes ``skos:broader``.
Human labels are preserved; uncertain aliases and standalone domain terms remain the job of the
terminology agent and its review queue.
"""
from __future__ import annotations

import hashlib
import re

from pyoxigraph import NamedNode

from app.ontology import schema, skos, store

AUTO_SCHEME_SUFFIX = "#scheme-extracted"


def _language(text: str) -> str:
    return "zh-CN" if re.search(r"[\u3400-\u9fff]", text or "") else "en"


def _concept_payload(concept: dict) -> dict:
    return {
        "scheme_iri": concept["scheme_iri"],
        "pref_labels": concept["pref_labels"],
        "alt_labels": concept["alt_labels"],
        "hidden_labels": concept["hidden_labels"],
        "description": concept["description"],
        "notation": concept["notation"],
        "broader": concept["broader"],
        "related": concept["related"],
        "mapped_entity_iri": concept["mapped_entity_iri"],
        "status": concept["status"],
        "origin": concept.get("origin", "manual"),
    }


def _scheme_title(ks) -> tuple[str, str, str]:
    language = _language(ks.name)
    if language == "zh-CN":
        return f"{ks.name}术语表", "随本体抽取自动形成，并由人工持续治理的受控词表。", language
    return (
        f"{ks.name} terminology",
        "Controlled terminology formed automatically during ontology extraction and governed by humans.",
        language,
    )


def ensure_scheme(ks) -> dict | None:
    ontology = schema.build_view(ks.graph_iri)
    entities = ontology["classes"] + ontology["object_properties"] + ontology["data_properties"]
    if not entities:
        return None
    graph_iri = skos.graph_iri_for(ks)
    view = skos.build_view(graph_iri)
    fixed_iri = f"{graph_iri}{AUTO_SCHEME_SUFFIX}"
    fixed = next((scheme for scheme in view["schemes"] if scheme["iri"] == fixed_iri), None)
    if fixed:
        return fixed
    if len(view["schemes"]) == 1:
        return view["schemes"][0]
    generated = [scheme for scheme in view["schemes"] if scheme.get("origin") == "extraction"]
    if generated:
        return max(generated, key=lambda scheme: scheme["concept_count"])
    if view["schemes"]:
        mapped_counts: dict[str, int] = {}
        for concept in view["concepts"]:
            if concept.get("mapped_entity_iri"):
                mapped_counts[concept["scheme_iri"]] = mapped_counts.get(concept["scheme_iri"], 0) + 1
        return max(
            view["schemes"],
            key=lambda scheme: (mapped_counts.get(scheme["iri"], 0), scheme["concept_count"]),
        )
    title, description, language = _scheme_title(ks)
    return skos.create_scheme(graph_iri, {
        "iri": fixed_iri,
        "title": title,
        "description": description,
        "default_language": language,
        "origin": "extraction",
    })


def sync_from_ontology(ks) -> dict:
    """Idempotently synchronize deterministic terminology from the current ontology."""
    ontology = schema.build_view(ks.graph_iri)
    entities = [
        *(dict(entity, entity_kind="class") for entity in ontology["classes"]),
        *(dict(entity, entity_kind="object_property") for entity in ontology["object_properties"]),
        *(dict(entity, entity_kind="data_property") for entity in ontology["data_properties"]),
    ]
    result = {
        "scheme_iri": None,
        "terms_added": 0,
        "terms_mapped": 0,
        "aliases_added": 0,
        "broader_added": 0,
        "stale_mappings_removed": 0,
        "mapping_conflicts": 0,
    }
    if not entities:
        return result

    scheme = ensure_scheme(ks)
    if not scheme:
        return result
    result["scheme_iri"] = scheme["iri"]
    graph_iri = skos.graph_iri_for(ks)
    vocabulary = skos.build_view(graph_iri)
    ontology_iris = {entity["iri"] for entity in entities}
    abox_iri = f"{ks.graph_iri.rstrip('/')}/abox"
    abox_iris = {
        subject.value
        for subject, _, _ in store.read_triples(abox_iri)
        if isinstance(subject, NamedNode)
    }
    valid_mapping_iris = ontology_iris | abox_iris

    # A removed ontology entity should not leave an apparently valid local mapping behind. Keep
    # the controlled term, but make it explicitly unmapped so a human can remap or deprecate it.
    for concept in list(vocabulary["concepts"]):
        mapped = concept.get("mapped_entity_iri")
        if mapped and mapped not in valid_mapping_iris:
            payload = _concept_payload(concept)
            payload["mapped_entity_iri"] = None
            skos.update_concept(graph_iri, concept["iri"], payload)
            result["stale_mappings_removed"] += 1

    vocabulary = skos.build_view(graph_iri)
    concept_by_mapping = {
        concept["mapped_entity_iri"]: concept
        for concept in vocabulary["concepts"] if concept.get("mapped_entity_iri")
    }
    concept_by_iri = {concept["iri"]: concept for concept in vocabulary["concepts"]}
    label_owner: dict[tuple[str, str], dict] = {}
    for concept in vocabulary["concepts"]:
        if concept["scheme_iri"] != scheme["iri"]:
            continue
        for label in concept["pref_labels"] + concept["alt_labels"] + concept["hidden_labels"]:
            label_owner[(skos.normalize_label(label["value"]), label["language"].casefold())] = concept

    for entity in entities:
        label = str(entity.get("label") or entity.get("local") or "").strip()
        if not label:
            continue
        language = _language(label)
        key = (skos.normalize_label(label), language.casefold())
        concept = concept_by_mapping.get(entity["iri"])

        if concept is None:
            exact = label_owner.get(key)
            if exact and not exact.get("mapped_entity_iri"):
                payload = _concept_payload(exact)
                payload["mapped_entity_iri"] = entity["iri"]
                concept = skos.update_concept(graph_iri, exact["iri"], payload)
                result["terms_mapped"] += 1
            elif exact:
                result["mapping_conflicts"] += 1
                continue
            else:
                digest = hashlib.sha256(entity["iri"].encode("utf-8")).hexdigest()[:16]
                try:
                    concept = skos.create_concept(graph_iri, {
                        "iri": f"{graph_iri}#entity-{digest}",
                        "scheme_iri": scheme["iri"],
                        "pref_labels": [{"value": label, "language": language}],
                        "alt_labels": [],
                        "hidden_labels": [],
                        "description": str(entity.get("comment") or "").strip(),
                        "notation": "",
                        "broader": [],
                        "related": [],
                        "mapped_entity_iri": entity["iri"],
                        "status": "active",
                        "origin": "extraction",
                    })
                except skos.VocabularyValidationError:
                    result["mapping_conflicts"] += 1
                    continue
                result["terms_added"] += 1
                result["terms_mapped"] += 1
                label_owner[key] = concept
            concept_by_mapping[entity["iri"]] = concept
            concept_by_iri[concept["iri"]] = concept

        existing_keys = {
            (skos.normalize_label(item["value"]), item["language"].casefold())
            for item in concept["pref_labels"] + concept["alt_labels"] + concept["hidden_labels"]
        }
        if key not in existing_keys and key not in label_owner:
            payload = _concept_payload(concept)
            payload["alt_labels"] = payload["alt_labels"] + [{"value": label, "language": language}]
            concept = skos.update_concept(graph_iri, concept["iri"], payload)
            concept_by_mapping[entity["iri"]] = concept
            concept_by_iri[concept["iri"]] = concept
            label_owner[key] = concept
            result["aliases_added"] += 1

    # OWL subclass relations are deterministic enough to seed the lexical hierarchy. Relations
    # spanning different schemes are left alone; cycles are rejected by the SKOS validator.
    for entity in ontology["classes"]:
        concept = concept_by_mapping.get(entity["iri"])
        if not concept:
            continue
        parent_concepts = [concept_by_mapping.get(parent) for parent in entity.get("superclasses", [])]
        additions = [
            parent["iri"] for parent in parent_concepts
            if parent and parent["scheme_iri"] == concept["scheme_iri"]
            and parent["iri"] != concept["iri"] and parent["iri"] not in concept["broader"]
        ]
        if not additions:
            continue
        payload = _concept_payload(concept)
        payload["broader"] = payload["broader"] + additions
        try:
            updated = skos.update_concept(graph_iri, concept["iri"], payload)
        except skos.VocabularyValidationError:
            continue
        concept_by_mapping[entity["iri"]] = updated
        concept_by_iri[updated["iri"]] = updated
        result["broader_added"] += len(additions)

    return result
