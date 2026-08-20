"""Entity resolution for ABox extraction — the learned, human-in-the-loop resolution memory.

Every extracted *mention* (a surface form + its class) is resolved against, in order:
  1. the ``EntityResolution`` table   — a prior decision for this exact surface form + class
                                         in the same source document (matched/new → reuse that
                                         individual; pending → skip)
  2. an exact label match             — a candidate, not automatic identity across documents
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
import re
import unicodedata

from sqlmodel import Session, select

from app import prompt_config
from app.config import settings
from app.db.models import Chunk, EntityResolution, utcnow
from app.llm import openrouter
from app.ontology import abox, embeddings

logger = logging.getLogger(__name__)

AUTO_MATCH = 0.90   # fallback (no-agent) threshold: >= this → auto-linked
QUEUE_LOW = 0.78    # fallback (no-agent) band: [QUEUE_LOW, AUTO_MATCH) → manual queue
_NAME_TOKEN_RE = re.compile(r"\w+", re.UNICODE)
_IDENTIFIER_RE = re.compile(r"\d+")
TYPE_SUFFIX_ALIAS_SCORE = 0.89


def _record(
    session: Session, *, ks_id: int, surface: str, class_iri: str, status: str,
    individual_iri: str | None, confidence: float | None, resolved_by: str | None,
    chunk_id: int | None, context: dict | None = None,
) -> EntityResolution:
    document_id = _document_id(session, chunk_id)
    source_chunk = session.get(Chunk, chunk_id) if chunk_id is not None else None
    row = EntityResolution(
        knowledge_system_id=ks_id, surface_form=surface, class_iri=class_iri, status=status,
        individual_iri=individual_iri, confidence=confidence, resolved_by=resolved_by,
        source_chunk_id=chunk_id, source_document_id=document_id, context={
            **(context or {}),
            **({"source_chunk_idx": source_chunk.idx} if source_chunk is not None else {}),
        },
        resolved_at=utcnow() if status != "pending" else None,
    )
    session.add(row)
    return row


def _document_id(session: Session, chunk_id: int | None) -> int | None:
    if chunk_id is None:
        return None
    chunk = session.get(Chunk, chunk_id)
    return chunk.document_id if chunk else None


def _prior(
    session: Session,
    ks_id: int,
    surface: str,
    class_iri: str,
    *,
    chunk_id: int | None,
    global_scope: bool = False,
) -> EntityResolution | None:
    rows = session.exec(
        select(EntityResolution)
        .where(
            EntityResolution.knowledge_system_id == ks_id,
            EntityResolution.surface_form == surface,
            EntityResolution.class_iri == class_iri,
        )
        .order_by(EntityResolution.id.desc())
    ).all()
    if global_scope:
        return rows[0] if rows else None

    document_id = _document_id(session, chunk_id)
    for row in rows:
        if chunk_id is not None and row.source_chunk_id == chunk_id:
            return row
        row_document_id = row.source_document_id or _document_id(session, row.source_chunk_id)
        if document_id is not None and row_document_id == document_id:
            return row
    return None


def _name_key(value: str) -> str:
    value = unicodedata.normalize("NFKC", value or "").casefold().replace("_", " ")
    return " ".join(_NAME_TOKEN_RE.findall(value))


def _identity_name_key(value: str, class_label: str = "") -> str:
    """Normalize a name and remove an explicitly appended type label.

    Extractors commonly alternate between a proper name and ``name + class`` (for example,
    ``FalconGuard`` and ``FalconGuard admission plugin``). The stripped form is used only for
    candidate retrieval; the resolution agent still makes the identity decision.
    """
    name = _name_key(value)
    type_name = _name_key(class_label)
    if not name or not type_name or name == type_name:
        return name
    spaced_suffix = f" {type_name}"
    if name.endswith(spaced_suffix):
        return name[:-len(spaced_suffix)].strip() or name
    if (
        " " not in type_name
        and any(ord(char) > 127 for char in type_name)
        and name.endswith(type_name)
    ):
        base = name[:-len(type_name)].strip()
        if len(base) >= 2:
            return base
    return name


def _is_type_suffix_alias(surface: str, candidate: str, class_label: str) -> bool:
    source = _identity_name_key(surface, class_label)
    target = _identity_name_key(candidate, class_label)
    return bool(source and source == target and _name_key(surface) != _name_key(candidate))


def _lexical_candidate_pool(
    surface: str, existing: list[tuple[str, str]], *, class_label: str = "",
    floor: float = 0.55, limit: int = 24,
) -> list[tuple[str, str]]:
    """Cheap name-shape retrieval before expensive semantic embeddings.

    Entity identity normally requires overlapping names or identifiers.  Filtering here avoids
    embedding an ever-growing ABox for every obviously new proper name while retaining spelling
    variants, abbreviations, shared-token names, and exact labels across sibling classes.
    """
    import difflib

    source = _name_key(surface)
    if not source:
        return []
    source_tokens = set(source.split())
    source_ids = _IDENTIFIER_RE.findall(source)
    scored: list[tuple[float, tuple[str, str]]] = []
    for candidate in existing:
        label = _name_key(candidate[1])
        if not label:
            continue
        candidate_ids = _IDENTIFIER_RE.findall(label)
        if (source_ids or candidate_ids) and source_ids != candidate_ids:
            continue
        label_tokens = set(label.split())
        ratio = difflib.SequenceMatcher(None, source, label).ratio()
        overlap = len(source_tokens & label_tokens) / max(1, min(len(source_tokens), len(label_tokens)))
        containment = min(len(source), len(label)) / max(len(source), len(label)) \
            if min(len(source), len(label)) >= 4 and (source in label or label in source) else 0.0
        type_suffix_alias = 1.0 if _is_type_suffix_alias(surface, candidate[1], class_label) else 0.0
        score = max(ratio, overlap, containment, type_suffix_alias)
        if score >= floor:
            scored.append((score, candidate))
    scored.sort(key=lambda item: -item[0])
    return [candidate for _, candidate in scored[:limit]]


def _similarities(
    surface: str, existing: list[tuple[str, str]], *, class_label: str = "",
) -> list[tuple[float, tuple[str, str]]]:
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
    scored = [
        (
            max(score, TYPE_SUFFIX_ALIAS_SCORE)
            if _is_type_suffix_alias(surface, candidate[1], class_label)
            else score,
            candidate,
        )
        for score, candidate in scored
    ]
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
- A proper name and the same name with its declared type appended are strong alias evidence
  (for example, "FalconGuard" and "FalconGuard admission plugin"). Match them unless inspected
  facts establish that they are different entities.
- An identical surface form is not enough to merge entities of different types. Treat homonyms
  as different individuals unless their compatible identity roles and facts establish coreference.
- An identical name and type across different documents or examples is still not enough. Runtime
  resources, records, containers, jobs, and other locally named objects are normally distinct;
  match only when stable identifiers and compatible facts establish that they are the same entity.
- "new" when it's clearly a different individual (e.g. a different number / location / identity).
- "uncertain" ONLY when you truly cannot tell — it goes to a human queue. Prefer a decision
  when the evidence is clear.
- Keep tool use minimal (a couple of lookups), then finish.
- Output MUST be a single valid JSON object with no surrounding prose."""

