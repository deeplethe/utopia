"""LLM-driven ABox (instance) extraction, guided by the KS's existing TBox.

Each chunk is sent to a cheap DeepSeek model with the ontology's class/property vocabulary;
the model returns specific individuals typed by those classes, with attribute/relationship
assertions using those properties. Every extracted mention is then run through entity
resolution (``resolution.resolve_mention``) so the same real-world entity mentioned across
chunks/documents collapses to one individual, ambiguous cases go to the manual queue, and
decisions accumulate as a learned lookup.

LLM mention extraction overlaps with bounded concurrency. Resolution and graph writes still run
SEQUENTIALLY because each chunk must see the individuals created by earlier chunks.
"""
from __future__ import annotations

import asyncio
import contextlib
import json
import logging
import math
import re
from collections.abc import Awaitable, Callable

from sqlalchemy.exc import OperationalError
from sqlmodel import Session, select

from app import model_config, prompt_config
from app.config import settings
from app.db.database import engine
from app.db.models import Chunk, EntityResolution
from app.llm import openrouter
from app.ontology import abox, abox_provenance, entity_roles, resolution, role_evidence, schema, skos

logger = logging.getLogger(__name__)


def _as_dicts(x) -> list[dict]:
    """Coerce an LLM-provided 'attributes'/'relations' field to a list of dicts. A cheap model
    may emit a single object, a list of strings, or null; anything that isn't a dict is dropped
    so a malformed shape can't crash the whole chunk (losing already-written individuals)."""
    if isinstance(x, dict):
        x = [x]
    if not isinstance(x, list):
        return []
    return [a for a in x if isinstance(a, dict)]


def _evidence(m: dict) -> str:
    """A short 'facts' string for a mention, given to the resolution agent to compare entities."""
    parts = []
    source_evidence = str(m.get("evidence") or "").strip()
    if source_evidence:
        parts.append(f"source={source_evidence}")
    for a in _as_dicts(m.get("attributes")):
        p, v = str(a.get("property", "")).strip(), a.get("value")
        if p and v not in (None, ""):
            parts.append(f"{p}={v}")
    for r in _as_dicts(m.get("relations")):
        p, t = str(r.get("property", "")).strip(), str(r.get("target", "")).strip()
        if p and t:
            parts.append(f"{p}→{t}")
    return "; ".join(parts)


_IDENTITY_EVIDENCE_PROPERTIES = {
    "address",
    "email",
    "image",
    "ip",
    "ip address",
    "serial number",
    "uid",
    "uri",
    "url",
    "uuid",
    "version",
}


def _evidence_values(evidence: str) -> dict[str, set[str]]:
    values: dict[str, set[str]] = {}
    for part in evidence.split("; "):
        prop, separator, value = part.partition("=")
        normalized_prop = skos.normalize_label(prop)
        normalized_value = skos.normalize_label(value)
        if separator and normalized_prop in _IDENTITY_EVIDENCE_PROPERTIES and normalized_value:
            values.setdefault(normalized_prop, set()).add(normalized_value)
    return values


def _detail_identity_values(details: dict | None) -> dict[str, set[str]]:
    values: dict[str, set[str]] = {}
    for attribute in (details or {}).get("attributes", []):
        prop = skos.normalize_label(str(attribute.get("property", "")))
        value = skos.normalize_label(str(attribute.get("value", "")))
        if prop in _IDENTITY_EVIDENCE_PROPERTIES and value:
            values.setdefault(prop, set()).add(value)
    return values


def _individual_document_ids(
    session: Session, ks_id: int, individual_iri: str,
) -> set[int]:
    document_ids: set[int] = set()
    rows = session.exec(
        select(EntityResolution).where(
            EntityResolution.knowledge_system_id == ks_id,
            EntityResolution.individual_iri == individual_iri,
        )
    ).all()
    for row in rows:
        chunk = session.get(Chunk, row.source_chunk_id) if row.source_chunk_id is not None else None
        if chunk:
            document_ids.add(chunk.document_id)
    return document_ids


def _cross_document_identity_conflict(
    session: Session,
    *,
    ks_id: int,
    chunk_id: int,
    candidate_iri: str,
    evidence: str,
    candidate_details: dict | None,
) -> tuple[bool, list[str]]:
    chunk = session.get(Chunk, chunk_id)
    if not chunk:
        return False, []
    candidate_documents = _individual_document_ids(session, ks_id, candidate_iri)
    if not candidate_documents or chunk.document_id in candidate_documents:
        return False, []

    incoming = _evidence_values(evidence)
    existing = _detail_identity_values(candidate_details)
    conflicts = sorted(
        prop
        for prop in incoming.keys() & existing.keys()
        if incoming[prop].isdisjoint(existing[prop])
    )
    return bool(conflicts), conflicts


