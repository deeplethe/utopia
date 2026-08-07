"""Entity resolution for ABox extraction — the learned, human-in-the-loop resolution memory.

Every extracted *mention* (a surface form + its class) is resolved against, in order:
  1. the ``EntityResolution`` table   — a prior decision for this exact surface form + class
                                         (matched/new → reuse that individual; pending → skip)
  2. an exact label match             — an existing individual of the same class
  3. candidates + AGENT               — embeddings/strings RETRIEVE the closest same-class
                                         individuals; a multi-step LLM agent (it can inspect a
                                         candidate's facts and query past decisions via tools)
                                         decides match / new / uncertain. Never a blind
                                         similarity threshold. "uncertain" → manual queue.
  4. no candidate                     → mint a new individual (record "new")

Decisions are written back so the same surface form resolves instantly next time and a human
never has to judge the same pair twice. When no agent is wired (or ``agentic_resolution`` is
off), step 3 falls back to embedding thresholds.
"""
from __future__ import annotations

import json
import logging

from sqlmodel import Session, select

from app.config import settings
from app.db.models import EntityResolution, utcnow
from app.llm import openrouter
from app.ontology import abox, embeddings

logger = logging.getLogger(__name__)

AUTO_MATCH = 0.90   # fallback (no-agent) threshold: >= this → auto-linked
QUEUE_LOW = 0.78    # fallback (no-agent) band: [QUEUE_LOW, AUTO_MATCH) → manual queue


def _record(
    session: Session, *, ks_id: int, surface: str, class_iri: str, status: str,
    individual_iri: str | None, confidence: float | None, resolved_by: str | None,
    chunk_id: int | None, context: dict | None = None,
) -> EntityResolution:
    row = EntityResolution(
        knowledge_system_id=ks_id, surface_form=surface, class_iri=class_iri, status=status,
        individual_iri=individual_iri, confidence=confidence, resolved_by=resolved_by,
        source_chunk_id=chunk_id, context=context or {},
        resolved_at=utcnow() if status != "pending" else None,
    )
    session.add(row)
    return row


def _prior(session: Session, ks_id: int, surface: str, class_iri: str) -> EntityResolution | None:
    return session.exec(
        select(EntityResolution)
        .where(
            EntityResolution.knowledge_system_id == ks_id,
            EntityResolution.surface_form == surface,
            EntityResolution.class_iri == class_iri,
        )
        .order_by(EntityResolution.id.desc())
    ).first()


def _similarities(surface: str, existing: list[tuple[str, str]]) -> list[tuple[float, tuple[str, str]]]:
    """(score, (iri, label)) for each candidate, sorted best-first. Uses embedding cosine when
    the backend is available, otherwise a difflib string-similarity fallback (so resolution
    still discriminates near-duplicates instead of blindly minting a new individual)."""
    labels = [lbl for _, lbl in existing]
    vecs = embeddings.embed([surface] + labels)
    if vecs is not None and len(vecs) == len(existing) + 1:
        import numpy as np

        q = vecs[0]
        scored = [(float(np.dot(q, vecs[i + 1])), existing[i]) for i in range(len(existing))]
    else:
        import difflib

        s = surface.lower()
        scored = [(difflib.SequenceMatcher(None, s, lbl.lower()).ratio(), existing[i])
                  for i, (_, lbl) in enumerate(existing)]
    scored.sort(key=lambda x: -x[0])
    return scored


_AGENT_SYSTEM = """You are an entity-resolution agent. Decide whether a newly mentioned \
individual is the SAME real-world entity as one of the existing candidate individuals, or a \
genuinely NEW one. Do NOT rely on name similarity alone — inspect the facts before deciding.

Respond with EXACTLY ONE JSON object per turn — one of:
1) {"action":"get_details","iri":"<candidate iri>"}
   → returns that candidate's type(s), attributes and relationships.
2) {"action":"lookup_alias","text":"<a name / surface form>"}
   → returns past resolution decisions recorded for that name (learned aliases).
3) {"action":"finish","decision":"match|new|uncertain","iri":"<candidate iri, only if match>",
     "confidence":<0..1>,"reason":"<short>"}

Guidance:
- "match" only when confident it's the same real-world entity (a spelling/format/abbreviation
  variant, or the facts clearly line up). "iri" must be exactly one of the candidate iris.
- "new" when it's clearly a different individual (e.g. a different number / location / identity).
- "uncertain" ONLY when you truly cannot tell — it goes to a human queue. Prefer a decision
  when the evidence is clear.
- Keep tool use minimal (a couple of lookups), then finish.
- Output MUST be a single valid JSON object with no surrounding prose."""