prompt_config.register(
    key="abox.entity_resolution",
    category="review",
    title="Entity resolution",
    description="Decide whether a mention matches an existing individual, is new, or needs review.",
    default=_AGENT_SYSTEM,
    order=30,
)


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
        f'Existing candidate individuals of a compatible type/identity role:\n{cand_lines}\n\n'
        "Inspect what you need, then finish with your decision."
    )
    messages = [
        {"role": "system", "content": prompt_config.get("abox.entity_resolution")},
        {"role": "user", "content": user},
    ]

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
    related_classes: set[str] | None = None,
    roles_by_class: dict[str, frozenset[str]] | None = None,
    index: "abox.ResolutionIndex | None" = None,
    authoritative: bool = False,
    force_review_reason: str | None = None,
    force_review_confidence: float | None = None,
) -> tuple[str | None, str]:
    """Resolve one mention to an individual IRI. Returns (individual_iri | None, status), where
    status ∈ {matched, new, pending}. Creates a new individual for ``new``; records a queue row
    for ``pending``. When ``agent`` is provided, it decides ambiguous candidate cases (see
    ``agentic_resolve``); otherwise embedding thresholds are used. Must be called with the ABox
    graph under an active ``store.capture`` if the write should land in history."""
    surface = surface.strip()
    if not surface:
        return None, "skipped"

    # 1) learned decision for this exact surface form + class in the same source scope.
    prior = _prior(
        session,
        ks_id,
        surface,
        class_iri,
        chunk_id=chunk_id,
        global_scope=authoritative,
    )
    if prior:
        if prior.status in ("matched", "new") and prior.individual_iri:
            _exists = (index.exists(prior.individual_iri) if index is not None
                       else abox.exists(abox_iri, prior.individual_iri))
            if _exists:
                return prior.individual_iri, "matched"
        if prior.status in ("pending", "deferred"):
            return None, "pending"  # already queued; don't duplicate
        if prior.status == "rejected":
            return None, "skipped"  # invalid mention; do not materialize it on re-extraction

    if force_review_reason:
        _record(
            session,
            ks_id=ks_id,
            surface=surface,
            class_iri=class_iri,
            status="pending",
            individual_iri=None,
            confidence=force_review_confidence,
            resolved_by=None,
            chunk_id=chunk_id,
            context={
                "reason": force_review_reason,
                "evidence": evidence,
                "review_kind": "entity_role",
                **(pending_payload or {}),
            },
        )
        return None, "pending"

    same_class = (index.individuals_of_class(class_iri) if index is not None
                  else abox.individuals_of_class(abox_iri, class_iri))

    # Values routed from an authoritative structured source are controlled reference entries.
    # Different labels are distinct entries by definition, so semantic similarity must never
    # merge or queue them.
    if authoritative:
        exact = next(
            (iri for iri, label in same_class if _name_key(label) == _name_key(surface)),
            None,
        )
        if exact:
            _record(
                session,
                ks_id=ks_id,
                surface=surface,
                class_iri=class_iri,
                status="matched",
                individual_iri=exact,
                confidence=1.0,
                resolved_by="structured-field",
                chunk_id=chunk_id,
                context={"reason": "exact label matched an existing controlled reference entry"},
            )
            return exact, "matched"
        iri = abox.create_individual(abox_iri, base_iri, surface, class_iri)
        if index is not None:
            index.add_individual(iri, surface, class_iri)
        _record(
            session,
            ks_id=ks_id,
            surface=surface,
            class_iri=class_iri,
            status="new",
            individual_iri=iri,
            confidence=1.0,
            resolved_by="structured-field",
            chunk_id=chunk_id,
            context={"reason": "no exact controlled reference entry existed"},
        )
        return iri, "new"

    desired_roles = (roles_by_class or {}).get(class_iri, frozenset())

    def role_compatible(iri: str) -> bool:
        if not desired_roles or index is None:
            return False
        candidate_roles = frozenset(
            role
            for candidate_type in index.types_of(iri)
            for role in (roles_by_class or {}).get(candidate_type, ())
        )
        return bool(desired_roles & candidate_roles)

    # Exact names inside an explicitly supplied semantic identity role may be unified even when
    # the TBox classes are siblings. No identity roles are inferred from domain-specific labels.
    if index is not None and desired_roles:
        role_exact = next(
            (
                iri for iri, label in index.label_index().items()
                if label.strip().casefold() == surface.casefold() and role_compatible(iri)
            ),
            None,
        )
        if role_exact:
            abox.add_type(abox_iri, role_exact, class_iri)
            index.add_type(role_exact, class_iri)
            _record(session, ks_id=ks_id, surface=surface, class_iri=class_iri, status="matched",
                    individual_iri=role_exact, confidence=1.0, resolved_by="role-exact",
                    chunk_id=chunk_id, context={
                        "roles": sorted(desired_roles),
                        "reason": "exact label matched an existing individual in a compatible identity role",
                    })
            return role_exact, "matched"

    # 3) retrieve candidates across the class HIERARCHY (parent/child), then let the AGENT
    #    decide same/new/uncertain — so e.g. "Zhang"(Worker) and "Zhang"(Senior Worker) can be
    #    recognised as one person. On a match, the mention's class is unioned onto the individual.
    if related_classes:
        pool = list(index.individuals_of_classes(related_classes) if index is not None
                    else abox.individuals_of_classes(abox_iri, related_classes))
    else:
        pool = list(same_class)
    if agent is None and desired_roles and index is not None:
        pool = [(iri, label) for iri, label in pool if role_compatible(iri)]
    # Expand beyond the explicit hierarchy only inside a known semantic identity role. An exact
    # name across arbitrary classes is a homonym signal, not a coreference signal (for example a
    # Job and the Pod template it contains can share a manifest name).
    if desired_roles and index is not None:
        have = {iri for iri, _ in pool}
        for iri, lbl in index.label_index().items():
            if iri not in have and lbl and role_compatible(iri):
                pool.append((iri, lbl))
                have.add(iri)
    pool = _lexical_candidate_pool(surface, pool, class_label=class_label)
    new_context: dict | None = None
    new_confidence: float | None = None
    if pool:
        sims = _similarities(
            surface, pool, class_label=class_label,
        )  # (score, (iri, lbl)) sorted by score desc
        ranked = [(iri, lbl, score) for score, (iri, lbl) in sims]
        above = [c for c in ranked if c[2] >= settings.resolution_candidate_floor]
        # Below the candidate floor there is no credible identity candidate. Sending the best of
        # an unrelated pool to the agent makes every new entity require another LLM call and can
        # encourage false merges; mint it as new instead.
        candidates = above[:settings.resolution_max_candidates]
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
                            chunk_id=chunk_id, context={
                                "candidates": cand_ctx,
                                "reason": v.get("reason") or "resolution agent selected an existing identity",
                            })
                    return v["iri"], "matched"
                if v.get("decision") == "uncertain":
                    _record(session, ks_id=ks_id, surface=surface, class_iri=class_iri, status="pending",
                            individual_iri=None, confidence=v.get("confidence"), resolved_by=None,
                            chunk_id=chunk_id,
                            context={"candidates": cand_ctx, "reason": v.get("reason", ""),
                                     "evidence": evidence, **(pending_payload or {})})
                    return None, "pending"
                if v.get("decision") == "new":
                    new_confidence = v.get("confidence")
                    new_context = {
                        "candidates": cand_ctx,
                        "reason": v.get("reason") or "resolution agent judged the mention to be a new identity",
                    }
            else:
                # Fallback (no agent wired): embedding thresholds.
                score, (cand_iri, cand_lbl) = sims[0]
                if _name_key(surface) == _name_key(cand_lbl):
                    _record(
                        session,
                        ks_id=ks_id,
                        surface=surface,
                        class_iri=class_iri,
                        status="pending",
                        individual_iri=None,
                        confidence=score,
                        resolved_by=None,
                        chunk_id=chunk_id,
                        context={
                            "candidates": cand_ctx,
                            "reason": "same name and type found outside this source scope",
                            "evidence": evidence,
                            **(pending_payload or {}),
                        },
                    )
                    return None, "pending"
                if score >= AUTO_MATCH:
                    abox.add_type(abox_iri, cand_iri, class_iri)
                    if index is not None:
                        index.add_type(cand_iri, class_iri)
                    _record(session, ks_id=ks_id, surface=surface, class_iri=class_iri, status="matched",
                            individual_iri=cand_iri, confidence=score, resolved_by="agent", chunk_id=chunk_id,
                            context={
                                "matched_label": cand_lbl,
                                "reason": "candidate exceeded the automatic identity-match threshold",
                            })
                    return cand_iri, "matched"
                if score >= QUEUE_LOW:
                    _record(session, ks_id=ks_id, surface=surface, class_iri=class_iri, status="pending",
                            individual_iri=None, confidence=score, resolved_by=None, chunk_id=chunk_id,
                        context={
                            "candidates": cand_ctx,
                            "reason": "candidate similarity requires human identity review",
                            "evidence": evidence,
                                **(pending_payload or {}),
                            })
                    return None, "pending"

    # 4) genuinely new individual
    iri = abox.create_individual(abox_iri, base_iri, surface, class_iri)
    if index is not None:
        index.add_individual(iri, surface, class_iri)
    _record(
        session,
        ks_id=ks_id,
        surface=surface,
        class_iri=class_iri,
        status="new",
        individual_iri=iri,
        confidence=new_confidence,
        resolved_by="agent",
        chunk_id=chunk_id,
        context=new_context or {
            "reason": "no compatible existing individual met the candidate threshold",
        },
    )
    return iri, "new"