def _pending_payload(m: dict, prop_index: dict[str, tuple[str, str]]) -> dict | None:
    """Resolve a mention's attributes/relations to property IRIs and stash them, so if the
    mention lands in the manual queue its facts aren't lost — they're replayed onto the
    individual when a human resolves it (relations keep the target *label*, resolved at replay)."""
    attrs, rels = [], []
    for a in _as_dicts(m.get("attributes")):
        pi = prop_index.get(skos.normalize_label(str(a.get("property", ""))))
        val = a.get("value")
        if pi and pi[1] == "data" and val not in (None, "") and str(val).strip():
            attrs.append({"prop": pi[0], "value": str(val).strip()})
    for r in _as_dicts(m.get("relations")):
        pi = prop_index.get(skos.normalize_label(str(r.get("property", ""))))
        tgt = str(r.get("target", "")).strip()
        if pi and pi[1] == "object" and tgt:
            rels.append({"prop": pi[0], "target_label": tgt})
    payload: dict = {}
    if attrs:
        payload["pending_attributes"] = attrs
    if rels:
        payload["pending_relations"] = rels
    return payload or None


def _compact_details(det: dict | None) -> dict | None:
    """Trim get_individual() output to what the resolution agent needs (keeps tokens down)."""
    if not det:
        return None
    return {
        "label": det["label"],
        "types": [t["label"] for t in det["types"]],
        "attributes": [{"property": a["prop_label"], "value": a["value"]} for a in det["data_assertions"]],
        "relations": [{"property": r["prop_label"], "target": r["target_label"]} for r in det["object_assertions"]],
    }

ProgressCb = Callable[[dict], Awaitable[None]] | None

_SYSTEM_PROMPT = """You propose ABox individuals — concrete entities or controlled entries with
stable identity — from text, typed by an EXISTING ontology's classes. A separate critic will
verify every proposal, so preserve exact evidence and do not repair or invent names.

Return ONLY a single JSON object:
{
  "individuals": [
    {
      "label": "<exact name/identifier as it appears in the text>",
      "class": "<exactly one of the EXISTING class labels listed below>",
      "evidence": "<short exact source span establishing identity and type>",
      "identity_basis": "explicit_name|identifier|structured_object|controlled_entry|other",
      "attributes": [{"property": "<existing data-property label>", "value": "<literal value>"}],
      "relations": [{"property": "<existing object-property label>", "target": "<label of another individual in this list>"}]
    }
  ]
}

Rules:
- Extract concrete individuals, not reusable concepts. "Pump" as a kind is not an individual;
  "Pump P-101" is when the source identifies that particular pump.
- Copy an exact source span into `evidence`. The label itself must occur in the source; never
  translate it, append a type suffix, or synthesize a display name.
- A bare number, date, address, version, enum, measurement, status, option, or scalar field value
  is a literal unless the source explicitly uses it as an entity's name or identifier.
- In structured data, ordinary scalar values remain literals. A mapping/object with an explicit
  identity field can be an individual when the text also supports one of the existing classes.
- A controlled entry may be an individual when the source treats the exact value as a stable
  member of a reusable category; do not turn that value into a TBox class.
- A class heading, abbreviation, plural, or generic concept mention is not an individual merely
  because it appears in quotes, code, a link, a list, or an example.
- Quotation marks, inline code, links, list items, and examples do not by themselves establish
  identity.
- Placeholder values such as "Untitled", "Unspecified", "Unknown", or "N/A" do not identify
  an entity. Drop them rather than merging unrelated records under one placeholder individual.
- Type each individual with the single best-matching EXISTING class label. If none fits, omit it.
- Do NOT extract vague descriptors, spatial phrases, or activity/task descriptions as
  individuals. Only extract things with a real, distinct identity.
- Use ONLY the existing property labels below for attributes/relations; DROP any assertion
  whose property is not in the ontology.
- For a data property whose type is numeric (integer/decimal), put ONLY the number in "value"
  (e.g. "37", not "37 kW"; "2000", not "2000 tons") — the unit is implied by the property.
  Keep the unit only when the property's type is a string.
- A relation's "target" must be the label of another individual you list.
- Keep labels and values in the SAME language as the source text. Do not translate.
- If the text contains no specific instances, return {"individuals": []}.
- Output must be valid JSON with no surrounding prose."""

prompt_config.register(
    key="abox.extract",
    category="extraction",
    title="ABox extraction",
    description="Extract concrete individuals and assertions using the knowledge system's TBox.",
    default=_SYSTEM_PROMPT,
    order=30,
)

_ROLE_CRITIC_PROMPT = """You are an independent ABox role critic. The first extractor is
untrusted and may convert a class, literal, enum, option, field value, or example token into an
individual, or may assign a malformed/composite class. Judge each candidate only from the
supplied source and ALLOWED CLASSES.

Choose exactly one role:
- individual: the source establishes a particular entity or stable controlled entry with identity;
- type: a reusable class/concept mention rather than a member;
- literal: a scalar, measurement, status, option, identifier value not used as an entity name,
  or descriptive text;
- uncertain: identity or type cannot be established from the source.

A number, address, code, or scalar may be an individual only when the source explicitly uses it
as a name/identifier of an entity. A value inside structured data is not automatically an
individual. Reject model-invented or rewritten labels and reject a candidate whose proposed class
does not match the entity established by the evidence.

Ontology and schema documentation often writes class-level examples as compact relation patterns,
such as `Pump hasPart Valve` or `Room hasPoint Sensor`. A predicate-bearing sentence does not by
itself establish concrete identities. When a candidate label is identical to an allowed class
label, treat it as a TYPE unless the source separately gives that entity an explicit name,
identifier, URI used as an instance, or an unambiguous "instance named ..." declaration.

For every kept individual, select exactly one class copied verbatim from ALLOWED CLASSES. You may
correct the untrusted candidate class, but never invent, combine, or rewrite a class label. Use
selected_class=null when no allowed class fits. Choose the most specific class directly supported
by the source. An explicit type/kind/class/category declaration outranks a broader compatible
class.

Return ONLY:
{"decisions":[{"label":"<exact candidate label>",
"candidate_class":"<exact untrusted candidate class>",
"selected_class":"<one exact allowed class or null>",
"role":"individual|type|literal|uncertain","keep":true,"confidence":0.0,
"evidence":"<short exact source span>","reason":"<short reason>"}]}

Do not add or rename candidate labels. Evidence must be copied from the source text."""

