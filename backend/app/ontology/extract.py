"""LLM-driven TBox extraction.

Each selected chunk is sent to a cheap DeepSeek model with a strict-JSON ontology
schema. The returned classes/properties/axioms are merged into the knowledge system's
named graph, and every axiom is linked back to its source chunk (provenance).
"""
from __future__ import annotations

import asyncio
import json
import logging
import math
import re
import threading
from collections.abc import Awaitable, Callable

from app import model_config, prompt_config
from app.config import settings
from app.llm import openrouter
from app.ontology import retrieval, role_evidence, schema, skos, store, tbox_guard

logger = logging.getLogger(__name__)

ProgressCb = Callable[[dict], Awaitable[None]] | None

# Serializes graph writes across concurrent extraction workers (they run in threads).
_write_lock = threading.Lock()

_TBOX_ENTITY_BOUNDARY_RULES = """
Mandatory class-versus-individual boundary:
- A class is a reusable TYPE that can have multiple members. A concrete named or identified
  person, organization, product, document, place, event, record, or asset is an INDIVIDUAL.
- For every proposed class, copy a short exact source span into its `evidence` field. The class
  label itself must occur in the source; do not translate, rename, or manufacture it. The span
  must support the reusable type itself, not merely contain a concrete value from which a new
  type-like label could be invented.
- In structured `field: value`, JSON, YAML, or tabular data, a scalar value is not a class merely
  because it is capitalized or descriptive. Only an explicit type/kind/class/category declaration
  makes its value direct type evidence. A field name may itself denote a reusable concept.
- Never disguise a concrete value by appending or prepending a type word. If `Asset: Orion-7`
  occurs, `Orion-7`, `Orion-7 Asset`, and `Asset Orion-7` are not classes. The reusable `Asset`
  class is valid only when the source supports that general role.
- Do not promote a named individual to a class merely because no suitable class currently
  exists. If the general type is not supported by the text, omit the individual entirely.
- Existing ontology content is not authority for this boundary: never reuse an existing class
  that is visibly a named individual in the current text.
- Before finishing, test every class label with: "Could several different things be instances
  of this type?" If not, remove it from the entire TBox delta.

Mandatory subclass semantics:
- Emit `subclass_of(sub, super)` only when the sentence "Every sub is necessarily a super"
  remains true. This is an is-a relation, never shorthand for has-a, part-of, configured-by,
  located-in, managed-by, associated-with, or represented-by.
- Namespace membership, grouping, ownership, hosting, and implementation do not imply a
  subclass. A component used by an object is not thereby a subtype of that object.
- Re-read every proposed subclass edge with the substitution test before returning JSON. If
  the text merely mentions the terms near each other, omit the edge.
- Copy a short exact supporting span into every subclass row's `evidence` field.

Mandatory class-versus-datatype boundary:
- XML Schema datatypes are literal value types, never domain classes or range classes. Never put
  "string", "xsd:string", "integer", "decimal", "boolean", "date", or "dateTime" in
  classes, object_properties, subclass_of, disjoint_with, or equivalent_class.
- Use a data_property for literal text, numbers, booleans, and dates. Its JSON range MUST be one
  bare token from string|integer|decimal|boolean|date|dateTime; do not prefix it with "xsd:".
- Use an object_property only when its value is another entity; both domain and range must be
  reusable class labels.

Boundary examples:
- Text: "Alice operates Pump P-101."
  Valid classes: "Person", "Pump". Forbidden classes: "Alice", "Pump P-101".
- Text: "Asset: Orion-7. Type: Centrifugal Pump."
  Valid classes: "Asset", "Centrifugal Pump". Forbidden classes: "Orion-7",
  "Orion-7 Asset", and "Orion-7 Pump".
"""

_SYSTEM_PROMPT = """You are an ontology engineer. From the given text, extract a lightweight OWL \
TBox (a schema-level ontology of general concepts and their relations) — NOT specific \
instances/individuals (no ABox).

Return ONLY a single JSON object with exactly these keys (use [] when empty):
{
  "classes": [{"label": "<natural-language singular noun, e.g. 'Pump Station'>", "comment": "<short gloss>", "evidence": "<exact source span>"}],
  "object_properties": [{"label": "<verb phrase, e.g. 'has component'>", "domain": "<class label>", "range": "<class label>", "comment": ""}],
  "data_properties": [{"label": "<attribute, e.g. 'nominal pressure'>", "domain": "<class label>", "range": "string|integer|decimal|boolean|date|dateTime", "comment": ""}],
  "subclass_of": [{"sub": "<child class label>", "super": "<parent class label>", "evidence": "<exact source span>"}],
  "disjoint_with": [{"a": "<class label>", "b": "<class label>"}],
  "equivalent_class": [{"a": "<class label>", "b": "<class label>"}]
}

""" + _TBOX_ENTITY_BOUNDARY_RULES + """
Additional rules:
- Write every label and comment in the SAME language as the source text (Chinese text →
  Chinese labels, English text → English labels). Do not translate.
- Extract only concepts/relations actually supported by the text.
- Classes are general kinds (singular). Do NOT create classes for named individuals.
- A specific one-off occurrence (a particular training session, drill, inspection, meeting, or
  incident) is an INDIVIDUAL, not a class — don't create a class for it; capture the general kind.
- Treat a PROPER NAME — a label that names ONE specific entity — as an INDIVIDUAL, not a class, even
  when it reads like a compound noun and carries no number/date/code. Extract only the GENERAL KIND it
  is an instance of as a class; leave the named entity for instance (ABox) extraction. Heuristic: if
  the label denotes one particular thing rather than a category that could have several members, it is
  an individual.
- Give every new class a broader parent via subclass_of when the text supports one, so concepts
  aren't left unattached (e.g. a specific kind of drill ⊑ a general "activity/event" class).
- Reuse the same label consistently so identical concepts merge.
- Reuse an object property only when the relation's meaning and role are the same. Prefer a
  meaningful general verb such as "owns" over range-specific variants, but never collapse
  distinct structural roles into a content-free predicate such as "has" or "has x". Relations
  such as "has label", "has lease", and "has template" are distinct when the source says so.
- Only assert disjoint_with / equivalent_class when the text clearly implies it.
- If an EXISTING ONTOLOGY is provided, REUSE its exact class/property labels for any
  concept it already covers (do NOT invent near-duplicate names); introduce new labels
  only for genuinely new concepts, and attach new classes under existing ones with
  subclass_of where the text supports it.
- If the text has no ontological content, return all empty arrays.
- Output must be valid JSON with no surrounding prose."""

prompt_config.register(
    key="tbox.extract.rag",
    category="extraction",
    title="TBox extraction",
    description="Extract schema-level classes, properties, and axioms from each source chunk.",
    default=_SYSTEM_PROMPT,
    order=10,
)

_MAX_CLASSES_CTX = 400
_MAX_PROPS_CTX = 200

_TBOX_CRITIC_PROMPT = """You are an independent ontology-boundary critic. The first extractor is
untrusted and may turn a concrete value into a type-like label. Judge every candidate only from
the supplied source text; do not use outside domain knowledge.

For each CLASS candidate choose exactly one role:
- type: a reusable category that can have multiple instances;
- individual: one concrete named/identified entity or controlled entry;
- literal: a scalar, measurement, status, option, identifier value, or descriptive text;
- uncertain: the source does not establish the role.

Reject labels manufactured from a concrete value by adding a type word. For example, source
`Asset: Orion-7` does not support classes `Orion-7 Asset` or `Orion-7 Device`. A structured scalar
is not a type unless a type/kind/class/category declaration or prose explicitly says so.

For each SUBCLASS candidate keep it only when every SUB is necessarily a SUPER and an exact source
span supports that is-a relation. Reject part-of, field-of, value-of, status-of, managed-by,
created-by, used-by, grouping, implementation, and mere co-occurrence.

Return ONLY:
{
  "class_decisions": [
    {"label":"<exact candidate label>","role":"type|individual|literal|uncertain",
     "keep":true,"confidence":0.0,"evidence":"<short exact source span>","reason":"<short reason>"}
  ],
  "subclass_decisions": [
    {"sub":"<exact candidate sub>","super":"<exact candidate super>",
     "keep":true,"confidence":0.0,"evidence":"<short exact source span>",
     "reason":"<short substitution-test reason>"}
  ]
}

Do not add, rename, or repair candidates. Evidence must be copied from the source text. Use
keep=false or role=uncertain when evidence is absent."""

prompt_config.register(
    key="tbox.boundary.critic",
    category="review",
    title="TBox boundary critic",
    description="Verify that proposed classes are reusable types rather than individuals or literals.",
    default=_TBOX_CRITIC_PROMPT,
    order=10,
)

