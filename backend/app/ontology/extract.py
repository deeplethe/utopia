"""LLM-driven TBox extraction.

Each selected chunk is sent to a cheap DeepSeek model with a strict-JSON ontology
schema. The returned classes/properties/axioms are merged into the knowledge system's
named graph, and every axiom is linked back to its source chunk (provenance).
"""
from __future__ import annotations

import asyncio
import json
import logging
import threading
from collections.abc import Awaitable, Callable

from app.config import settings
from app.llm import openrouter
from app.ontology import retrieval, schema, store

logger = logging.getLogger(__name__)

ProgressCb = Callable[[dict], Awaitable[None]] | None

# Serializes graph writes across concurrent extraction workers (they run in threads).
_write_lock = threading.Lock()

_SYSTEM_PROMPT = """You are an ontology engineer. From the given text, extract a lightweight OWL \
TBox (a schema-level ontology of general concepts and their relations) — NOT specific \
instances/individuals (no ABox).

Return ONLY a single JSON object with exactly these keys (use [] when empty):
{
  "classes": [{"label": "<natural-language singular noun, e.g. 'Pump Station'>", "comment": "<short gloss>"}],
  "object_properties": [{"label": "<verb phrase, e.g. 'has component'>", "domain": "<class label>", "range": "<class label>", "comment": ""}],
  "data_properties": [{"label": "<attribute, e.g. 'nominal pressure'>", "domain": "<class label>", "range": "string|integer|decimal|boolean|date|dateTime", "comment": ""}],
  "subclass_of": [{"sub": "<child class label>", "super": "<parent class label>"}],
  "disjoint_with": [{"a": "<class label>", "b": "<class label>"}],
  "equivalent_class": [{"a": "<class label>", "b": "<class label>"}]
}

Rules:
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
- For an object property, prefer a GENERAL relation and let the RANGE class carry the
  specificity — do NOT bake the object's noun into the property label. Use "拥有" (range: 油井),
  NOT "拥有井"; "has component" (range: Pump), NOT "has pump". Only use a distinct property when
  the relation's MEANING differs (e.g. owns vs operates), not merely because the object type differs.
- Only assert disjoint_with / equivalent_class when the text clearly implies it.
- If an EXISTING ONTOLOGY is provided, REUSE its exact class/property labels for any
  concept it already covers (do NOT invent near-duplicate names); introduce new labels
  only for genuinely new concepts, and attach new classes under existing ones with
  subclass_of where the text supports it.
- If the text has no ontological content, return all empty arrays.
- Output must be valid JSON with no surrounding prose."""

_MAX_CLASSES_CTX = 400
_MAX_PROPS_CTX = 200


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
        {"role": "system", "content": _SYSTEM_PROMPT},
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
  "classes": [{"label": "...", "comment": "..."}],
  "object_properties": [{"label": "...", "domain": "<class label>", "range": "<class label>", "comment": ""}],
  "data_properties": [{"label": "...", "domain": "<class label>", "range": "string|integer|decimal|boolean|date|dateTime", "comment": ""}],
  "subclass_of": [{"sub": "...", "super": "..."}],
  "disjoint_with": [{"a": "...", "b": "..."}],
  "equivalent_class": [{"a": "...", "b": "..."}]
}

Rules:
- Write every label and comment in the SAME language as the source text (Chinese text →
  Chinese labels). Do not translate.
- First search the ontology for the key concepts in the text. REUSE the exact existing
  labels for concepts already present; introduce new labels only for genuinely new concepts;
  attach new classes under an existing class with subclass_of where the text supports it.
- Keep tool use minimal (a few searches); when confident, finish.
- For an object property, prefer a GENERAL relation and let the RANGE carry the specificity —
  do NOT bake the object's noun into the label ("拥有" range 油井, NOT "拥有井"). Only use a
  distinct property when the relation's MEANING differs, not merely the object type.
- Extract only what the text supports; no ABox individuals.
- Output MUST be a single valid JSON object with no surrounding prose."""


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
        {"role": "system", "content": _AGENT_SYSTEM_PROMPT},
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


def _merge_into_graph(base_iri: str, graph_iri: str, onto: dict) -> list[str]:
    """Merge an extracted delta into the graph under a lock (serializes concurrent
    workers' read-modify-write). Returns the axiom keys added."""
    with _write_lock:
        mut = schema.build_mutation(base_iri, onto, schema.read_index(graph_iri))
        store.add_triples(graph_iri, mut.triples)
        return mut.axiom_keys


async def extract_tbox_from_chunks(
    *,
    base_iri: str,
    graph_iri: str,
    chunks: list[tuple[int, str]],  # (chunk_id, text)
    model: str | None = None,
    progress: ProgressCb = None,
) -> dict:
    """Run extraction over chunks CONCURRENTLY (capped by extraction_concurrency), merging
    into the graph. The LLM/agent calls overlap; graph writes are serialized by a lock, so
    there are no write/write races. Same-label concepts merge (IRI derives from the label);
    residual cross-chunk near-duplicates are caught by conflict detection afterward.
    """
    before = schema.graph_stats(graph_iri)
    use_agentic = _should_use_agentic(graph_iri)  # decided once per job
    total = len(chunks)

    provenance: list[tuple[str, int]] = []
    per_chunk: list[dict | None] = [None] * total
    log_lines: list[str | None] = [None] * total
    sem = asyncio.Semaphore(max(1, settings.extraction_concurrency))
    completed = 0

    async def worker(i: int, chunk_id: int, text: str) -> None:
        nonlocal completed
        entry = {"chunk_id": chunk_id, "status": "ok", "axioms": 0, "error": None}
        try:
            async with sem:
                onto = await _extract_for_chunk(text, model, graph_iri, use_agentic)
            keys = await asyncio.to_thread(_merge_into_graph, base_iri, graph_iri, onto)
            provenance.extend((k, chunk_id) for k in keys)
            entry["axioms"] = len(keys)
            log_lines[i] = f"chunk {chunk_id}: +{len(keys)} axioms"
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

    per_chunk_clean = [e for e in per_chunk if e is not None]
    log_clean = "\n".join(line for line in log_lines if line)
    after = schema.graph_stats(graph_iri)
    return {
        "classes_added": after["class_count"] - before["class_count"],
        "properties_added": after["property_count"] - before["property_count"],
        "axioms_added": after["axiom_count"] - before["axiom_count"],
        "stats_after": after,
        "per_chunk": per_chunk_clean,
        "provenance": provenance,
        "log": log_clean,
    }
