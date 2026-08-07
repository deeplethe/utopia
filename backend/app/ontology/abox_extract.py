"""LLM-driven ABox (instance) extraction, guided by the KS's existing TBox.

Each chunk is sent to a cheap DeepSeek model with the ontology's class/property vocabulary;
the model returns specific individuals typed by those classes, with attribute/relationship
assertions using those properties. Every extracted mention is then run through entity
resolution (``resolution.resolve_mention``) so the same real-world entity mentioned across
chunks/documents collapses to one individual, ambiguous cases go to the manual queue, and
decisions accumulate as a learned lookup.

Chunks are processed SEQUENTIALLY (not concurrently like TBox extraction): resolution is
stateful — a chunk must see the individuals created by earlier chunks to merge correctly.
"""
from __future__ import annotations

import asyncio
import logging
from collections.abc import Awaitable, Callable

from sqlmodel import Session

from app.config import settings
from app.db.database import engine
from app.llm import openrouter
from app.ontology import abox, abox_provenance, resolution, schema

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
    for a in _as_dicts(m.get("attributes")):
        p, v = str(a.get("property", "")).strip(), a.get("value")
        if p and v not in (None, ""):
            parts.append(f"{p}={v}")
    for r in _as_dicts(m.get("relations")):
        p, t = str(r.get("property", "")).strip(), str(r.get("target", "")).strip()
        if p and t:
            parts.append(f"{p}→{t}")
    return "; ".join(parts)


def _pending_payload(m: dict, prop_index: dict[str, tuple[str, str]]) -> dict | None:
    """Resolve a mention's attributes/relations to property IRIs and stash them, so if the
    mention lands in the manual queue its facts aren't lost — they're replayed onto the
    individual when a human resolves it (relations keep the target *label*, resolved at replay)."""
    attrs, rels = [], []
    for a in _as_dicts(m.get("attributes")):
        pi = prop_index.get(str(a.get("property", "")).strip().lower())
        val = a.get("value")
        if pi and pi[1] == "data" and val not in (None, "") and str(val).strip():
            attrs.append({"prop": pi[0], "value": str(val).strip()})
    for r in _as_dicts(m.get("relations")):
        pi = prop_index.get(str(r.get("property", "")).strip().lower())
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

_SYSTEM_PROMPT = """You extract ABox individuals — SPECIFIC, NAMED INSTANCES (a particular \
well, station, person, device, event, record) — from text, typed by an EXISTING ontology's \
classes, with attribute and relationship assertions that use the ontology's EXISTING \
properties.

Return ONLY a single JSON object:
{
  "individuals": [
    {
      "label": "<the specific name/identifier as it appears in the text, e.g. 'Well No. 3'>",
      "class": "<exactly one of the EXISTING class labels listed below>",
      "attributes": [{"property": "<existing data-property label>", "value": "<literal value>"}],
      "relations": [{"property": "<existing object-property label>", "target": "<label of another individual in this list>"}]
    }
  ]
}

Rules:
- Extract ONLY specific individuals — NOT general concepts/classes. "Pump" (a kind) is NOT an
  individual; "Pump #7 at Station A" IS. A specific individual has a distinguishing identifier:
  a proper name, a number, a code, a date, or a location (e.g. "Well No. 3", "2024 annual
  inspection", "Beihai Station"). A bare generic noun is a class, not an individual.
- CRITICAL: an individual's "label" must NEVER be identical (or near-identical) to a class
  name — if the only name you have for something is a class name (e.g. label "Pump" typed as
  class "Pump"), that IS the class, so DROP it. Do not make a class an instance of itself.
- Type each individual with the single best-matching EXISTING class label. If none fits, omit it.
- Do NOT extract vague descriptors, spatial phrases, or activity/task descriptions as
  individuals (e.g. "east side of the plant", "this block's water-injection task" are
  descriptions, not named individuals). Only extract things with a real, distinct identity.
- Use ONLY the existing property labels below for attributes/relations; DROP any assertion
  whose property is not in the ontology.
- For a data property whose type is numeric (integer/decimal), put ONLY the number in "value"
  (e.g. "37", not "37 kW"; "2000", not "2000 tons") — the unit is implied by the property.
  Keep the unit only when the property's type is a string.
- A relation's "target" must be the label of another individual you list.
- Keep labels and values in the SAME language as the source text. Do not translate.
- If the text contains no specific instances, return {"individuals": []}.
- Output must be valid JSON with no surrounding prose."""

