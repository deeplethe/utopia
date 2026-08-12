"""LLM-assisted controlled-terminology proposals.

The agent is advisory only: every suggestion is persisted as a pending ``TermProposal`` and
must be accepted or rejected through Review before the active SKOS graph changes.
"""
from __future__ import annotations

import hashlib
import json
import logging
import re
import unicodedata

from sqlmodel import Session, select

from app import prompt_config
from app.config import settings
from app.db.models import Document, KnowledgeSystem, TermProposal
from app.llm import openrouter
from app.ontology import schema, skos

logger = logging.getLogger(__name__)

_SYSTEM = """You are a controlled-terminology steward. Read source excerpts, the current SKOS
vocabulary, the ontology, and past human decisions. Propose precise terminology governance
changes, but do not invent unsupported terms.

Return ONLY one JSON object: {"proposals": [...]}.
Each proposal must use exactly one action:

1. Create a new controlled concept:
{"action":"create","preferred_label":"...","language":"zh-CN","alternate_labels":["..."],
 "hidden_labels":[],"description":"...","broader_concept_iri":null,
 "mapped_entity_iri":null,"confidence":0.0,"reason":"...","source_chunk_ids":[1]}

2. Add genuine synonyms to an existing concept:
{"action":"add_alias","target_concept_iri":"...","alternate_labels":["..."],
 "language":"zh-CN","confidence":0.0,"reason":"...","source_chunk_ids":[1]}

3. Add a broader relation or ontology mapping to an existing concept:
{"action":"update","target_concept_iri":"...","broader_concept_iri":null,
 "mapped_entity_iri":null,"confidence":0.0,"reason":"...","source_chunk_ids":[1]}

Rules:
- Distinguish synonyms from subtypes. A subtype such as "permanent-magnet motor" is not an alias
  of "motor"; create a narrower concept instead and set its broader concept.
- Every proposed preferred or alternate label MUST occur verbatim in at least one cited source
  chunk. Do not synthesize contextual names such as "Industrial Pump" when only "Pump" occurs.
- An alternate label must be a substitutable name for the same concept, not a definition,
  description, metaphor, sentence fragment, or related phrase.
- Add a broader concept only when "Every target concept is necessarily a broader concept" is
  true. Created-by, managed-by, used-by, contains, and part-of relations are NOT broader links.
- One mapped ontology entity has one controlled concept. For a spelling/spacing variant of an
  already mapped entity, propose add_alias on its existing concept instead of create.
- Reuse only IRIs explicitly supplied below. Never fabricate a target, broader, or mapped IRI.
- Do not repeat an existing preferred/alternative/hidden label.
- Prefer the source language. Keep explanations concise and evidence-based.
- Skip uncertain noise rather than proposing it. Empty proposals are valid.
- Human decisions below are authoritative; do not repeat rejected proposals.
"""

prompt_config.register(
    key="terminology.steward",
    category="governance",
    title="Terminology steward",
    description="Suggest controlled terms, aliases, broader links, and ontology mappings for review.",
    default=_SYSTEM,
    order=30,
)


def _compact_terms(view: dict, scheme_iri: str) -> str:
    rows = []
    for concept in view["concepts"]:
        if concept["scheme_iri"] != scheme_iri:
            continue
        aliases = ", ".join(label["value"] for label in concept["alt_labels"]) or "-"
        broader = ", ".join(concept["broader_labels"]) or "-"
        rows.append(
            f'- {concept["display_label"]} | iri={concept["iri"]} | aliases={aliases} | '
            f'broader={broader} | mapsTo={concept.get("mapped_entity_iri") or "-"}'
        )
    return "\n".join(rows[:300]) or "(none)"


def _compact_ontology(view: dict) -> tuple[str, set[str]]:
    rows: list[str] = []
    iris: set[str] = set()
    for kind, entities in (
        ("class", view["classes"]),
        ("object_property", view["object_properties"]),
        ("data_property", view["data_properties"]),
    ):
        for entity in entities:
            iris.add(entity["iri"])
            rows.append(f'- [{kind}] {entity["label"]} | iri={entity["iri"]}')
    return "\n".join(rows[:500]) or "(none)", iris


def _decision_memory(session: Session, ks_id: int) -> str:
    rows = session.exec(
        select(TermProposal).where(
            TermProposal.knowledge_system_id == ks_id,
            TermProposal.status != "pending",
        ).order_by(TermProposal.id.desc()).limit(40)
    ).all()
    return "\n".join(
        f'- {row.status}: {row.action} "{row.term}" target={row.target_iri or "-"}'
        + (f' note={row.resolution_note}' if row.resolution_note else "")
        for row in rows
    ) or "(none)"


def _as_strings(value) -> list[str]:
    if isinstance(value, str):
        value = [value]
    if not isinstance(value, list):
        return []
    out: list[str] = []
    seen: set[str] = set()
    for item in value:
        text = str(item).strip()
        key = skos.normalize_label(text)
        if text and key not in seen:
            seen.add(key)
            out.append(text)
    return out