prompt_config.register(
    key="abox.boundary.critic",
    category="review",
    title="ABox boundary critic",
    description="Reject class, literal, and malformed candidates before they enter the instance graph.",
    default=_ROLE_CRITIC_PROMPT,
    order=20,
)

_SELF_TYPED_ADJUDICATOR_PROMPT = """You are the final identity-boundary adjudicator for ABox
candidates whose exact surface label is also their selected class label. This shape is ambiguous:
schema documentation frequently uses class labels as variables in compact relation examples.

Keep a candidate as an INDIVIDUAL only when the source independently establishes one particular
identity through an explicit name, identifier, instance URI, controlled-entry declaration, or
wording such as "the instance named X". A relation pattern like `Pump hasPart Valve`, a class
heading, a glossary definition, a diagram legend, or a generic example does NOT name instances,
even though the labels participate in predicates. Self-named entities are possible, but require
that independent identity evidence.

Return ONLY:
{"decisions":[{"label":"<exact candidate label>",
"candidate_class":"<exact candidate class>","selected_class":"<same allowed class or null>",
"role":"individual|type|literal|uncertain","keep":true,"confidence":0.0,
"evidence":"<short exact source span>","reason":"<short identity reason>"}]}

Return one decision per candidate. Do not add or rename labels. Use role=type and keep=false for
schema-level relation examples. Evidence must be copied exactly from the source."""

prompt_config.register(
    key="abox.boundary.self_typed_adjudicator",
    category="review",
    title="Self-typed ABox adjudicator",
    description="Distinguish true self-named individuals from class-level schema examples.",
    default=_SELF_TYPED_ADJUDICATOR_PROMPT,
    order=25,
)

_MAX_CLASSES = 400
_MAX_PROPS = 200
_NON_IDENTIFYING_LABELS = {
    skos.normalize_label(label)
    for label in (
        "untitled", "unnamed", "no title",
        "unspecified", "not specified", "unknown", "n/a", "none", "null",
        "无标题", "未命名", "未指定", "未知", "不详", "无",
    )
}
_TEMPLATE_LABEL_RE = re.compile(
    r"^(?:\$\{?[A-Z_][A-Z0-9_]*\}?|%[A-Z_][A-Z0-9_]*%|<[^<>]+>|\{\{[^{}]+\}\})$"
)


def _is_non_identifying_label(label: str) -> bool:
    stripped = label.strip()
    return (
        skos.normalize_label(stripped) in _NON_IDENTIFYING_LABELS
        or _TEMPLATE_LABEL_RE.fullmatch(stripped) is not None
    )


def _class_hierarchy(view: dict) -> dict[str, set[str]]:
    """class iri -> {itself + all ancestors + all descendants}, so entity resolution can look
    for candidates across the hierarchy (a person typed 'Worker' vs 'Senior Worker')."""
    direct_super = {c["iri"]: set(c.get("superclasses", [])) for c in view["classes"]}
    direct_sub: dict[str, set[str]] = {}
    for iri, sups in direct_super.items():
        for s in sups:
            direct_sub.setdefault(s, set()).add(iri)

    def closure(start: str, adj: dict[str, set[str]]) -> set[str]:
        seen, stack = set(), [start]
        while stack:
            for nxt in adj.get(stack.pop(), ()):
                if nxt not in seen:
                    seen.add(nxt)
                    stack.append(nxt)
        return seen

    return {iri: {iri} | closure(iri, direct_super) | closure(iri, direct_sub) for iri in direct_super}


def _class_ancestors(view: dict) -> dict[str, set[str]]:
    direct_super = {c["iri"]: set(c.get("superclasses", [])) for c in view["classes"]}
    result: dict[str, set[str]] = {}
    for class_iri in direct_super:
        seen: set[str] = {class_iri}
        stack = [class_iri]
        while stack:
            for parent in direct_super.get(stack.pop(), ()):
                if parent not in seen:
                    seen.add(parent)
                    stack.append(parent)
        result[class_iri] = seen
    return result


def _class_allowed(
    class_iri: str, allowed_classes: list[str], ancestors: dict[str, set[str]],
) -> bool:
    return not allowed_classes or bool(ancestors.get(class_iri, {class_iri}) & set(allowed_classes))