_TBOX_BOUNDARY_ADJUDICATOR_PROMPT = """You are the final adjudicator for class candidates that
one ontology critic rejected. The extractor and first critic disagreed. Re-evaluate each candidate
only from the supplied source text; do not use outside domain knowledge and do not add labels.

A candidate is a reusable TYPE only when the text uses it generically for a category that can have
multiple members. Strong type evidence includes an indefinite or generic use (for example, "a/an
X", "each X", or generic plural Xs), an explicit type/kind/class/category declaration, or a
definition that clearly applies to repeatable members. Capitalization alone is neither positive nor
negative evidence.

A proper name remains an INDIVIDUAL when the text says that named subject is a type of something
(for example, "Argentina is a country" or "Blue Danube Wine Co. is a winery"). Quoted names,
identifiers, records, places, organizations, products, and one-off events are not classes merely
because an extractor proposed them. Mere mention or co-occurrence is insufficient.

Return ONLY:
{
  "class_decisions": [
    {"label":"<exact candidate label>","role":"type|individual|literal|uncertain",
     "keep":true,"confidence":0.0,"evidence":"<short exact source span>",
     "reason":"<short repeatability/proper-name reason>"}
  ],
  "subclass_decisions": []
}

Copy evidence exactly from the source. Set keep=false unless the source establishes a reusable
type with high confidence."""

prompt_config.register(
    key="tbox.boundary.adjudicator",
    category="review",
    title="TBox boundary adjudicator",
    description="Re-evaluate grounded class candidates when extraction and the first boundary critic disagree.",
    default=_TBOX_BOUNDARY_ADJUDICATOR_PROMPT,
    order=15,
)

_TBOX_DENOTATION_CRITIC_PROMPT = """You are the final independent ontology denotation critic.
Earlier extraction stages proposed every supplied label, but may have accepted or rejected it.
Apply a stricter modeling convention: distinguish a repeatable category from one named design,
variant, place, organization, standard, mode, algorithm, product, or software module.

- A full label is a TYPE only when distinct members can instantiate that full category. Text that
  uses "a/an", "each/every", a generic plural, or an explicit type/kind/class definition is strong
  positive evidence.
- Copies, deployments, installations, configurations, or executions of one named design do not make
  the named design itself a class. Model the named design as an INDIVIDUAL of its reusable general
  type; model runtime copies separately when the source discusses them.
- A proper-name-plus-generic-head phrase such as "FalconGuard admission plugin" normally denotes the
  one named plugin design. Reject the full phrase. When its reusable generic head occurs as an exact
  suffix, you MUST recover the longest meaningful suffix ("admission plugin", not merely "plugin")
  as a replacement class; its occurrence inside the full phrase is sufficient lexical evidence.
- By contrast, a phrase used as a repeatable schema category, such as "an ExternalName Service" or
  "each ConfigMap", remains a TYPE.
- Do not use outside knowledge. Capitalization alone proves nothing. Copy evidence from the source.

Return ONLY:
{
  "class_decisions": [
    {"label":"<exact candidate>","role":"type|individual|literal|uncertain",
     "keep":true,"confidence":0.0,"evidence":"<exact source span>","reason":"<short reason>"}
  ],
  "replacement_classes": [
    {"from":"<rejected exact candidate>","label":"<exact reusable suffix from source>",
     "confidence":0.0,"evidence":"<exact source span>","reason":"<short reason>"}
  ],
  "subclass_decisions": []
}

For every rejected proper-name-plus-generic-head individual, include a replacement when an exact
reusable suffix exists in the source. Only omit it when no such suffix exists. Never invent or
translate the replacement."""

prompt_config.register(
    key="tbox.denotation.critic",
    category="review",
    title="TBox denotation critic",
    description="Distinguish reusable classes from named designs or variants and recover their general type.",
    default=_TBOX_DENOTATION_CRITIC_PROMPT,
    order=20,
)

_TBOX_CORPUS_ROLE_RECOVERY_PROMPT = """You are a corpus-level ontology boundary adjudicator.
Earlier per-passage critics rejected the supplied class candidates, but a short passage can be
ambiguous or omit the definition found elsewhere. Re-evaluate every candidate from ALL supplied
source passages together. Do not use outside knowledge and do not add or rename labels.

A candidate is a reusable TYPE when at least one passage explicitly establishes that exact label
as a class, category, kind, reusable role, superclass, or definition applying to multiple possible
members. A generic singular/plural use or an explicit class hierarchy statement is also positive
evidence. Other passages may use the same type label in examples without changing its type role.

The reusable type must belong to the domain model described by the source. Do not promote terms
that only describe the publication, vocabulary, standardization activity, authorship, tooling, or
document discourse. Such a term is in scope only when the passages explicitly model its possible
members as domain entities, rather than merely mentioning the artifact that contains the model.

A candidate is an INDIVIDUAL only when the full label identifies one particular person, place,
organization, product, document, event, record, asset, design, or controlled entry. A scalar,
identifier value, status, option, measurement, or datatype is a LITERAL. Use UNCERTAIN when the
passages never establish a reusable type or a particular identity. A direct statement that the
exact label is an "instance" or "individual" is authoritative identity evidence and must not be
overridden by another passage describing what that named instance categorizes or represents.

Return ONLY:
{"class_decisions":[
  {"label":"<exact candidate label>","role":"type|individual|literal|uncertain",
   "keep":true,"confidence":0.0,"evidence":"<exact span from one supplied passage>",
   "reason":"<short corpus-level reason>"}
]}

Every candidate needs one decision. Evidence must be copied exactly from a supplied passage. Set
keep=true only for role=type with high confidence; otherwise set keep=false."""

prompt_config.register(
    key="tbox.boundary.evidence_selector",
    category="review",
    title="Corpus evidence selector",
    description="Select the strongest source passages for corpus-level type boundary review.",
    default="""You are a source-evidence curator for ontology boundary review.
For every exact candidate label, select the passages that best let a later adjudicator determine
whether it denotes a reusable DOMAIN TYPE, a particular individual, a literal value, or document
metadata. Do not make the final role decision and do not use outside knowledge.

Prefer direct definitions, explicit class/category declarations, reusable membership statements,
and class hierarchy statements. Also retain a contradictory passage when it directly identifies
the label as one particular entity or as publication, vocabulary, standardization, authorship, or
tooling discourse. Mere repetition and navigational mentions are weak evidence.

Return ONLY:
{"evidence_selections":[
  {"label":"<exact candidate label>","passage_ids":["p1","p3"],
   "reason":"<short selection reason>"}
]}

Every candidate needs one entry. Select one to four supplied passage IDs per candidate, ordered
strongest first. Never invent a passage ID or alter a label.""",
    order=21,
)

prompt_config.register(
    key="tbox.boundary.corpus_recovery",
    category="review",
    title="Corpus-level TBox boundary recovery",
    description="Re-evaluate rejected class candidates using evidence gathered across source chunks.",
    default=_TBOX_CORPUS_ROLE_RECOVERY_PROMPT,
    order=22,
)


def _existing_context(graph_iri: str) -> str:
    """Compact summary of the knowledge system's current ontology, so extraction aligns
    to and extends it instead of re-deriving concepts in isolation."""
    view = schema.build_view(graph_iri)
    classes = [c["label"] for c in view["classes"]]
    obj = [p["label"] for p in view["object_properties"]]
    data = [p["label"] for p in view["data_properties"]]
    if not (classes or obj or data):
        return ""

    def fmt(items: list[str], cap: int) -> str:
        shown = items[:cap]
        s = "、".join(shown)
        extra = len(items) - len(shown)
        return s + (f" …(and {extra} more)" if extra > 0 else "")

    lines = [
        "EXISTING ONTOLOGY (reuse these exact labels for concepts already covered; "
        "only add genuinely new ones, and hook new classes under existing ones):",
    ]
    if classes:
        lines.append(f"- classes: {fmt(classes, _MAX_CLASSES_CTX)}")
    if obj:
        lines.append(f"- object properties: {fmt(obj, _MAX_PROPS_CTX)}")
    if data:
        lines.append(f"- data properties: {fmt(data, _MAX_PROPS_CTX)}")
    return "\n".join(lines) + "\n\n"


def _retrieved_context(graph_iri: str, chunk_text: str) -> str:
    """Retrieval-augmented context: only the ontology entities *relevant* to this chunk
    (vector search), not the whole ontology — scales to large ontologies and stays focused."""
    hits = retrieval.search_ontology(graph_iri, chunk_text, k=12)
    if not hits:
        return ""
    lines = ["RELEVANT EXISTING ONTOLOGY (reuse these exact labels for concepts already "
             "present; attach new classes under an appropriate existing class):"]
    for h in hits:
        if h["kind"] == "class":
            sup = f" (subclass of: {', '.join(h['superclasses'])})" if h.get("superclasses") else ""
            com = f" — {h['comment']}" if h.get("comment") else ""
            lines.append(f"- [class] {h['label']}{sup}{com}")
        else:
            kind = "object property" if h["kind"] == "object_property" else "data property"
            dr = f" ({h.get('domain') or '?'} → {h.get('range') or '?'})" if (h.get("domain") or h.get("range")) else ""
            lines.append(f"- [{kind}] {h['label']}{dr}")
    return "\n".join(lines) + "\n\n"