def _confidence(value) -> float:
    try:
        return max(0.0, min(1.0, float(value)))
    except (TypeError, ValueError):
        return 0.0


def _signature(action: str, target_iri: str | None, payload: dict) -> str:
    raw = json.dumps(
        {"action": action, "target_iri": target_iri, "payload": payload},
        ensure_ascii=False, sort_keys=True, separators=(",", ":"),
    )
    return hashlib.sha256(raw.encode("utf-8")).hexdigest()


def _sanitize(
    raw: dict, *, scheme_iri: str, concept_by_iri: dict[str, dict], ontology_iris: set[str],
) -> tuple[str, str, str | None, dict] | None:
    action = str(raw.get("action", "")).strip()
    language = str(raw.get("language", "zh-CN")).strip() or "zh-CN"
    target_iri = str(raw.get("target_concept_iri", "")).strip() or None
    broader = str(raw.get("broader_concept_iri", "")).strip() or None
    mapped = str(raw.get("mapped_entity_iri", "")).strip() or None
    if broader not in concept_by_iri:
        broader = None
    if mapped not in ontology_iris:
        mapped = None

    existing_labels = {
        skos.normalize_label(label["value"])
        for concept in concept_by_iri.values()
        for label in concept["pref_labels"] + concept["alt_labels"] + concept["hidden_labels"]
    }
    concept_by_mapping = {
        concept["mapped_entity_iri"]: concept
        for concept in concept_by_iri.values() if concept.get("mapped_entity_iri")
    }

    if action == "create":
        preferred = str(raw.get("preferred_label", "")).strip()
        if not preferred or skos.normalize_label(preferred) in existing_labels:
            return None
        aliases = [value for value in _as_strings(raw.get("alternate_labels")) if skos.normalize_label(value) != skos.normalize_label(preferred)]
        hidden = [value for value in _as_strings(raw.get("hidden_labels")) if skos.normalize_label(value) != skos.normalize_label(preferred)]
        if mapped and mapped in concept_by_mapping:
            target = concept_by_mapping[mapped]
            candidates = [preferred, *aliases, *hidden]
            aliases = [value for value in candidates if skos.normalize_label(value) not in existing_labels]
            if not aliases:
                return None
            return (
                "add_alias", aliases[0], target["iri"],
                {"add_alt_labels": [{"value": value, "language": language} for value in aliases]},
            )
        payload = {
            "scheme_iri": scheme_iri,
            "pref_labels": [{"value": preferred, "language": language}],
            "alt_labels": [{"value": value, "language": language} for value in aliases],
            "hidden_labels": [{"value": value, "language": language} for value in hidden],
            "description": str(raw.get("description", "")).strip()[:1000],
            "notation": "",
            "broader": [broader] if broader else [],
            "related": [],
            "mapped_entity_iri": mapped,
            "status": "active",
            "origin": "agent",
        }
        return action, preferred, None, payload

    if action == "add_alias" and target_iri in concept_by_iri:
        aliases = [
            value for value in _as_strings(raw.get("alternate_labels"))
            if skos.normalize_label(value) not in existing_labels
        ]
        if not aliases:
            return None
        payload = {
            "add_alt_labels": [{"value": value, "language": language} for value in aliases],
        }
        return action, aliases[0], target_iri, payload

    if action == "update" and target_iri in concept_by_iri and (broader or mapped):
        payload = {"broader_iri": broader, "mapped_entity_iri": mapped}
        return action, concept_by_iri[target_iri]["display_label"], target_iri, payload
    return None


def _source_contains(source_texts: list[str], label: str) -> bool:
    needle = re.sub(r"\s+", " ", unicodedata.normalize("NFKC", label).casefold()).strip()
    return bool(needle) and any(
        needle in re.sub(r"\s+", " ", unicodedata.normalize("NFKC", text).casefold())
        for text in source_texts
    )


_DEFINITION_ALIAS_RE = re.compile(
    r"\b(?:around|because|consists?\s+of|contains?|described\s+as|made\s+from|"
    r"environment\s+for|that|used\s+(?:as|for|to)|which|who|whose)\b",
    re.IGNORECASE,
)


def _looks_like_definition_alias(label: str) -> bool:
    normalized = re.sub(r"\s+", " ", unicodedata.normalize("NFKC", label)).strip()
    return bool(_DEFINITION_ALIAS_RE.search(normalized)) or len(normalized.split()) > 8