def agentic_resolve(
    *, session: Session, ks_id: int, surface: str, class_label: str, evidence: str,
    candidates: list[tuple[str, str, float]], details_fn, model: str | None,
    max_steps: int = 4,
) -> dict:
    """Multi-step ReAct resolution: the agent inspects candidates / past decisions via tools,
    then finishes with match|new|uncertain. Returns {decision, iri, confidence, reason}. Runs
    synchronously (called from the extraction worker thread). ``details_fn(iri)`` returns a
    compact dict describing a candidate individual."""
    cand_iris = {iri for iri, _, _ in candidates}
    cand_lines = "\n".join(f'- iri={iri}  "{lbl}"  (similarity {score:.2f})' for iri, lbl, score in candidates)
    user = (
        f'New mention: "{surface}"  (type: {class_label or "?"})\n'
        f'{("Known facts: " + evidence) if evidence else "No extra facts were extracted for it."}\n\n'
        f'Existing candidate individuals of the same type:\n{cand_lines}\n\n'
        "Inspect what you need, then finish with your decision."
    )
    messages = [{"role": "system", "content": _AGENT_SYSTEM}, {"role": "user", "content": user}]

    for _ in range(max_steps):
        try:
            reply = openrouter.chat_sync(messages, model=model)
            data = openrouter.extract_json(reply)
        except Exception as e:  # noqa: BLE001  (LLM hiccup → let a human decide)
            logger.warning("resolution agent error (%s) → manual queue", e)
            return {"decision": "uncertain", "iri": None, "confidence": None, "reason": f"agent error: {e}"}

        # Every correction must echo the model's own reply first, or it never sees what it said
        # (repeats the mistake, burning the step budget) and some providers reject back-to-back
        # user turns.
        if not isinstance(data, dict):
            messages.append({"role": "assistant", "content": reply})
            messages.append({"role": "user", "content": "Reply with a single JSON object."})
            continue
        action = data.get("action")

        if action == "finish":
            decision = data.get("decision")
            if decision not in ("match", "new", "uncertain"):
                messages.append({"role": "assistant", "content": reply})
                messages.append({"role": "user", "content": 'decision must be "match", "new", or "uncertain".'})
                continue
            iri = data.get("iri") if decision == "match" else None
            if decision == "match" and iri not in cand_iris:
                messages.append({"role": "assistant", "content": reply})
                messages.append({"role": "user", "content": "For a match, iri must be exactly one candidate iri."})
                continue
            return {"decision": decision, "iri": iri,
                    "confidence": data.get("confidence"), "reason": str(data.get("reason", ""))[:200]}

        if action == "get_details":
            iri = str(data.get("iri", ""))
            det = details_fn(iri) if iri in cand_iris else None
            out = json.dumps(det, ensure_ascii=False) if det else "no such candidate"
            messages.append({"role": "assistant", "content": reply})
            messages.append({"role": "user", "content": f"get_details result:\n{out}"})
        elif action == "lookup_alias":
            text = str(data.get("text", "")).strip()
            rows = session.exec(
                select(EntityResolution).where(
                    EntityResolution.knowledge_system_id == ks_id,
                    EntityResolution.surface_form == text,
                    EntityResolution.status.in_(("matched", "new")),
                ).order_by(EntityResolution.id.desc())
            ).all()[:5]
            out = json.dumps([
                {"surface_form": r.surface_form, "status": r.status,
                 "individual": (details_fn(r.individual_iri) or {}).get("label") if r.individual_iri else None,
                 "reason": (r.context or {}).get("reason") or None}
                for r in rows
            ], ensure_ascii=False)
            messages.append({"role": "assistant", "content": reply})
            messages.append({"role": "user", "content": f"lookup_alias result:\n{out}"})
        else:
            messages.append({"role": "assistant", "content": reply})
            messages.append({"role": "user", "content": "Unknown action. Use get_details, lookup_alias, or finish."})

    return {"decision": "uncertain", "iri": None, "confidence": None,
            "reason": "agent did not finish within the step budget"}