def _indexes(
    view: dict, aliases: dict[str, str] | None = None,
) -> tuple[dict[str, str], dict[str, tuple[str, str]]]:
    """Normalized class/property labels, including approved controlled-term aliases."""
    class_index = {skos.normalize_label(c["label"]): c["iri"] for c in view["classes"]}
    prop_index: dict[str, tuple[str, str]] = {}
    for p in view["object_properties"]:
        prop_index[skos.normalize_label(p["label"])] = (p["iri"], "object")
    for p in view["data_properties"]:
        prop_index[skos.normalize_label(p["label"])] = (p["iri"], "data")
    class_iris = {c["iri"] for c in view["classes"]}
    object_iris = {p["iri"] for p in view["object_properties"]}
    data_iris = {p["iri"] for p in view["data_properties"]}
    for label, iri in (aliases or {}).items():
        if iri in class_iris:
            class_index[label] = iri
        elif iri in object_iris:
            prop_index[label] = (iri, "object")
        elif iri in data_iris:
            prop_index[label] = (iri, "data")
    return class_index, prop_index


def _format_tbox(view: dict) -> str:
    classes = [c["label"] for c in view["classes"]][:_MAX_CLASSES]
    lines = [
        "EXISTING CLASSES (choose exactly one exact string from this JSON array):",
        json.dumps(classes, ensure_ascii=False) if classes else "[]",
    ]
    obj = view["object_properties"][:_MAX_PROPS]
    if obj:
        lines.append("\nOBJECT PROPERTIES (label: domain → range) — use for relations:")
        lines += [f"- {p['label']}: {p.get('domain_label') or '?'} → {p.get('range_label') or '?'}" for p in obj]
    data = view["data_properties"][:_MAX_PROPS]
    if data:
        lines.append("\nDATA PROPERTIES (label: type) — use for attributes:")
        lines += [f"- {p['label']}: {p.get('range_label') or 'string'}" for p in data]
    return "\n".join(lines)


async def _extract_one(text: str, model: str | None, tbox_ctx: str) -> list[dict]:
    user = f"{tbox_ctx}\n\nText:\n\"\"\"\n{text}\n\"\"\"\n\nReturn the JSON object of individuals."
    reply = await openrouter.chat(
        [{"role": "system", "content": prompt_config.get("abox.extract")}, {"role": "user", "content": user}],
        model=model,
    )
    data = openrouter.extract_json(reply)
    if not isinstance(data, dict):
        raise openrouter.LLMError("LLM did not return a JSON object")
    inds = data.get("individuals")
    return inds if isinstance(inds, list) else []


def _role_confidence(value: object) -> float:
    try:
        confidence = float(value or 0.0)
    except (TypeError, ValueError):
        return 0.0
    return max(0.0, min(1.0, confidence)) if math.isfinite(confidence) else 0.0


def _mention_key(mention: dict) -> tuple[str, str]:
    return (
        skos.normalize_label(str(mention.get("label", ""))),
        skos.normalize_label(str(mention.get("class", ""))),
    )


def _decision_key(decision: dict) -> tuple[str, str]:
    return (
        skos.normalize_label(str(decision.get("label", ""))),
        skos.normalize_label(str(decision.get("candidate_class") or decision.get("class") or "")),
    )


def _is_self_typed_mention(
    mention: dict, allowed_classes: list[str] | None = None,
) -> bool:
    """Whether a proposed identity reuses any known class label.

    The selected class alone is insufficient: a schema example such as ``AHU hasPart Fan`` may be
    returned as an ``AHU`` individual typed as ``Equipment``.  It still needs the stricter
    class-name adjudicator because its surface is itself a known TBox class.
    """
    label = role_evidence.normalize(str(mention.get("label", "")))
    class_label = role_evidence.normalize(str(mention.get("class", "")))
    allowed = {
        role_evidence.normalize(class_name)
        for class_name in (allowed_classes or [])
        if class_name.strip()
    }
    return bool(label and (label == class_label or label in allowed))