def _filter_to_supported_labels(
    sanitized: tuple[str, str, str | None, dict], source_texts: list[str],
) -> tuple[str, str, str | None, dict] | None:
    action, term, target_iri, payload = sanitized
    if action == "create":
        preferred = payload["pref_labels"][0]["value"]
        if not _source_contains(source_texts, preferred):
            return None
        payload = dict(payload)
        payload["alt_labels"] = [
            item for item in payload["alt_labels"]
            if _source_contains(source_texts, item["value"])
            and not _looks_like_definition_alias(item["value"])
        ]
        payload["hidden_labels"] = [
            item for item in payload["hidden_labels"] if _source_contains(source_texts, item["value"])
        ]
        return action, preferred, target_iri, payload
    if action == "add_alias":
        payload = dict(payload)
        payload["add_alt_labels"] = [
            item for item in payload["add_alt_labels"]
            if _source_contains(source_texts, item["value"])
            and not _looks_like_definition_alias(item["value"])
        ]
        if not payload["add_alt_labels"]:
            return None
        return action, payload["add_alt_labels"][0]["value"], target_iri, payload
    return sanitized


def suggest(
    session: Session,
    ks: KnowledgeSystem,
    scheme_iri: str,
    chunks: list[tuple[object, Document]],
    *,
    model: str | None = None,
    job_id: int | None = None,
    proposed_by: str = "terminology-agent",
) -> list[TermProposal]:
    vocabulary = skos.build_view(skos.graph_iri_for(ks))
    if not any(scheme["iri"] == scheme_iri for scheme in vocabulary["schemes"]):
        raise skos.VocabularyValidationError("Vocabulary scheme not found")
    concept_by_iri = {concept["iri"]: concept for concept in vocabulary["concepts"]}
    ontology = schema.build_view(ks.graph_iri)
    ontology_text, ontology_iris = _compact_ontology(ontology)

    remaining = settings.terminology_suggestion_max_chars
    source_blocks: list[str] = []
    chunk_lookup: dict[int, tuple[object, Document]] = {}
    for chunk, document in chunks[: settings.terminology_suggestion_max_chunks]:
        if remaining <= 0:
            break
        text = str(chunk.text).strip()
        excerpt = text[:remaining]
        remaining -= len(excerpt)
        chunk_lookup[chunk.id] = (chunk, document)
        source_blocks.append(f"[chunk:{chunk.id} | {document.original_filename}]\n{excerpt}")
    if not source_blocks:
        raise skos.VocabularyValidationError("No parsed document chunks are available")

    prompt = (
        "CURRENT CONTROLLED TERMS:\n" + _compact_terms(vocabulary, scheme_iri)
        + "\n\nONTOLOGY ENTITIES:\n" + ontology_text
        + "\n\nPAST HUMAN DECISIONS:\n" + _decision_memory(session, ks.id)
        + "\n\nSOURCE EXCERPTS:\n" + "\n\n".join(source_blocks)
        + "\n\nPropose controlled-terminology changes."
    )
    reply = openrouter.chat_sync(
        [{"role": "system", "content": prompt_config.get("terminology.steward")}, {"role": "user", "content": prompt}],
        model=model,
        temperature=0.1,
    )
    parsed = openrouter.extract_json(reply)
    proposals = parsed.get("proposals", []) if isinstance(parsed, dict) else []
    if not isinstance(proposals, list):
        return []

    created: list[TermProposal] = []
    for raw in proposals[:50]:
        if not isinstance(raw, dict):
            continue
        source_ids = []
        for value in raw.get("source_chunk_ids", []) if isinstance(raw.get("source_chunk_ids"), list) else []:
            try:
                chunk_id = int(value)
            except (TypeError, ValueError):
                continue
            if chunk_id in chunk_lookup and chunk_id not in source_ids:
                source_ids.append(chunk_id)
        if not source_ids:
            continue
        sanitized = _sanitize(
            raw, scheme_iri=scheme_iri, concept_by_iri=concept_by_iri, ontology_iris=ontology_iris,
        )
        if sanitized:
            sanitized = _filter_to_supported_labels(
                sanitized, [str(chunk_lookup[chunk_id][0].text) for chunk_id in source_ids],
            )
        if not sanitized:
            continue
        action, term, target_iri, payload = sanitized
        signature = _signature(action, target_iri, payload)
        exists = session.exec(
            select(TermProposal).where(
                TermProposal.knowledge_system_id == ks.id,
                TermProposal.signature == signature,
            )
        ).first()
        if exists:
            continue
        evidence = [
            {
                "chunk_id": chunk_id,
                "document_id": chunk_lookup[chunk_id][1].id,
                "document": chunk_lookup[chunk_id][1].original_filename,
                "snippet": str(chunk_lookup[chunk_id][0].text).strip()[:600],
            }
            for chunk_id in source_ids
        ]
        row = TermProposal(
            knowledge_system_id=ks.id,
            signature=signature,
            action=action,
            term=term,
            target_iri=target_iri,
            payload=payload,
            confidence=_confidence(raw.get("confidence")),
            reason=str(raw.get("reason", "")).strip()[:500] or None,
            evidence=evidence,
            source_chunk_ids=source_ids,
            extraction_job_id=job_id,
            proposed_by=proposed_by,
        )
        session.add(row)
        created.append(row)
    session.commit()
    for row in created:
        session.refresh(row)
    logger.info("terminology agent created %d proposal(s) for KS %s", len(created), ks.id)
    return created