def resolve_mention(
    session: Session, *, ks_id: int, abox_iri: str, base_iri: str,
    surface: str, class_iri: str, chunk_id: int | None = None,
    class_label: str = "", evidence: str = "", agent=None, pending_payload: dict | None = None,
    related_classes: set[str] | None = None, index: "abox.ResolutionIndex | None" = None,
) -> tuple[str | None, str]:
    """Resolve one mention to an individual IRI. Returns (individual_iri | None, status), where
    status ∈ {matched, new, pending}. Creates a new individual for ``new``; records a queue row
    for ``pending``. When ``agent`` is provided, it decides ambiguous candidate cases (see
    ``agentic_resolve``); otherwise embedding thresholds are used. Must be called with the ABox
    graph under an active ``store.capture`` if the write should land in history."""
    surface = surface.strip()
    if not surface:
        return None, "skipped"

    # 1) learned decision for this exact surface form + class
    prior = _prior(session, ks_id, surface, class_iri)
    if prior:
        if prior.status in ("matched", "new") and prior.individual_iri:
            _exists = (index.exists(prior.individual_iri) if index is not None
                       else abox.exists(abox_iri, prior.individual_iri))
            if _exists:
                return prior.individual_iri, "matched"
        if prior.status == "pending":
            return None, "pending"  # already queued; don't duplicate

    same_class = (index.individuals_of_class(class_iri) if index is not None
                  else abox.individuals_of_class(abox_iri, class_iri))

    # 2) exact label match within the same class (strong identity signal → auto-link)
    exact = next((iri for iri, lbl in same_class if lbl.strip() == surface), None)
    if exact:
        _record(session, ks_id=ks_id, surface=surface, class_iri=class_iri, status="matched",
                individual_iri=exact, confidence=1.0, resolved_by="agent", chunk_id=chunk_id)
        return exact, "matched"

    # 3) retrieve candidates across the class HIERARCHY (parent/child), then let the AGENT
    #    decide same/new/uncertain — so e.g. "Zhang"(Worker) and "Zhang"(Senior Worker) can be
    #    recognised as one person. On a match, the mention's class is unioned onto the individual.
    if related_classes:
        pool = list(index.individuals_of_classes(related_classes) if index is not None
                    else abox.individuals_of_classes(abox_iri, related_classes))
    else:
        pool = list(same_class)
    # A matching name is a strong identity signal the class hierarchy alone misses — e.g. the same
    # person mentioned under two sibling roles (维修工 vs 巡检员) with no sub/super link between them.
    # Surface exact same-name individuals from ANY class so the AGENT can judge them (it still
    # decides same/new/uncertain; we never auto-merge across unrelated classes here).
    have = {iri for iri, _ in pool}
    sl = surface.lower()
    for iri, lbl in (index.label_index() if index is not None else abox.label_index(abox_iri)).items():
        if iri not in have and lbl and lbl.strip().lower() == sl:
            pool.append((iri, lbl))
            have.add(iri)
    if pool:
        sims = _similarities(surface, pool)  # (score, (iri, lbl)) sorted by score desc
        ranked = [(iri, lbl, score) for score, (iri, lbl) in sims]
        above = [c for c in ranked if c[2] >= settings.resolution_candidate_floor]
        # Never let the floor hide the single best candidate from the agent — that reintroduces a
        # blind threshold and silently mints duplicates. Keep top-1 even if it's below the floor.
        candidates = (above or ranked[:1])[:settings.resolution_max_candidates]
        if candidates:
            cand_ctx = [{"iri": i, "label": l, "score": round(s, 3)} for i, l, s in candidates]
            if agent is not None:
                v = agent(surface, class_label, evidence, candidates)
                if v.get("decision") == "match" and v.get("iri"):
                    abox.add_type(abox_iri, v["iri"], class_iri)  # union the mention's class onto it
                    if index is not None:
                        index.add_type(v["iri"], class_iri)
                    _record(session, ks_id=ks_id, surface=surface, class_iri=class_iri, status="matched",
                            individual_iri=v["iri"], confidence=v.get("confidence"), resolved_by="agent",
                            chunk_id=chunk_id, context={"candidates": cand_ctx, "reason": v.get("reason", "")})
                    return v["iri"], "matched"
                if v.get("decision") == "uncertain":
                    _record(session, ks_id=ks_id, surface=surface, class_iri=class_iri, status="pending",
                            individual_iri=None, confidence=v.get("confidence"), resolved_by=None,
                            chunk_id=chunk_id,
                            context={"candidates": cand_ctx, "reason": v.get("reason", ""), **(pending_payload or {})})
                    return None, "pending"
                # decision == "new" → fall through to create
            else:
                # Fallback (no agent wired): embedding thresholds.
                score, (cand_iri, cand_lbl) = sims[0]
                if score >= AUTO_MATCH:
                    abox.add_type(abox_iri, cand_iri, class_iri)
                    if index is not None:
                        index.add_type(cand_iri, class_iri)
                    _record(session, ks_id=ks_id, surface=surface, class_iri=class_iri, status="matched",
                            individual_iri=cand_iri, confidence=score, resolved_by="agent", chunk_id=chunk_id,
                            context={"matched_label": cand_lbl})
                    return cand_iri, "matched"
                if score >= QUEUE_LOW:
                    _record(session, ks_id=ks_id, surface=surface, class_iri=class_iri, status="pending",
                            individual_iri=None, confidence=score, resolved_by=None, chunk_id=chunk_id,
                            context={"candidates": cand_ctx, **(pending_payload or {})})
                    return None, "pending"

    # 4) genuinely new individual
    iri = abox.create_individual(abox_iri, base_iri, surface, class_iri)
    if index is not None:
        index.add_individual(iri, surface, class_iri)
    _record(session, ks_id=ks_id, surface=surface, class_iri=class_iri, status="new",
            individual_iri=iri, confidence=None, resolved_by="agent", chunk_id=chunk_id)
    return iri, "new"