def _context_for(graph_iri: str, chunk_text: str) -> str:
    """Sync context builder (runs in a thread — it makes a blocking embedding call)."""
    return _retrieved_context(graph_iri, chunk_text) or _existing_context(graph_iri)


async def _extract_one(text: str, model: str | None, existing_ctx: str = "") -> dict:
    user = f"{existing_ctx}Text:\n\"\"\"\n{text}\n\"\"\"\n\nReturn the JSON object."
    messages = [
        {"role": "system", "content": prompt_config.get("tbox.extract.rag")},
        {"role": "user", "content": user},
    ]
    reply = await openrouter.chat(messages, model=model)
    data = openrouter.extract_json(reply)
    if not isinstance(data, dict):
        raise openrouter.LLMError("LLM did not return a JSON object")
    return data


_AGENT_SYSTEM_PROMPT = """You are an ontology-engineering agent that extends an EXISTING \
ontology from a text chunk. Before deciding, you may inspect the current ontology with tools.

Respond with EXACTLY ONE JSON object per turn — one of:
1) {"action": "search_ontology", "query": "<concept/phrase>"}
   → returns existing classes/properties semantically related to the query (find whether a
     concept already exists and its EXACT label).
2) {"action": "get_neighborhood", "class": "<existing class label>"}
   → returns that class's superclasses/subclasses/properties (decide where to attach).
3) {"action": "finish", "ontology": { ...the extracted TBox delta... }}

The "ontology" object uses exactly these keys (use [] when empty):
{
  "classes": [{"label": "...", "comment": "...", "evidence": "<exact source span>"}],
  "object_properties": [{"label": "...", "domain": "<class label>", "range": "<class label>", "comment": ""}],
  "data_properties": [{"label": "...", "domain": "<class label>", "range": "string|integer|decimal|boolean|date|dateTime", "comment": ""}],
  "subclass_of": [{"sub": "...", "super": "...", "evidence": "<exact source span>"}],
  "disjoint_with": [{"a": "...", "b": "..."}],
  "equivalent_class": [{"a": "...", "b": "..."}]
}

""" + _TBOX_ENTITY_BOUNDARY_RULES + """
Additional rules:
- Write every label and comment in the SAME language as the source text (Chinese text →
  Chinese labels). Do not translate.
- First classify named entities versus general concepts. Search for reusable GENERAL TYPES,
  never for a concrete proper name or identifier.
- Then search the ontology for the key concepts in the text. REUSE the exact existing
  labels for concepts already present; introduce new labels only for genuinely new concepts;
  attach new classes under an existing class with subclass_of where the text supports it.
- Keep tool use minimal (a few searches); when confident, finish.
- Reuse an object property only when the relation's meaning and role are the same. Prefer a
  meaningful general verb over range-specific variants, but never collapse distinct structural
  roles into a content-free predicate such as "has" or "has x".
- Extract only what the text supports; no ABox individuals.
- Output MUST be a single valid JSON object with no surrounding prose."""

prompt_config.register(
    key="tbox.extract.agent",
    category="extraction",
    title="Agentic TBox extraction",
    description="Extend an existing ontology after searching its relevant classes and properties.",
    default=_AGENT_SYSTEM_PROMPT,
    order=20,
)

_HIERARCHY_RECOVERY_PROMPT = """You are a specialist in recovering EXPLICIT ontology class
hierarchies that a general extractor may have missed. Read the source and the supplied EXISTING
CLASSES, then return directly supported is-a relations for those classes. You may also recover a
missing reusable superclass when its exact label and the is-a statement both occur in the source.

Return ONLY:
{
  "classes":[{"label":"<exact missing superclass label>","comment":"",
              "evidence":"<short exact source span>"}],
  "subclass_of":[{"sub":"<exact existing class>",
                  "super":"<exact existing or recovered superclass>",
                  "evidence":"<short exact source span>"}]
}

Rules:
- Every `sub` must be an exact label from EXISTING CLASSES.
- A `super` may be an exact existing label or a missing reusable type copied exactly from the
  source. Declare each missing superclass in `classes`; never emit an unconnected class.
- Never rename, translate, combine, or infer a label that does not occur in the source.
- Add an edge only when the source explicitly supports "Every SUB is necessarily a SUPER".
- Definitions such as "X is a Y", "X is a type/kind/form of Y", and an explicit statement that
  X is an object/component/resource are valid when X is used as a reusable type in that statement.
- If an existing label is used as one concrete proper name in the source, do not attach it as a
  subclass even when the sentence says that named thing is a type of something.
- Part-of, contains, uses, creates, manages, runs-on, configured-by, association, co-occurrence,
  and a shared topic are NOT subclass relations.
- Copy the decisive wording verbatim into evidence. Do not rely on outside knowledge, even when
  the domain is familiar. If the source has no explicit hierarchy, return both arrays empty."""

prompt_config.register(
    key="tbox.hierarchy.recovery",
    category="extraction",
    title="TBox hierarchy recovery",
    description="Recover explicit is-a relations missed by the general extractor, with source evidence.",
    default=_HIERARCHY_RECOVERY_PROMPT,
    order=25,
)

_SUBCLASS_CRITIC_PROMPT = """You are an independent ontology subclass critic. The endpoint
labels are already admitted reusable classes; do NOT reclassify or reject those classes. Judge
only whether each proposed directed edge is a valid is-a relation in the supplied source text.

Keep an edge only when the exact source supports: every SUB is necessarily a SUPER. Definitions,
explicit superclass/subclass statements, and phrases such as "X is a Y" or "X generalizes Y" are
valid when used for reusable classes. Reject part-of, contains, uses, creates, manages, located-in,
configured-by, grouping, implementation, and mere co-occurrence.

Return ONLY:
{"subclass_decisions":[
  {"sub":"<exact proposed sub>","super":"<exact proposed super>","keep":true,
   "confidence":0.0,"evidence":"<short exact source span>","reason":"<short reason>"}
]}

Return one decision for every proposed edge. Do not add, rename, reverse, or repair edges. Evidence
must be copied exactly from the source text."""

prompt_config.register(
    key="tbox.hierarchy.critic",
    category="review",
    title="Subclass edge critic",
    description="Verify hierarchy edges without re-litigating already admitted endpoint classes.",
    default=_SUBCLASS_CRITIC_PROMPT,
    order=27,
)


def _should_use_agentic(graph_iri: str) -> bool:
    mode = settings.extraction_mode
    if mode == "agentic":
        return True
    if mode == "rag":
        return False
    return schema.graph_stats(graph_iri)["class_count"] >= settings.agentic_min_classes


async def _agentic_extract_one(text: str, model: str | None, graph_iri: str) -> dict:
    """ReAct-style loop: the LLM calls retrieval tools, then finishes with the TBox delta."""
    messages = [
        {"role": "system", "content": prompt_config.get("tbox.extract.agent")},
        {"role": "user", "content": f"Text:\n\"\"\"\n{text}\n\"\"\"\n\nInspect the ontology as needed, then finish with the extracted TBox."},
    ]
    for _ in range(settings.agentic_max_steps):
        reply = await openrouter.chat(messages, model=model)
        data = openrouter.extract_json(reply)
        if not isinstance(data, dict):
            messages.append({"role": "user", "content": "Respond with a single JSON object (an action or finish)."})
            continue
        action = data.get("action")
        logger.info("agent step: action=%s query=%r class=%r", action, data.get("query"), data.get("class"))
        if action == "finish":
            onto = data.get("ontology")
            if isinstance(onto, dict):
                return onto
            messages.append({"role": "user", "content": "finish must include an 'ontology' object."})
            continue
        if action == "search_ontology":
            res = await asyncio.to_thread(retrieval.search_ontology, graph_iri, str(data.get("query", "")), 8)
            tool_out = f"search_ontology result:\n{json.dumps(res, ensure_ascii=False)}"
        elif action == "get_neighborhood":
            res = await asyncio.to_thread(retrieval.get_neighborhood, graph_iri, str(data.get("class", "")))
            tool_out = f"get_neighborhood result:\n{json.dumps(res, ensure_ascii=False)}"
        else:
            tool_out = "Unknown action. Use search_ontology, get_neighborhood, or finish."
        messages.append({"role": "assistant", "content": reply})
        messages.append({"role": "user", "content": tool_out})
    raise openrouter.LLMError("agent did not finish within the step limit")