def _apply_abox_role_decisions(
    text: str, mentions: list[dict], payload: dict, allowed_classes: list[str] | None = None,
) -> tuple[list[dict], int]:
    """Apply independent role decisions with deterministic source-grounding checks."""
    allowed_index = {
        skos.normalize_label(class_label): class_label
        for class_label in (allowed_classes or [])
        if class_label.strip()
    }
    structured_roles = role_evidence.structured_value_roles(text)
    explicit_structured_classes = {
        allowed_index[normalized]
        for normalized, roles in structured_roles.items()
        if role_evidence.ROLE_TYPE in roles and normalized in allowed_index
    }
    decisions: dict[tuple[str, str], dict] = {}
    raw_decisions = payload.get("decisions", []) if isinstance(payload, dict) else []
    for decision in raw_decisions if isinstance(raw_decisions, list) else []:
        if isinstance(decision, dict) and all(_decision_key(decision)):
            decisions[_decision_key(decision)] = decision

    accepted: list[dict] = []
    rejected = 0
    for mention in mentions:
        if not isinstance(mention, dict):
            rejected += 1
            continue
        decision = decisions.get(_mention_key(mention))
        if not decision:
            rejected += 1
            continue
        evidence = decision.get("evidence")
        grounded = (
            role_evidence.surface_is_grounded(text, mention.get("label"))
            and role_evidence.evidence_is_grounded(text, evidence)
            and role_evidence.surface_is_grounded(str(evidence or ""), mention.get("label"))
        )
        role = str(decision.get("role", "")).strip().casefold()
        confidence = _role_confidence(decision.get("confidence"))
        selected_raw = str(
            decision.get("selected_class")
            or decision.get("class")
            or mention.get("class")
            or ""
        ).strip()
        selected_class = allowed_index.get(skos.normalize_label(selected_raw)) \
            if allowed_index else selected_raw
        mention_roles = structured_roles.get(role_evidence.normalize(str(mention.get("label") or "")), set())
        if (
            len(explicit_structured_classes) == 1
            and role_evidence.ROLE_LITERAL in mention_roles
            and role_evidence.ROLE_TYPE not in mention_roles
        ):
            selected_class = next(iter(explicit_structured_classes))
        class_valid = bool(selected_class)
        verified = bool(
            grounded
            and class_valid
            and decision.get("keep") is True
            and role == role_evidence.ROLE_INDIVIDUAL
            and confidence >= settings.role_auto_accept_floor
        )
        review = bool(
            grounded
            and class_valid
            and decision.get("keep") is True
            and confidence >= settings.role_review_floor
            and role in {role_evidence.ROLE_INDIVIDUAL, role_evidence.ROLE_UNCERTAIN}
        )
        if not verified and not review:
            rejected += 1
            continue
        cleaned = dict(mention)
        cleaned.pop("_role_verified", None)
        cleaned.pop("_force_review", None)
        cleaned.pop("_role_confidence", None)
        cleaned["class"] = selected_class
        cleaned["evidence"] = str(evidence or "").strip()
        cleaned["attributes"] = _as_dicts(cleaned.get("attributes"))
        cleaned["relations"] = _as_dicts(cleaned.get("relations"))
        if verified:
            cleaned["_role_verified"] = True
        else:
            cleaned["_force_review"] = str(
                decision.get("reason") or "individual role requires human confirmation"
            )
            cleaned["_role_confidence"] = confidence
        accepted.append(cleaned)
    return accepted, rejected


async def _adjudicate_self_typed_candidates(
    text: str,
    mentions: list[dict],
    model: str | None,
    allowed_classes: list[str] | None = None,
) -> tuple[list[dict], int]:
    """Recheck the ambiguous ``surface == class`` shape against explicit identity evidence."""
    suspicious = [
        mention for mention in mentions
        if _is_self_typed_mention(mention, allowed_classes)
    ]
    if not suspicious:
        return mentions, 0
    critic_input = [
        {
            "label": str(mention.get("label") or ""),
            "candidate_class": str(mention.get("class") or ""),
            "first_evidence": str(mention.get("evidence") or ""),
            "attributes": _as_dicts(mention.get("attributes")),
            "relations": _as_dicts(mention.get("relations")),
        }
        for mention in suspicious
    ]
    user = (
        f"SOURCE TEXT:\n\"\"\"\n{text}\n\"\"\"\n\n"
        f"ALLOWED CLASSES:\n{json.dumps(allowed_classes or [], ensure_ascii=False)}\n\n"
        f"SELF-TYPED CANDIDATES:\n{json.dumps(critic_input, ensure_ascii=False)}"
    )
    reply = await openrouter.chat(
        [
            {
                "role": "system",
                "content": prompt_config.get("abox.boundary.self_typed_adjudicator"),
            },
            {"role": "user", "content": user},
        ],
        model=model,
    )
    payload = openrouter.extract_json(reply)
    if not isinstance(payload, dict):
        raise openrouter.LLMError("Self-typed ABox adjudicator did not return a JSON object")
    adjudicated, rejected = _apply_abox_role_decisions(
        text, suspicious, payload, allowed_classes,
    )
    accepted_by_key = {_mention_key(mention): mention for mention in adjudicated}
    merged = [
        accepted_by_key.get(_mention_key(mention), mention)
        for mention in mentions
        if (
            not _is_self_typed_mention(mention, allowed_classes)
            or _mention_key(mention) in accepted_by_key
        )
    ]
    return merged, rejected


async def _verify_abox_candidates(
    text: str, mentions: list[dict], model: str | None,
    allowed_classes: list[str] | None = None,
) -> tuple[list[dict], int]:
    candidates = [mention for mention in mentions if isinstance(mention, dict)]
    if not candidates:
        return [], 0
    critic_input = [
        {
            "label": str(mention.get("label") or ""),
            "candidate_class": str(mention.get("class") or ""),
            "extractor_evidence": str(mention.get("evidence") or ""),
            "identity_basis": str(mention.get("identity_basis") or ""),
            "attributes": _as_dicts(mention.get("attributes")),
            "relations": _as_dicts(mention.get("relations")),
        }
        for mention in candidates
    ]
    user = (
        f"SOURCE TEXT:\n\"\"\"\n{text}\n\"\"\"\n\n"
        f"ALLOWED CLASSES:\n{json.dumps(allowed_classes or [], ensure_ascii=False)}\n\n"
        f"UNTRUSTED CANDIDATES:\n{json.dumps(critic_input, ensure_ascii=False)}"
    )
    reply = await openrouter.chat(
        [
            {"role": "system", "content": prompt_config.get("abox.boundary.critic")},
            {"role": "user", "content": user},
        ],
        model=model,
    )
    payload = openrouter.extract_json(reply)
    if not isinstance(payload, dict):
        raise openrouter.LLMError("ABox role critic did not return a JSON object")
    return _apply_abox_role_decisions(text, candidates, payload, allowed_classes)