_MAX_CLASSES = 400
_MAX_PROPS = 200


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


def _indexes(view: dict) -> tuple[dict[str, str], dict[str, tuple[str, str]]]:
    """(class label.lower -> iri, property label.lower -> (iri, 'object'|'data'))."""
    class_index = {c["label"].strip().lower(): c["iri"] for c in view["classes"]}
    prop_index: dict[str, tuple[str, str]] = {}
    for p in view["object_properties"]:
        prop_index[p["label"].strip().lower()] = (p["iri"], "object")
    for p in view["data_properties"]:
        prop_index[p["label"].strip().lower()] = (p["iri"], "data")
    return class_index, prop_index


def _format_tbox(view: dict) -> str:
    classes = [c["label"] for c in view["classes"]][:_MAX_CLASSES]
    lines = ["EXISTING CLASSES (type each individual with exactly one of these):",
             "、".join(classes) or "(none)"]
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
        [{"role": "system", "content": _SYSTEM_PROMPT}, {"role": "user", "content": user}],
        model=model,
    )
    data = openrouter.extract_json(reply)
    if not isinstance(data, dict):
        raise openrouter.LLMError("LLM did not return a JSON object")
    inds = data.get("individuals")
    return inds if isinstance(inds, list) else []


def _resolve_and_merge_chunk(
    ks_id: int, abox_iri: str, base_iri: str, chunk_id: int,
    class_index: dict[str, str], prop_index: dict[str, tuple[str, str]],
    class_labels: dict[str, str], prop_labels: dict[str, str], hierarchy: dict[str, set[str]],
    res_index: "abox.ResolutionIndex", model: str | None, mentions: list[dict],
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

    with Session(engine) as session:
        # A multi-step agent decides ambiguous candidate matches (embeddings only retrieve).
        agent = None
        if settings.agentic_resolution:
            def _details(iri: str):
                return _compact_details(abox.get_individual(abox_iri, iri, class_labels, prop_labels))

            def _agent(surface: str, class_label: str, evidence: str, candidates):
                return resolution.agentic_resolve(
                    session=session, ks_id=ks_id, surface=surface, class_label=class_label,
                    evidence=evidence, candidates=candidates, details_fn=_details, model=model,
                    max_steps=settings.resolution_max_steps,
                )
            agent = _agent

        seen: set[tuple[str, str]] = set()  # (surface, class) already handled this chunk
        prov: list[str] = []  # provenance fact keys produced by THIS chunk
        for m in mentions:
            surface = str(m.get("label", "")).strip()
            cls_label = str(m.get("class", "")).strip()
            if not surface or not cls_label:
                continue
            cls_iri = class_index.get(cls_label.lower())
            if not cls_iri:
                unknown.append(cls_label)  # surface as a "suggested class", don't silently drop
                continue
            # De-dup within the chunk: the same (surface, class) mentioned twice is one entity —
            # resolve/count it once (its assertions still merge idempotently in the loop below).
            key = (surface.lower(), cls_iri)
            if key in seen:
                continue
            seen.add(key)
            # Guard: a "mention" whose label is itself a class name is the class, not an
            # instance (the LLM over-extracting a generic concept). Never make a class its own
            # instance — drop it rather than mint a spurious class-named individual.
            if surface.lower() in class_index:
                counts["rejected"] += 1
                continue
            iri, status = resolution.resolve_mention(
                session, ks_id=ks_id, abox_iri=abox_iri, base_iri=base_iri,
                surface=surface, class_iri=cls_iri, chunk_id=chunk_id,
                class_label=cls_label, evidence=_evidence(m), agent=agent,
                pending_payload=_pending_payload(m, prop_index),
                related_classes=hierarchy.get(cls_iri, {cls_iri}),
                index=res_index,
            )
            if status == "new":
                counts["created"] += 1
            elif status == "matched":
                counts["matched"] += 1
            elif status == "pending":
                counts["queued"] += 1
            if iri:
                local[(surface.lower(), cls_iri)] = iri
                by_label.setdefault(surface.lower(), set()).add(iri)
                prov.append(abox_provenance.ind_key(iri))  # this chunk mentioned this individual

        for m in mentions:
            m_cls = class_index.get(str(m.get("class", "")).strip().lower())
            subj = local.get((str(m.get("label", "")).strip().lower(), m_cls)) if m_cls else None
            if not subj:
                continue  # subject unresolved (queued/skipped) → drop its assertions
            for a in _as_dicts(m.get("attributes")):
                pi = prop_index.get(str(a.get("property", "")).strip().lower())
                val = a.get("value")
                if pi and pi[1] == "data" and val is not None and str(val).strip():
                    v = str(val).strip()
                    if abox.add_data_assertion(abox_iri, subj, pi[0], v):
                        counts["assertions"] += 1  # count only triples actually added (idempotent re-runs)
                    prov.append(abox_provenance.data_key(subj, pi[0], v))  # this chunk asserted this value
            for r in _as_dicts(m.get("relations")):
                pi = prop_index.get(str(r.get("property", "")).strip().lower())
                # A bare target label carries no class; only link when it maps to exactly one
                # individual — otherwise it's ambiguous and mis-routing would corrupt the graph.
                tgts = by_label.get(str(r.get("target", "")).strip().lower())
                tgt = next(iter(tgts)) if tgts and len(tgts) == 1 else None
                if pi and pi[1] == "object" and tgt:
                    if abox.add_object_assertion(abox_iri, subj, pi[0], tgt):
                        counts["assertions"] += 1
                    prov.append(abox_provenance.obj_key(subj, pi[0], tgt))  # this chunk asserted this relation

        abox_provenance.rebuild_for_chunk(session, ks_id, chunk_id, prov)
        session.commit()
    counts["unknown"] = unknown
    return counts


async def extract_instances_from_chunks(
    *,
    base_iri: str,
    graph_iri: str,
    abox_iri: str,
    ks_id: int,
    chunks: list[tuple[int, str]],
    model: str | None = None,
    progress: ProgressCb = None,
) -> dict:
    """Extract individuals + assertions from chunks (sequential), resolving each mention."""
    view = schema.build_view(graph_iri)
    if not view["classes"]:
        return {"created": 0, "matched": 0, "queued": 0, "assertions": 0,
                "unknown_classes": {}, "per_chunk": [], "log": "No classes in the ontology — extract a TBox first."}

    tbox_ctx = _format_tbox(view)
    class_index, prop_index = _indexes(view)
    hierarchy = _class_hierarchy(view)
    class_labels = {c["iri"]: c["label"] for c in view["classes"]}
    prop_labels = {p["iri"]: p["label"] for p in view["object_properties"] + view["data_properties"]}
    # One ABox scan up front; resolution keeps it in sync as it mints individuals, so each mention
    # is index lookups instead of ~3 full ABox scans (chunks run sequentially -> single-threaded).
    res_index = abox.build_resolution_index(abox_iri)
    totals = {"created": 0, "matched": 0, "queued": 0, "assertions": 0, "rejected": 0}
    unknown_classes: dict[str, int] = {}  # suggested class label -> times referenced
    per_chunk: list[dict] = []
    log_lines: list[str] = []
    total = len(chunks)

    for i, (chunk_id, text) in enumerate(chunks):
        entry = {"chunk_id": chunk_id, "status": "ok", "created": 0, "matched": 0,
                 "queued": 0, "assertions": 0, "rejected": 0, "error": None}
        try:
            mentions = await _extract_one(text, model, tbox_ctx)
            res = await asyncio.to_thread(
                _resolve_and_merge_chunk, ks_id, abox_iri, base_iri, chunk_id,
                class_index, prop_index, class_labels, prop_labels, hierarchy,
                res_index, model, mentions,
            )
            for lbl in res.pop("unknown", []):
                unknown_classes[lbl] = unknown_classes.get(lbl, 0) + 1
            entry.update(res)
            for k in totals:
                totals[k] += res[k]
            log_lines.append(
                f"chunk {chunk_id}: +{res['created']} new / {res['matched']} linked / "
                f"{res['queued']} queued / {res['rejected']} rejected / {res['assertions']} assertions"
            )
        except Exception as e:  # noqa: BLE001  (one bad chunk shouldn't kill the job)
            entry["status"] = "error"
            entry["error"] = str(e)
            log_lines.append(f"chunk {chunk_id}: ERROR {e}")
            logger.warning("abox extraction failed on chunk %s: %s", chunk_id, e)
        per_chunk.append(entry)
        if progress:
            await progress({"type": "chunk", "index": i, "total": total, "chunk_id": chunk_id, "result": entry})

    return {**totals, "unknown_classes": unknown_classes, "per_chunk": per_chunk, "log": "\n".join(log_lines)}