async def _extract_for_chunk(text: str, model: str | None, graph_iri: str, use_agentic: bool) -> dict:
    """Extract one chunk (agentic tool-loop or retrieval-augmented single-shot)."""
    if use_agentic:
        try:
            return await _agentic_extract_one(text, model, graph_iri)
        except Exception as e:  # noqa: BLE001  (agent hiccup -> fall back to RAG)
            logger.warning("agentic extraction failed (%s); falling back to RAG", e)
    ctx = await asyncio.to_thread(_context_for, graph_iri, text)
    return await _extract_one(text, model, ctx)


def _subclass_pair(row: dict) -> tuple[str, str]:
    sub = row.get("sub") or row.get("child") or row.get("subclass") or ""
    parent = row.get("super") or row.get("parent") or row.get("superclass") or ""
    return str(sub).strip(), str(parent).strip()


def _confidence(value: object) -> float:
    try:
        confidence = float(value or 0.0)
    except (TypeError, ValueError):
        return 0.0
    return max(0.0, min(1.0, confidence)) if math.isfinite(confidence) else 0.0


def _has_independent_type_evidence(label: str, decision: dict | None) -> bool:
    evidence = str((decision or {}).get("evidence", "")).strip()
    if not evidence:
        return False
    return role_evidence.normalize(label) not in role_evidence.structured_non_type_values(evidence)