def _resolve_and_merge_chunk(
    ks_id: int, abox_iri: str, base_iri: str, chunk_id: int, job_id: int | None,
    actor_name: str,
    class_index: dict[str, str], prop_index: dict[str, tuple[str, str]],
    class_labels: dict[str, str], prop_labels: dict[str, str], hierarchy: dict[str, set[str]],
    ancestors: dict[str, set[str]], property_shapes: dict[str, dict[str, list[str]]],
    roles_by_class: dict[str, frozenset[str]],
    res_index: "abox.ResolutionIndex", model: str | None, source_text: str, mentions: list[dict],
    agentic_resolution: bool,
    *,
    db_session: Session | None = None,
    commit: bool = True,
) -> dict:
    """Resolve each mention to an individual, then apply its attribute/relation assertions.
    Runs in a worker thread (blocking embeddings + LLM + DB + graph writes); graph writes are
    captured by the job's active ``store.capture``. Idempotent: re-adding a triple is a no-op."""
    counts = {"created": 0, "matched": 0, "queued": 0, "assertions": 0, "rejected": 0}
    unknown: list[str] = []  # class labels the LLM used that aren't in the TBox
    # Subject lookup is keyed by (surface, class): the same surface under two different classes
    # in one chunk is two distinct individuals, so its assertions must not collide.
    local: dict[tuple[str, str], str] = {}  # (surface_lower, class_iri) -> individual iri
    by_label: dict[str, set[str]] = {}      # surface_lower -> resolved iris (relation-target lookup)

    # Extraction jobs pass their outer session with ``commit=False`` so every
    # resolution row and provenance update commits together with the graph diff,
    # document markers, job counters, and audit event.  Keeping the owned-session
    # path preserves compatibility for callers that use this helper directly.
    session_context = (
        Session(engine) if db_session is None else contextlib.nullcontext(db_session)
    )
    with session_context as session:
        # A multi-step agent decides ambiguous candidate matches (embeddings only retrieve).
        agent = None
        if agentic_resolution:
            def _details(iri: str):
                return _compact_details(abox.get_individual(abox_iri, iri, class_labels, prop_labels))

            def _agent(surface: str, class_label: str, evidence: str, candidates):
                decision = resolution.agentic_resolve(
                    session=session, ks_id=ks_id, surface=surface, class_label=class_label,
                    evidence=evidence, candidates=candidates, details_fn=_details, model=model,
                    max_steps=settings.resolution_max_steps,
                )
                candidate_iri = decision.get("iri") if decision.get("decision") == "match" else None
                if candidate_iri:
                    conflict, properties = _cross_document_identity_conflict(
                        session,
                        ks_id=ks_id,
                        chunk_id=chunk_id,
                        candidate_iri=candidate_iri,
                        evidence=evidence,
                        candidate_details=_details(candidate_iri),
                    )
                    if conflict:
                        return {
                            "decision": "new",
                            "iri": None,
                            "confidence": 1.0,
                            "reason": (
                                "same name occurs in another document with conflicting identity "
                                f"properties: {', '.join(properties)}"
                            ),
                        }
                return decision
            agent = _agent

        seen: set[tuple[str, str]] = set()  # (surface, class) already handled this chunk
        prov: list[str] = []  # provenance fact keys produced by THIS chunk
        for m in mentions:
            surface = str(m.get("label", "")).strip()
            cls_label = str(m.get("class", "")).strip()
            if not surface or not cls_label:
                continue
            if _is_non_identifying_label(surface):
                counts["rejected"] += 1
                continue
            cls_iri = class_index.get(skos.normalize_label(cls_label))
            if not cls_iri:
                unknown.append(cls_label)  # surface as a "suggested class", don't silently drop
                continue
            canonical_class_label = class_labels.get(cls_iri, cls_label)
            if not (
                (m.get("_role_verified") or m.get("_force_review"))
                and role_evidence.surface_is_grounded(source_text, surface)
                and role_evidence.evidence_is_grounded(source_text, m.get("evidence"))
            ):
                counts["rejected"] += 1
                continue
            # De-dup within the chunk: the same (surface, class) mentioned twice is one entity —
            # resolve/count it once (its assertions still merge idempotently in the loop below).
            key = (skos.normalize_label(surface), cls_iri)
            if key in seen:
                continue
            seen.add(key)
            iri, status = resolution.resolve_mention(
                session, ks_id=ks_id, abox_iri=abox_iri, base_iri=base_iri,
                surface=surface, class_iri=cls_iri, chunk_id=chunk_id,
                class_label=canonical_class_label, evidence=_evidence(m), agent=agent,
                pending_payload=_pending_payload(m, prop_index),
                related_classes=hierarchy.get(cls_iri, {cls_iri}),
                roles_by_class=roles_by_class,
                index=res_index,
                force_review_reason=str(m.get("_force_review") or "") or None,
                force_review_confidence=_role_confidence(m.get("_role_confidence")),
            )
            if status == "new":
                counts["created"] += 1
            elif status == "matched":
                counts["matched"] += 1
            elif status == "pending":
                counts["queued"] += 1
            if iri:
                local[(skos.normalize_label(surface), cls_iri)] = iri
                by_label.setdefault(skos.normalize_label(surface), set()).add(iri)
                prov.append(abox_provenance.ind_key(iri))  # this chunk mentioned this individual

        for m in mentions:
            m_cls = class_index.get(skos.normalize_label(str(m.get("class", ""))))
            subj = local.get((skos.normalize_label(str(m.get("label", ""))), m_cls)) if m_cls else None
            if not subj:
                continue  # subject unresolved (queued/skipped) → drop its assertions
            for a in _as_dicts(m.get("attributes")):
                pi = prop_index.get(skos.normalize_label(str(a.get("property", ""))))
                val = a.get("value")
                shape = property_shapes.get(pi[0], {}) if pi else {}
                if (
                    pi and pi[1] == "data" and val is not None and str(val).strip()
                    and _class_allowed(m_cls, shape.get("domain_members", []), ancestors)
                ):
                    v = str(val).strip()
                    if abox.add_data_assertion(abox_iri, subj, pi[0], v):
                        counts["assertions"] += 1  # count only triples actually added (idempotent re-runs)
                    prov.append(abox_provenance.data_key(subj, pi[0], v))  # this chunk asserted this value
            for r in _as_dicts(m.get("relations")):
                pi = prop_index.get(skos.normalize_label(str(r.get("property", ""))))
                # A bare target label carries no class; only link when it maps to exactly one
                # individual — otherwise it's ambiguous and mis-routing would corrupt the graph.
                tgts = by_label.get(skos.normalize_label(str(r.get("target", ""))))
                tgt = next(iter(tgts)) if tgts and len(tgts) == 1 else None
                shape = property_shapes.get(pi[0], {}) if pi else {}
                target_types = res_index.types_of(tgt) if tgt else set()
                domain_ok = _class_allowed(m_cls, shape.get("domain_members", []), ancestors)
                range_ok = not shape.get("range_members") or any(
                    _class_allowed(target_type, shape["range_members"], ancestors)
                    for target_type in target_types
                )
                if pi and pi[1] == "object" and tgt and domain_ok and range_ok:
                    if abox.add_object_assertion(abox_iri, subj, pi[0], tgt):
                        counts["assertions"] += 1
                    prov.append(abox_provenance.obj_key(subj, pi[0], tgt))  # this chunk asserted this relation

        abox_provenance.rebuild_for_chunk(
            session, ks_id, chunk_id, prov, job_id=job_id, actor_name=actor_name,
        )
        if commit:
            session.commit()
        else:
            session.flush()
    counts["unknown"] = unknown
    return counts