def _evenly_sampled(rows: list, limit: int) -> list:
    if limit <= 0 or not rows:
        return []
    if len(rows) <= limit:
        return list(rows)
    if limit == 1:
        return [rows[len(rows) // 2]]
    indexes = {
        round(index * (len(rows) - 1) / (limit - 1))
        for index in range(limit)
    }
    return [rows[index] for index in sorted(indexes)]


def _label_evidence_windows(
    text: str,
    label: str,
    *,
    radius: int = 320,
    limit: int = 2,
) -> list[str]:
    forms = []
    for form in (label, label.rsplit(":", 1)[-1], label.replace("_", " ")):
        if form and form.casefold() not in {item.casefold() for item in forms}:
            forms.append(form)
    positions = sorted({
        match.start()
        for form in forms
        for match in re.finditer(re.escape(form), text, flags=re.IGNORECASE)
    })
    if not positions:
        return [text[: 2 * radius]] if text else []
    windows: list[str] = []
    for position in _evenly_sampled(positions, limit):
        start = max(0, position - radius)
        end = min(len(text), position + len(label) + radius)
        window = text[start:end]
        if window and window not in windows:
            windows.append(window)
    return windows


def _prepare_corpus_evidence(candidates: dict[str, dict]) -> dict[str, dict]:
    prepared: dict[str, dict] = {}
    for normalized, candidate in candidates.items():
        label = str(candidate.get("label", "")).strip()
        occurrences = [
            row for row in candidate.get("occurrences", [])
            if isinstance(row, dict) and isinstance(row.get("text"), str)
        ]
        passages: list[dict] = []
        seen: set[tuple[object, str]] = set()
        for occurrence in _evenly_sampled(occurrences, 8):
            for window in _label_evidence_windows(occurrence["text"], label):
                key = (occurrence.get("chunk_id"), window)
                if key in seen:
                    continue
                seen.add(key)
                passages.append({
                    "passage_id": f"p{len(passages) + 1}",
                    "chunk_id": occurrence.get("chunk_id"),
                    "text": window,
                    "earlier_reason": str(occurrence.get("earlier_reason", "")),
                    "extractor_evidence": str(occurrence.get("extractor_evidence", "")),
                })
        prepared[normalized] = {"label": label, "passages": passages}
    return prepared


def _apply_corpus_evidence_selections(
    prepared: dict[str, dict],
    payload: dict,
    *,
    limit: int = 4,
) -> dict[str, list[dict]]:
    decisions = {
        skos.normalize_label(str(row.get("label", ""))): row
        for row in payload.get("evidence_selections", [])
        if isinstance(row, dict)
    } if isinstance(payload, dict) and isinstance(payload.get("evidence_selections"), list) else {}
    selected: dict[str, list[dict]] = {}
    for normalized, candidate in prepared.items():
        passages = [row for row in candidate.get("passages", []) if isinstance(row, dict)]
        by_id = {str(row.get("passage_id", "")): row for row in passages}
        decision = decisions.get(normalized)
        passage_ids = decision.get("passage_ids", []) if isinstance(decision, dict) else []
        chosen: list[dict] = []
        seen_ids: set[str] = set()
        for passage_id in passage_ids if isinstance(passage_ids, list) else []:
            key = str(passage_id)
            if key in by_id and key not in seen_ids:
                seen_ids.add(key)
                chosen.append(by_id[key])
            if len(chosen) >= limit:
                break
        selected[normalized] = chosen or _evenly_sampled(passages, limit)
    return selected


def _apply_corpus_role_decisions(
    candidates: dict[str, dict],
    payload: dict,
    structured_non_type_signals: set[str] | None = None,
) -> list[dict]:
    """Apply corpus-level role decisions with deterministic source grounding.

    ``candidates`` is keyed by normalized label. Each value contains ``label`` and a list of
    ``occurrences`` with ``chunk_id`` and ``text``. The helper is intentionally domain-neutral and
    side-effect free so the fail-closed boundary can be regression-tested without an LLM or graph.
    """
    decisions = {
        skos.normalize_label(str(row.get("label", ""))): row
        for row in payload.get("class_decisions", [])
        if isinstance(row, dict)
    } if isinstance(payload, dict) and isinstance(payload.get("class_decisions"), list) else {}
    structured_signals = structured_non_type_signals or set()
    accepted: list[dict] = []
    for normalized, candidate in candidates.items():
        decision = decisions.get(normalized)
        label = str(candidate.get("label", "")).strip()
        evidence = str((decision or {}).get("evidence", "")).strip()
        occurrences = [
            row for row in candidate.get("occurrences", [])
            if isinstance(row, dict) and isinstance(row.get("text"), str)
        ]
        support = next(
            (
                row for row in occurrences
                if role_evidence.surface_is_grounded(row["text"], label)
                and role_evidence.evidence_is_grounded(row["text"], evidence)
            ),
            None,
        )
        explicitly_individual = any(
            role_evidence.has_explicit_individual_declaration(row["text"], label)
            for row in occurrences
        )
        if (
            (
                role_evidence.normalize(label) in structured_signals
                and not _has_independent_type_evidence(label, decision)
            )
            or explicitly_individual
            or tbox_guard.canonical_datatype_name(label) is not None
            or not decision
            or decision.get("keep") is not True
            or str(decision.get("role", "")).strip().casefold() != role_evidence.ROLE_TYPE
            or _confidence(decision.get("confidence")) < settings.role_auto_accept_floor
            or support is None
        ):
            continue
        accepted.append({
            "label": label,
            "comment": "",
            "evidence": evidence,
            "_role_verified": True,
            "chunk_id": support.get("chunk_id"),
            "source_text": support["text"],
        })
    return accepted


def _apply_subclass_decisions(
    text: str,
    proposed: list[dict],
    payload: dict,
    allowed_norms: set[str],
) -> list[dict]:
    """Apply dedicated edge decisions while trusting only already admitted endpoints."""
    decisions: dict[tuple[str, str], dict] = {}
    rows = payload.get("subclass_decisions", []) if isinstance(payload, dict) else []
    for row in rows if isinstance(rows, list) else []:
        if not isinstance(row, dict):
            continue
        key = tuple(skos.normalize_label(value) for value in _subclass_pair(row))
        if all(key):
            decisions[key] = row
    accepted: list[dict] = []
    for row in proposed:
        sub, parent = _subclass_pair(row)
        key = (skos.normalize_label(sub), skos.normalize_label(parent))
        decision = decisions.get(key)
        evidence = str((decision or {}).get("evidence", "")).strip()
        if (
            not decision
            or not all(endpoint in allowed_norms for endpoint in key)
            or decision.get("keep") is not True
            or _confidence(decision.get("confidence")) < settings.role_auto_accept_floor
            or not role_evidence.evidence_is_grounded(text, evidence)
        ):
            continue
        accepted.append({"sub": sub, "super": parent, "evidence": evidence})
    return accepted


def _apply_tbox_role_decisions(text: str, ontology: dict, payload: dict) -> dict:
    """Apply critic output with deterministic evidence checks; useful for regression tests."""
    raw_classes = ontology.get("classes", [])
    class_rows = [row for row in raw_classes if isinstance(row, dict)] \
        if isinstance(raw_classes, list) else []
    raw_subclasses = ontology.get("subclass_of", [])
    subclass_rows = [row for row in raw_subclasses if isinstance(row, dict)] \
        if isinstance(raw_subclasses, list) else []

    structured_roles = role_evidence.structured_value_roles(text)
    class_decisions: dict[str, dict] = {}
    decisions = payload.get("class_decisions", []) if isinstance(payload, dict) else []
    for decision in decisions if isinstance(decisions, list) else []:
        if isinstance(decision, dict):
            label = skos.normalize_label(str(decision.get("label", "")))
            if label:
                class_decisions[label] = decision

    accepted_classes: list[dict] = []
    rejected: list[dict[str, str]] = []
    for row in class_rows:
        label = str(row.get("label") or row.get("name") or "").strip()
        normalized = skos.normalize_label(label)
        decision = class_decisions.get(normalized)
        roles = structured_roles.get(role_evidence.normalize(label), set())
        exact_non_type = (
            role_evidence.ROLE_LITERAL in roles
            and role_evidence.ROLE_TYPE not in roles
        )
        independent_type_evidence = _has_independent_type_evidence(label, decision)
        label_grounded = role_evidence.surface_is_grounded(text, label)
        accepted = bool(
            decision
            and decision.get("keep") is True
            and str(decision.get("role", "")).strip().casefold() == role_evidence.ROLE_TYPE
            and _confidence(decision.get("confidence")) >= settings.role_auto_accept_floor
            and role_evidence.evidence_is_grounded(text, decision.get("evidence"))
            and label_grounded
            and (not exact_non_type or independent_type_evidence)
        )
        if accepted:
            cleaned = dict(row)
            cleaned["evidence"] = str(decision.get("evidence", "")).strip()
            cleaned["_role_verified"] = True
            accepted_classes.append(cleaned)
        else:
            reason = "missing or ungrounded independent type decision"
            if exact_non_type and not independent_type_evidence:
                reason = "exact structured scalar value is not declared as a type"
            elif not label_grounded:
                reason = "class label is not lexically grounded in the source"
            elif decision and decision.get("reason"):
                reason = str(decision["reason"])
            rejected.append({
                "label": label,
                "reason": reason,
                "evidence": str(row.get("evidence") or "").strip(),
                "comment": str(row.get("comment") or "").strip(),
            })

    subclass_decisions: dict[tuple[str, str], dict] = {}
    decisions = payload.get("subclass_decisions", []) if isinstance(payload, dict) else []
    for decision in decisions if isinstance(decisions, list) else []:
        if not isinstance(decision, dict):
            continue
        key = (
            skos.normalize_label(str(decision.get("sub", ""))),
            skos.normalize_label(str(decision.get("super", ""))),
        )
        if all(key):
            subclass_decisions[key] = decision

    accepted_subclasses: list[dict] = []
    for row in subclass_rows:
        pair = _subclass_pair(row)
        key = tuple(skos.normalize_label(value) for value in pair)
        decision = subclass_decisions.get(key)
        if not (
            decision
            and decision.get("keep") is True
            and _confidence(decision.get("confidence")) >= settings.role_auto_accept_floor
            and role_evidence.evidence_is_grounded(text, decision.get("evidence"))
        ):
            continue
        cleaned = dict(row)
        cleaned["evidence"] = str(decision.get("evidence", "")).strip()
        accepted_subclasses.append(cleaned)

    return {
        **ontology,
        "classes": accepted_classes,
        "subclass_of": accepted_subclasses,
        "_role_rejections": rejected,
    }


def _remove_rejected_class_references(ontology: dict, rejected_norms: set[str]) -> dict:
    if not rejected_norms:
        return ontology
    out = dict(ontology)

    def rejected(value: object) -> bool:
        return isinstance(value, str) and skos.normalize_label(value) in rejected_norms

    def first(row: dict, fields: tuple[str, ...]) -> str:
        for field in fields:
            value = row.get(field)
            if isinstance(value, str) and value.strip():
                return value.strip()
        return ""

    for key, slots in (("object_properties", ("domain", "range")), ("data_properties", ("domain",))):
        cleaned_rows: list[dict] = []
        rows = ontology.get(key, [])
        for row in rows if isinstance(rows, list) else []:
            if not isinstance(row, dict):
                continue
            cleaned = dict(row)
            for slot in slots:
                if rejected(cleaned.get(slot)):
                    cleaned.pop(slot, None)
            cleaned_rows.append(cleaned)
        out[key] = cleaned_rows

    for key, field_groups in (
        ("subclass_of", (("sub", "child", "subclass"), ("super", "parent", "superclass"))),
        ("disjoint_with", (("a",), ("b",))),
        ("equivalent_class", (("a",), ("b",))),
    ):
        rows = ontology.get(key, [])
        out[key] = [
            dict(row)
            for row in (rows if isinstance(rows, list) else []) if isinstance(row, dict)
            if not any(rejected(first(row, fields)) for fields in field_groups)
        ]
    return out


def _denotation_replacements(
    text: str,
    payload: dict,
    original_by_norm: dict[str, dict],
    rejected_norms: set[str],
) -> list[dict]:
    decisions = {
        skos.normalize_label(str(row.get("label", ""))): row
        for row in payload.get("class_decisions", [])
        if isinstance(row, dict)
    } if isinstance(payload.get("class_decisions"), list) else {}
    structured_roles = role_evidence.structured_value_roles(text)
    accepted: list[dict] = []
    seen: set[str] = set()
    rows = payload.get("replacement_classes", [])
    for row in rows if isinstance(rows, list) else []:
        if not isinstance(row, dict):
            continue
        source_norm = skos.normalize_label(str(row.get("from", "")))
        label = str(row.get("label", "")).strip()
        label_norm = skos.normalize_label(label)
        evidence = str(row.get("evidence", "")).strip()
        decision = decisions.get(source_norm)
        source_row = original_by_norm.get(source_norm)
        roles = structured_roles.get(role_evidence.normalize(label), set())
        exact_non_type = (
            role_evidence.ROLE_LITERAL in roles
            and role_evidence.ROLE_TYPE not in roles
        )
        independent_type_evidence = _has_independent_type_evidence(label, row)
        if (
            source_norm not in rejected_norms
            or not source_row
            or not decision
            or decision.get("keep") is not False
            or str(decision.get("role", "")).strip().casefold() != role_evidence.ROLE_INDIVIDUAL
            or not label_norm
            or label_norm in seen
            or label_norm == source_norm
            or not source_norm.endswith(" " + label_norm)
            or _confidence(row.get("confidence")) < settings.role_auto_accept_floor
            or not role_evidence.surface_is_grounded(text, label)
            or not role_evidence.evidence_is_grounded(text, evidence)
            or (exact_non_type and not independent_type_evidence)
        ):
            continue
        seen.add(label_norm)
        accepted.append({
            "label": label,
            "comment": "",
            "evidence": evidence,
            "_role_verified": True,
        })
    return accepted


async def _verify_class_denotations(
    text: str,
    ontology: dict,
    model: str | None,
    candidate_classes: list[dict] | None = None,
    eligible_full_norms: set[str] | None = None,
) -> dict:
    provisional_classes = [row for row in ontology.get("classes", []) if isinstance(row, dict)] \
        if isinstance(ontology.get("classes"), list) else []
    classes = candidate_classes if candidate_classes is not None else provisional_classes
    if not classes:
        return ontology
    if eligible_full_norms is None:
        eligible_full_norms = {
            skos.normalize_label(str(row.get("label") or row.get("name") or ""))
            for row in provisional_classes
        }
    candidates = {
        "classes": [
            {
                "label": str(row.get("label") or row.get("name") or ""),
                "comment": str(row.get("comment") or ""),
                "accepted_evidence": str(row.get("evidence") or ""),
                "provisionally_accepted": (
                    skos.normalize_label(str(row.get("label") or row.get("name") or ""))
                    in eligible_full_norms
                ),
            }
            for row in classes
        ]
    }
    user = (
        f"SOURCE TEXT:\n\"\"\"\n{text}\n\"\"\"\n\n"
        f"PROVISIONALLY ACCEPTED CLASSES:\n{json.dumps(candidates, ensure_ascii=False)}"
    )
    reply = await openrouter.chat(
        [
            {"role": "system", "content": prompt_config.get("tbox.denotation.critic")},
            {"role": "user", "content": user},
        ],
        model=model,
    )
    payload = openrouter.extract_json(reply)
    if not isinstance(payload, dict):
        raise openrouter.LLMError("TBox denotation critic did not return a JSON object")
    checked = _apply_tbox_role_decisions(
        text,
        {
            "classes": classes,
            "object_properties": [],
            "data_properties": [],
            "subclass_of": [],
            "disjoint_with": [],
            "equivalent_class": [],
        },
        payload,
    )
    accepted_classes = [
        row for row in checked.get("classes", [])
        if isinstance(row, dict)
        and skos.normalize_label(str(row.get("label") or row.get("name") or ""))
        in eligible_full_norms
    ]
    original_by_norm = {
        skos.normalize_label(str(row.get("label") or row.get("name") or "")): row
        for row in classes
    }
    accepted_norms = {
        skos.normalize_label(str(row.get("label") or row.get("name") or ""))
        for row in accepted_classes
    }
    rejected_norms = set(original_by_norm) - accepted_norms
    replacements = [
        row for row in _denotation_replacements(text, payload, original_by_norm, rejected_norms)
        if skos.normalize_label(str(row.get("label") or row.get("name") or ""))
        not in accepted_norms
    ]
    cleaned = _remove_rejected_class_references(ontology, rejected_norms)
    existing_recoveries = [
        row for row in ontology.get("_role_recoveries", [])
        if isinstance(row, dict)
        and skos.normalize_label(str(row.get("label", ""))) in accepted_norms
    ]
    return {
        **cleaned,
        "classes": [*accepted_classes, *replacements],
        "_role_rejections": [
            *[
                row for row in ontology.get("_role_rejections", [])
                if isinstance(row, dict)
            ],
            *[
                row for row in checked.get("_role_rejections", [])
                if isinstance(row, dict)
            ],
        ],
        "_role_recoveries": [
            *existing_recoveries,
            *[{"label": row["label"]} for row in replacements],
        ],
    }


async def _verify_tbox_candidates(text: str, ontology: dict, model: str | None) -> dict:
    """Independently classify class roles and verify subclass semantics, fail closed."""
    classes = [row for row in ontology.get("classes", []) if isinstance(row, dict)] \
        if isinstance(ontology.get("classes"), list) else []
    subclasses = [row for row in ontology.get("subclass_of", []) if isinstance(row, dict)] \
        if isinstance(ontology.get("subclass_of"), list) else []
    if not classes and not subclasses:
        return ontology
    candidates = {
        "classes": [
            {
                "label": str(row.get("label") or row.get("name") or ""),
                "comment": str(row.get("comment") or ""),
                "extractor_evidence": str(row.get("evidence") or ""),
            }
            for row in classes
        ],
        "subclass_of": [
            {
                "sub": _subclass_pair(row)[0],
                "super": _subclass_pair(row)[1],
                "extractor_evidence": str(row.get("evidence") or ""),
            }
            for row in subclasses
        ],
    }
    user = (
        f"SOURCE TEXT:\n\"\"\"\n{text}\n\"\"\"\n\n"
        f"UNTRUSTED CANDIDATES:\n{json.dumps(candidates, ensure_ascii=False)}"
    )
    reply = await openrouter.chat(
        [
            {"role": "system", "content": prompt_config.get("tbox.boundary.critic")},
            {"role": "user", "content": user},
        ],
        model=model,
    )
    payload = openrouter.extract_json(reply)
    if not isinstance(payload, dict):
        raise openrouter.LLMError("TBox role critic did not return a JSON object")
    verified = _apply_tbox_role_decisions(text, ontology, payload)

    accepted_norms = {
        skos.normalize_label(str(row.get("label") or row.get("name") or ""))
        for row in verified.get("classes", [])
        if isinstance(row, dict)
    }
    disputed_rows = [
        row for row in classes
        if skos.normalize_label(str(row.get("label") or row.get("name") or ""))
        not in accepted_norms
    ]
    if not disputed_rows:
        return await _verify_class_denotations(
            text, verified, model, candidate_classes=classes, eligible_full_norms=accepted_norms,
        )

    first_reasons = {
        skos.normalize_label(str(row.get("label", ""))): str(row.get("reason", ""))
        for row in verified.get("_role_rejections", [])
        if isinstance(row, dict)
    }
    disputed = {
        "classes": [
            {
                "label": str(row.get("label") or row.get("name") or ""),
                "comment": str(row.get("comment") or ""),
                "extractor_evidence": str(row.get("evidence") or ""),
                "first_critic_reason": first_reasons.get(
                    skos.normalize_label(str(row.get("label") or row.get("name") or "")), ""
                ),
            }
            for row in disputed_rows
        ]
    }
    adjudicator_user = (
        f"SOURCE TEXT:\n\"\"\"\n{text}\n\"\"\"\n\n"
        f"DISPUTED CLASS CANDIDATES:\n{json.dumps(disputed, ensure_ascii=False)}"
    )
    try:
        adjudicator_reply = await openrouter.chat(
            [
                {"role": "system", "content": prompt_config.get("tbox.boundary.adjudicator")},
                {"role": "user", "content": adjudicator_user},
            ],
            model=model,
        )
        adjudicator_payload = openrouter.extract_json(adjudicator_reply)
        if not isinstance(adjudicator_payload, dict):
            raise openrouter.LLMError("TBox boundary adjudicator did not return a JSON object")
        adjudicated = _apply_tbox_role_decisions(
            text,
            {
                "classes": disputed_rows,
                "object_properties": [],
                "data_properties": [],
                "subclass_of": [],
                "disjoint_with": [],
                "equivalent_class": [],
            },
            adjudicator_payload,
        )
    except Exception as exc:  # noqa: BLE001
        logger.warning("TBox boundary adjudication failed: %s", exc)
        return await _verify_class_denotations(
            text, verified, model, candidate_classes=classes, eligible_full_norms=accepted_norms,
        )

    recovered = [row for row in adjudicated.get("classes", []) if isinstance(row, dict)]
    denotation_checked = await _verify_class_denotations(
        text,
        {**verified, "_role_rejections": []},
        model,
        candidate_classes=[
            row for row in verified.get("classes", []) if isinstance(row, dict)
        ],
        eligible_full_norms=accepted_norms,
    )
    final_classes = [
        row for row in denotation_checked.get("classes", []) if isinstance(row, dict)
    ]
    final_norms = {
        skos.normalize_label(str(row.get("label") or row.get("name") or ""))
        for row in final_classes
    }
    for row in recovered:
        normalized = skos.normalize_label(str(row.get("label") or row.get("name") or ""))
        if normalized and normalized not in final_norms:
            final_norms.add(normalized)
            final_classes.append(row)
    return {
        **denotation_checked,
        "classes": final_classes,
        "_role_rejections": [
            *[
                row for row in adjudicated.get("_role_rejections", [])
                if isinstance(row, dict)
            ],
            *[
                row for row in denotation_checked.get("_role_rejections", [])
                if isinstance(row, dict)
            ],
        ],
        "_role_recoveries": [
            *[
                row for row in denotation_checked.get("_role_recoveries", [])
                if isinstance(row, dict)
            ],
            *[
                {"label": str(row.get("label") or row.get("name") or "")}
                for row in recovered
            ],
        ],
    }


async def _recover_rejected_classes(
    *,
    base_iri: str,
    graph_iri: str,
    chunks: list[tuple[int, str]],
    per_chunk: list[dict | None],
    model: str | None,
    terminology_aliases: dict[str, str] | None,
    structured_non_type_signals: dict[str, str],
    corpus_role_source_text: str,
) -> tuple[list[str], list[tuple[str, int]]]:
    """Recover reusable classes whose local critics lacked enough context.

    Only labels already proposed by the extractor are considered. A model first selects the most
    diagnostic passages from multiple occurrences, and all accepted decisions still pass exact
    evidence grounding plus the normal TBox sanitizer before entering the graph.
    """
    text_by_chunk = dict(chunks)

    existing = {
        skos.normalize_label(row["label"])
        for row in schema.build_view(graph_iri)["classes"]
    }
    candidates: dict[str, dict] = {}
    for entry in per_chunk:
        if not entry:
            continue
        chunk_id = entry.get("chunk_id")
        source_text = text_by_chunk.get(chunk_id, "")
        if not source_text:
            continue
        for row in entry.get("rejected_tbox_individuals", []):
            if not isinstance(row, dict):
                continue
            label = str(row.get("label", "")).strip()
            normalized = skos.normalize_label(label)
            if (
                not normalized
                or normalized in existing
                or tbox_guard.canonical_datatype_name(label) is not None
                or not role_evidence.surface_is_grounded(source_text, label)
            ):
                continue
            candidate = candidates.setdefault(normalized, {"label": label, "occurrences": []})
            occurrences = candidate["occurrences"]
            if any(item["chunk_id"] == chunk_id for item in occurrences):
                continue
            occurrences.append({
                "chunk_id": chunk_id,
                "text": source_text,
                "extractor_evidence": str(row.get("evidence", "")),
                "earlier_reason": str(row.get("reason", "")),
            })
    if not candidates:
        return [], []

    candidate_rows = list(candidates.items())
    batch_size = 8
    batches = [candidate_rows[index:index + batch_size] for index in range(0, len(candidate_rows), batch_size)]
    semaphore = asyncio.Semaphore(model_config.llm_concurrency())

    async def decide(batch: list[tuple[str, dict]]) -> list[dict]:
        batch_candidates = dict(batch)
        prepared = _prepare_corpus_evidence(batch_candidates)
        fallback = _apply_corpus_evidence_selections(prepared, {})
        async with semaphore:
            async with openrouter.capacity_slot():
                selector_user = "CANDIDATES AND NUMBERED SOURCE PASSAGES:\n" + json.dumps(
                    list(prepared.values()), ensure_ascii=False
                )
                selected = fallback
                try:
                    async with asyncio.timeout(settings.llm_timeout_s * 2):
                        selector_reply = await openrouter.chat(
                            [
                                {
                                    "role": "system",
                                    "content": prompt_config.get("tbox.boundary.evidence_selector"),
                                },
                                {"role": "user", "content": selector_user},
                            ],
                            model=model,
                        )
                    selector_payload = openrouter.extract_json(selector_reply)
                    if not isinstance(selector_payload, dict):
                        raise openrouter.LLMError("Corpus evidence selector did not return a JSON object")
                    selected = _apply_corpus_evidence_selections(prepared, selector_payload)
                except Exception as exc:  # noqa: BLE001
                    logger.warning("corpus evidence selection failed; using diverse passages: %s", exc)

                input_rows = [
                    {
                        "label": candidate["label"],
                        "source_passages": [
                            {
                                "text": passage["text"],
                                "earlier_reason": passage["earlier_reason"],
                                "extractor_evidence": passage["extractor_evidence"],
                            }
                            for passage in selected.get(normalized, [])
                        ],
                    }
                    for normalized, candidate in batch
                ]
                user = "REJECTED CLASS CANDIDATES WITH SELECTED CORPUS EVIDENCE:\n" + json.dumps(
                    input_rows, ensure_ascii=False
                )
                async with asyncio.timeout(settings.llm_timeout_s * 3):
                    reply = await openrouter.chat(
                        [
                            {"role": "system", "content": prompt_config.get("tbox.boundary.corpus_recovery")},
                            {"role": "user", "content": user},
                        ],
                        model=model,
                    )
        payload = openrouter.extract_json(reply)
        if not isinstance(payload, dict):
            raise openrouter.LLMError("Corpus role recovery did not return a JSON object")
        initially_accepted = _apply_corpus_role_decisions(
            batch_candidates, payload, set(structured_non_type_signals),
        )
        return initially_accepted

    recovered: list[dict] = []
    results = await asyncio.gather(*(decide(batch) for batch in batches), return_exceptions=True)
    for result in results:
        if isinstance(result, Exception):
            logger.warning("corpus role recovery failed for one batch: %s", result)
        else:
            recovered.extend(result)

    labels: list[str] = []
    provenance: list[tuple[str, int]] = []
    for row in recovered:
        keys, _ = await asyncio.to_thread(
            _merge_into_graph,
            base_iri,
            graph_iri,
            {
                "classes": [{
                    "label": row["label"],
                    "comment": row["comment"],
                    "evidence": row["evidence"],
                    "_role_verified": row.get("_role_verified") is True,
                }],
                "object_properties": [],
                "data_properties": [],
                "subclass_of": [],
                "disjoint_with": [],
                "equivalent_class": [],
            },
            row["source_text"],
            terminology_aliases,
            structured_non_type_signals,
            corpus_role_source_text,
        )
        if keys:
            labels.append(row["label"])
            if row.get("chunk_id") is not None:
                provenance.extend((key, row["chunk_id"]) for key in keys)
    return labels, provenance


async def _verify_subclass_candidates(
    text: str,
    proposed: list[dict],
    model: str | None,
    allowed_norms: set[str],
) -> list[dict]:
    if not proposed:
        return []
    user = (
        f"SOURCE TEXT:\n\"\"\"\n{text}\n\"\"\"\n\n"
        f"PROPOSED SUBCLASS EDGES:\n{json.dumps(proposed, ensure_ascii=False)}"
    )
    reply = await openrouter.chat(
        [
            {"role": "system", "content": prompt_config.get("tbox.hierarchy.critic")},
            {"role": "user", "content": user},
        ],
        model=model,
    )
    payload = openrouter.extract_json(reply)
    if not isinstance(payload, dict):
        raise openrouter.LLMError("Subclass critic did not return a JSON object")
    return _apply_subclass_decisions(text, proposed, payload, allowed_norms)


async def _recover_hierarchy_one(
    text: str,
    model: str | None,
    allowed_labels: list[str],
) -> dict:
    """Recover explicit parents/edges, then send both through independent boundary critics."""
    if not allowed_labels:
        return {"classes": [], "subclass_of": []}
    user = (
        f"SOURCE TEXT:\n\"\"\"\n{text}\n\"\"\"\n\n"
        f"EXISTING CLASSES:\n{json.dumps(allowed_labels, ensure_ascii=False)}"
    )
    reply = await openrouter.chat(
        [
            {"role": "system", "content": prompt_config.get("tbox.hierarchy.recovery")},
            {"role": "user", "content": user},
        ],
        model=model,
    )
    payload = openrouter.extract_json(reply)
    if not isinstance(payload, dict):
        raise openrouter.LLMError("Hierarchy recovery did not return a JSON object")

    canonical = {skos.normalize_label(label): label for label in allowed_labels}
    proposed_classes: list[dict] = []
    new_canonical: dict[str, str] = {}
    rows = payload.get("classes", [])
    for row in rows if isinstance(rows, list) else []:
        if not isinstance(row, dict):
            continue
        label = str(row.get("label") or row.get("name") or "").strip()
        normalized = skos.normalize_label(label)
        evidence = str(row.get("evidence", "")).strip()
        if (
            not normalized
            or normalized in canonical
            or normalized in new_canonical
            or not role_evidence.surface_is_grounded(text, label)
            or not role_evidence.evidence_is_grounded(text, evidence)
        ):
            continue
        new_canonical[normalized] = label
        proposed_classes.append({"label": label, "comment": "", "evidence": evidence})

    proposed: list[dict] = []
    rows = payload.get("subclass_of", [])
    for row in rows if isinstance(rows, list) else []:
        if not isinstance(row, dict):
            continue
        sub = canonical.get(skos.normalize_label(str(row.get("sub", ""))))
        super_norm = skos.normalize_label(str(row.get("super", "")))
        super_ = canonical.get(super_norm) or new_canonical.get(super_norm)
        evidence = str(row.get("evidence", "")).strip()
        if (
            not sub
            or not super_
            or sub == super_
            or not role_evidence.evidence_is_grounded(text, evidence)
        ):
            continue
        proposed.append({"sub": sub, "super": super_, "evidence": evidence})
    if not proposed:
        return {"classes": [], "subclass_of": []}

    used_new_norms = {
        skos.normalize_label(row["super"])
        for row in proposed
        if skos.normalize_label(row["super"]) in new_canonical
    }
    proposed_classes = [
        row for row in proposed_classes
        if skos.normalize_label(row["label"]) in used_new_norms
    ]
    accepted_classes: list[dict] = []
    if proposed_classes:
        verified_new = await _verify_tbox_candidates(
            text,
            {
                "classes": proposed_classes,
                "object_properties": [],
                "data_properties": [],
                "subclass_of": [],
                "disjoint_with": [],
                "equivalent_class": [],
            },
            model,
        )
        accepted_classes = [
            row for row in verified_new.get("classes", []) if isinstance(row, dict)
        ] if isinstance(verified_new.get("classes"), list) else []
    accepted_new_norms = {
        skos.normalize_label(str(row.get("label") or row.get("name") or ""))
        for row in accepted_classes
    }
    allowed_norms = set(canonical) | accepted_new_norms
    admissible_edges = [
        row for row in proposed
        if all(
            skos.normalize_label(value) in allowed_norms
            for value in _subclass_pair(row)
        )
    ]
    accepted_edges = await _verify_subclass_candidates(
        text, admissible_edges, model, allowed_norms,
    )
    used_accepted_new_norms = {
        skos.normalize_label(_subclass_pair(row)[1])
        for row in accepted_edges
        if skos.normalize_label(_subclass_pair(row)[1]) in used_new_norms
    }
    accepted_classes = [
        row for row in accepted_classes
        if skos.normalize_label(str(row.get("label") or row.get("name") or ""))
        in used_accepted_new_norms
    ]
    return {"classes": accepted_classes, "subclass_of": accepted_edges}


def _merge_into_graph(
    base_iri: str, graph_iri: str, onto: dict, source_text: str,
    terminology_aliases: dict[str, str] | None = None,
    structured_non_type_signals: dict[str, str] | None = None,
    corpus_role_source_text: str | None = None,
) -> tuple[list[str], list[dict[str, str]]]:
    """Merge an extracted delta into the graph under a lock (serializes concurrent
    workers' read-modify-write). Returns added axiom keys and rejected TBox candidates."""
    with _write_lock:
        role_rejections = list(onto.pop("_role_rejections", []))
        onto = skos.normalize_ontology_delta(onto, terminology_aliases or {})
        index = schema.read_index(graph_iri)
        onto, rejected = tbox_guard.sanitize_ontology_delta(
            onto, source_text, existing_class_norms=set(index.class_by_norm),
            structured_non_type_signals=structured_non_type_signals,
            corpus_role_source_text=corpus_role_source_text,
            existing_object_property_norms=index.object_property_norms,
            existing_data_property_norms=index.data_property_norms,
        )
        mut = schema.build_mutation(base_iri, onto, index)
        store.add_triples(graph_iri, mut.triples)
        return mut.axiom_keys, role_rejections + rejected


async def extract_tbox_from_chunks(
    *,
    base_iri: str,
    graph_iri: str,
    chunks: list[tuple[int, str]],  # (chunk_id, text)
    model: str | None = None,
    progress: ProgressCb = None,
    terminology_aliases: dict[str, str] | None = None,
) -> dict:
    """Run extraction over chunks concurrently (capped by the selected LLM endpoint), merging
    into the graph. The LLM/agent calls overlap; graph writes are serialized by a lock, so
    there are no write/write races. Same-label concepts merge (IRI derives from the label);
    residual cross-chunk near-duplicates are caught by conflict detection afterward.
    """
    before = schema.graph_stats(graph_iri)
    use_agentic = _should_use_agentic(graph_iri)  # decided once per job
    total = len(chunks)
    corpus_role_source_text = "\n\n".join(text for _, text in chunks)
    structured_roles: dict[str, set[str]] = {}
    for _, text in chunks:
        for normalized, roles in role_evidence.structured_value_roles(text).items():
            structured_roles.setdefault(normalized, set()).update(roles)
    structured_non_type_signals = {
        normalized: "structured scalar value without an explicit type declaration"
        for normalized, roles in structured_roles.items()
        if role_evidence.ROLE_LITERAL in roles and role_evidence.ROLE_TYPE not in roles
    }

    provenance: list[tuple[str, int]] = []
    per_chunk: list[dict | None] = [None] * total
    log_lines: list[str | None] = [None] * total
    sem = asyncio.Semaphore(model_config.llm_concurrency())
    completed = 0

    async def worker(i: int, chunk_id: int, text: str) -> None:
        nonlocal completed
        entry = {"chunk_id": chunk_id, "status": "ok", "axioms": 0, "error": None}
        try:
            async with sem:
                async with openrouter.capacity_slot():
                    async with asyncio.timeout(settings.llm_timeout_s * 3):
                        onto = await _extract_for_chunk(text, model, graph_iri, use_agentic)
                        onto = await _verify_tbox_candidates(text, onto, model)
            recovered_classes = list(onto.pop("_role_recoveries", []))
            keys, rejected = await asyncio.to_thread(
                _merge_into_graph, base_iri, graph_iri, onto, text, terminology_aliases,
                structured_non_type_signals, corpus_role_source_text,
            )
            provenance.extend((k, chunk_id) for k in keys)
            entry["axioms"] = len(keys)
            entry["rejected_tbox_individuals"] = rejected
            entry["recovered_tbox_classes"] = recovered_classes
            rejected_labels = ", ".join(item["label"] for item in rejected)
            rejected_note = f", rejected invalid TBox candidate(s): {rejected_labels}" if rejected else ""
            recovered_labels = ", ".join(item["label"] for item in recovered_classes)
            recovered_note = f", recovered disputed class(es): {recovered_labels}" if recovered_labels else ""
            log_lines[i] = f"chunk {chunk_id}: +{len(keys)} axioms{recovered_note}{rejected_note}"
        except Exception as e:  # noqa: BLE001  (one bad chunk shouldn't kill the job)
            entry["status"] = "error"
            entry["error"] = str(e)
            log_lines[i] = f"chunk {chunk_id}: ERROR {e}"
            logger.warning("extraction failed on chunk %s: %s", chunk_id, e)
        per_chunk[i] = entry
        completed += 1
        if progress:
            await progress({
                "type": "chunk", "index": completed - 1, "total": total,
                "chunk_id": chunk_id, "result": entry,
            })

    await asyncio.gather(*(worker(i, cid, txt) for i, (cid, txt) in enumerate(chunks)))

    if progress:
        await progress({"type": "role_recovery", "index": 0, "total": total})
    corpus_recovered, corpus_provenance = await _recover_rejected_classes(
        base_iri=base_iri,
        graph_iri=graph_iri,
        chunks=chunks,
        per_chunk=per_chunk,
        model=model,
        terminology_aliases=terminology_aliases,
        structured_non_type_signals=structured_non_type_signals,
        corpus_role_source_text=corpus_role_source_text,
    )
    provenance.extend(corpus_provenance)

    # A dedicated second pass repairs a systematic weakness of general TBox extraction: models
    # often identify both classes but omit the explicit is-a edge between them. It sees the final
    # merged class vocabulary, proposes only source-grounded edges, and sends every proposal through
    # the independent subclass critic before writing it. This is semantic evidence, not a
    # domain-specific allowlist or a lexical-suffix shortcut.
    class_labels = [item["label"] for item in schema.build_view(graph_iri)["classes"]]
    hierarchy_completed = 0

    async def recover_worker(i: int, chunk_id: int, text: str) -> None:
        nonlocal hierarchy_completed
        try:
            grounded = [
                label for label in class_labels
                if role_evidence.surface_is_grounded(text, label)
            ][:400]
            if not grounded:
                return
            async with sem:
                async with openrouter.capacity_slot():
                    async with asyncio.timeout(settings.llm_timeout_s * 3):
                        recovered = await _recover_hierarchy_one(text, model, grounded)
            rows = recovered.get("subclass_of", [])
            if not rows:
                return
            keys, _ = await asyncio.to_thread(
                _merge_into_graph,
                base_iri,
                graph_iri,
                {
                    "classes": recovered.get("classes", []),
                    "object_properties": [],
                    "data_properties": [],
                    "subclass_of": rows,
                    "disjoint_with": [],
                    "equivalent_class": [],
                },
                text,
                terminology_aliases,
                structured_non_type_signals,
                corpus_role_source_text,
            )
            edge_keys = [key for key in keys if key.startswith("subClassOf|")]
            if not edge_keys:
                return
            hierarchy_keys = [
                key for key in keys
                if key.startswith("class|") or key.startswith("subClassOf|")
            ]
            provenance.extend((key, chunk_id) for key in hierarchy_keys)
            entry = per_chunk[i]
            if entry is not None:
                entry["hierarchy_axioms"] = len(edge_keys)
                entry["hierarchy_classes"] = len(recovered.get("classes", []))
            new_class_count = len(recovered.get("classes", []))
            class_note = f" and {new_class_count} missing superclass(es)" if new_class_count else ""
            suffix = f", recovered {len(edge_keys)} hierarchy edge(s){class_note}"
            log_lines[i] = (log_lines[i] or f"chunk {chunk_id}: +0 axioms") + suffix
        except Exception as e:  # noqa: BLE001
            logger.warning("hierarchy recovery failed on chunk %s: %s", chunk_id, e)
        finally:
            hierarchy_completed += 1
            if progress:
                await progress({
                    "type": "hierarchy", "index": hierarchy_completed - 1, "total": total,
                    "chunk_id": chunk_id,
                })

    await asyncio.gather(*(recover_worker(i, cid, txt) for i, (cid, txt) in enumerate(chunks)))

    per_chunk_clean = [e for e in per_chunk if e is not None]
    log_clean = "\n".join(line for line in log_lines if line)
    if corpus_recovered:
        log_clean = (
            log_clean
            + "\ncorpus role recovery: "
            + ", ".join(corpus_recovered)
        ).strip()
    after = schema.graph_stats(graph_iri)
    return {
        "classes_added": after["class_count"] - before["class_count"],
        "properties_added": after["property_count"] - before["property_count"],
        "axioms_added": after["axiom_count"] - before["axiom_count"],
        "stats_after": after,
        "per_chunk": per_chunk_clean,
        "rejected_tbox_individuals": sum(
            len(entry.get("rejected_tbox_individuals", [])) for entry in per_chunk_clean
        ),
        "corpus_recovered_classes": corpus_recovered,
        "provenance": provenance,
        "log": log_clean,
    }