def _database_is_locked(exc: BaseException) -> bool:
    current: BaseException | None = exc
    while current is not None:
        if "database is locked" in str(current).casefold():
            return True
        current = current.__cause__ or current.__context__
    return False


async def _resolve_and_merge_with_retry(
    *args,
    db_session: Session | None = None,
    commit: bool = True,
) -> dict:
    """Retry transient SQLite writer contention caused by parallel knowledge-system jobs."""
    attempts = 4
    for attempt in range(attempts):
        try:
            return await asyncio.to_thread(
                _resolve_and_merge_chunk,
                *args,
                db_session=db_session,
                commit=commit,
            )
        except OperationalError as exc:
            # An externally-owned transaction cannot be retried piecemeal after a
            # database error: earlier chunks and RDF writes belong to that same
            # unit of work.  Let the job roll back the complete paired capture.
            if (
                db_session is not None
                or not _database_is_locked(exc)
                or attempt == attempts - 1
            ):
                raise
            delay = 0.5 * (2 ** attempt)
            logger.warning(
                "ABox SQLite writer contention; retrying chunk merge in %.1fs (%s/%s)",
                delay,
                attempt + 1,
                attempts - 1,
            )
            await asyncio.sleep(delay)
    raise RuntimeError("unreachable")


async def extract_instances_from_chunks(
    *,
    base_iri: str,
    graph_iri: str,
    abox_iri: str,
    ks_id: int,
    chunks: list[tuple[int, str]],
    job_id: int | None = None,
    actor_name: str = "extraction-agent",
    model: str | None = None,
    progress: ProgressCb = None,
    agentic_resolution: bool | None = None,
    session: Session | None = None,
    commit: bool = True,
    fail_fast: bool = False,
) -> dict:
    """Extract mentions concurrently, then resolve and merge them in source order.

    ``session``/``commit=False`` lets an API job include all SQL side effects in
    its outer transaction.  ``fail_fast=True`` is required for that atomic mode:
    a failed merge must escape to the paired graph captures so both RDF and SQL
    roll back.  Defaults retain the historical per-chunk standalone behaviour.
    """
    if not commit and session is None:
        raise ValueError("commit=False requires an externally managed session")
    view = schema.build_view(graph_iri)
    if not view["classes"]:
        return {"created": 0, "matched": 0, "queued": 0, "assertions": 0,
                "unknown_classes": {}, "per_chunk": [], "log": "No classes in the ontology — extract a TBox first."}

    vocabulary_graph = graph_iri.rstrip("/") + "/vocabulary"
    aliases = skos.mapped_aliases(vocabulary_graph)
    tbox_ctx = _format_tbox(view)
    class_index, prop_index = _indexes(view, aliases)
    hierarchy = _class_hierarchy(view)
    ancestors = _class_ancestors(view)
    roles_by_class = entity_roles.class_role_map(view)
    class_labels = {c["iri"]: c["label"] for c in view["classes"]}
    allowed_class_labels = [c["label"] for c in view["classes"]][:_MAX_CLASSES]
    prop_labels = {p["iri"]: p["label"] for p in view["object_properties"] + view["data_properties"]}
    property_shapes = {
        p["iri"]: {
            "domain_members": list(p.get("domain_members") or []),
            "range_members": list(p.get("range_members") or []),
        }
        for p in view["object_properties"] + view["data_properties"]
    }
    # One ABox scan up front; resolution keeps it in sync as it mints individuals, so each mention
    # is index lookups instead of ~3 full ABox scans (chunks run sequentially -> single-threaded).
    res_index = abox.build_resolution_index(abox_iri)
    use_agentic_resolution = settings.agentic_resolution if agentic_resolution is None else agentic_resolution
    totals = {"created": 0, "matched": 0, "queued": 0, "assertions": 0, "rejected": 0}
    unknown_classes: dict[str, int] = {}  # suggested class label -> times referenced
    per_chunk: list[dict] = []
    log_lines: list[str] = []
    total = len(chunks)

    mention_sem = asyncio.Semaphore(model_config.llm_concurrency())

    async def prepare_mentions(text: str) -> tuple[list[dict], int, str | None]:
        llm_error = None
        rejected = 0
        async with mention_sem:
            try:
                async with openrouter.capacity_slot():
                    async with asyncio.timeout(settings.llm_timeout_s * 3):
                        mentions = await _extract_one(text, model, tbox_ctx)
                        mentions, rejected = await _verify_abox_candidates(
                            text, mentions, model, allowed_class_labels,
                        )
                        mentions, self_typed_rejected = await _adjudicate_self_typed_candidates(
                            text, mentions, model, allowed_class_labels,
                        )
                rejected += self_typed_rejected
            except Exception as exc:  # noqa: BLE001
                mentions = []
                llm_error = str(exc)
        return mentions, rejected, llm_error

    mention_tasks = [asyncio.create_task(prepare_mentions(text)) for _, text in chunks]

    for i, (chunk_id, text) in enumerate(chunks):
        entry = {"chunk_id": chunk_id, "status": "ok", "created": 0, "matched": 0,
                 "queued": 0, "assertions": 0, "rejected": 0, "error": None}
        try:
            mentions, role_rejected, llm_error = await mention_tasks[i]
            res = await _resolve_and_merge_with_retry(
                ks_id, abox_iri, base_iri, chunk_id, job_id, actor_name,
                class_index, prop_index, class_labels, prop_labels, hierarchy,
                ancestors, property_shapes,
                roles_by_class, res_index, model, text, mentions, use_agentic_resolution,
                db_session=session,
                commit=commit,
            )
            res["rejected"] += role_rejected
            for lbl in res.pop("unknown", []):
                unknown_classes[lbl] = unknown_classes.get(lbl, 0) + 1
            entry.update(res)
            if llm_error:
                entry["status"] = "partial"
                entry["error"] = llm_error
            for k in totals:
                totals[k] += res[k]
            prefix = f"chunk {chunk_id}: PARTIAL ({llm_error}); " if llm_error else f"chunk {chunk_id}: "
            log_lines.append(prefix + (
                f"+{res['created']} new / {res['matched']} linked / {res['queued']} queued / "
                f"{res['rejected']} rejected / {res['assertions']} assertions"
            ))
        except Exception as e:  # noqa: BLE001  (one bad chunk shouldn't kill the job)
            if fail_fast:
                for task in mention_tasks[i + 1:]:
                    task.cancel()
                if i + 1 < len(mention_tasks):
                    await asyncio.gather(*mention_tasks[i + 1:], return_exceptions=True)
                raise
            entry["status"] = "error"
            entry["error"] = str(e)
            log_lines.append(f"chunk {chunk_id}: ERROR {e}")
            logger.warning("abox extraction failed on chunk %s: %s", chunk_id, e)
        per_chunk.append(entry)
        if progress:
            await progress({"type": "chunk", "index": i, "total": total, "chunk_id": chunk_id, "result": entry})

    return {**totals, "unknown_classes": unknown_classes, "per_chunk": per_chunk, "log": "\n".join(log_lines)}
