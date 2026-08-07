"""SQLite metadata models (SQLModel).

These tables hold *metadata only* — the raw file bytes live in the content-addressed
blob store, and the ontology triples live in the Oxigraph RDF store. Everything here
is the relational glue that ties documents, chunks, knowledge systems and extraction
jobs together.
"""
from __future__ import annotations

from datetime import datetime, timezone
from typing import Optional

from sqlalchemy import JSON, Column, LargeBinary, Text
from sqlmodel import Field, SQLModel


def utcnow() -> datetime:
    return datetime.now(timezone.utc)


# --------------------------------------------------------------------------- #
# Auth: users & sessions
# --------------------------------------------------------------------------- #
class User(SQLModel, table=True):
    id: Optional[int] = Field(default=None, primary_key=True)
    username: str = Field(index=True, unique=True)  # login handle, immutable after creation
    display_name: Optional[str] = None  # optional friendly nickname shown in the UI; falls back to username
    password_hash: str
    is_admin: bool = False
    active: bool = True
    created_at: datetime = Field(default_factory=utcnow)


class AuthSession(SQLModel, table=True):
    """Server-side session: opaque token stored in an HttpOnly cookie."""

    token: str = Field(primary_key=True)
    user_id: int = Field(index=True, foreign_key="user.id")
    created_at: datetime = Field(default_factory=utcnow)
    expires_at: datetime = Field(default_factory=utcnow)


class KSGrant(SQLModel, table=True):
    """Per-knowledge-system access grant. Owner has full control implicitly; grants add
    other users as viewer (read) or editor (read + content ops)."""

    id: Optional[int] = Field(default=None, primary_key=True)
    knowledge_system_id: int = Field(index=True, foreign_key="knowledgesystem.id")
    user_id: int = Field(index=True, foreign_key="user.id")
    role: str = "viewer"  # viewer | editor
    created_at: datetime = Field(default_factory=utcnow)


# --------------------------------------------------------------------------- #
# Documents & chunks
# --------------------------------------------------------------------------- #
class Document(SQLModel, table=True):
    """An uploaded source file, bound to exactly one knowledge system (1 KS : N docs).

    The raw bytes are content-addressed in the blob store and shared across KS (identical
    bytes are stored once on disk), but each KS gets its own Document *row*: the same file
    uploaded into two knowledge systems is two documents. Dedup is therefore scoped per-KS
    (unique on knowledge_system_id + sha256), not globally.
    """

    id: Optional[int] = Field(default=None, primary_key=True)
    knowledge_system_id: Optional[int] = Field(default=None, index=True, foreign_key="knowledgesystem.id")
    sha256: str = Field(index=True)
    original_filename: str
    folder: str = Field(default="/", index=True)  # virtual folder path, KS-internal, e.g. "/manuals/pumps"
    ext: str = Field(index=True)  # normalized lowercase extension, no dot
    mime: Optional[str] = None
    size_bytes: int = 0
    storage_path: str = ""  # relative to settings.blob_dir, e.g. "aa/bb/<hash>"
    uploaded_at: datetime = Field(default_factory=utcnow)

    # Parsing lifecycle
    parse_status: str = Field(default="pending", index=True)  # pending|parsed|failed
    parser_backend: Optional[str] = None  # "docling" | "fallback:pdf" | ...
    parse_error: Optional[str] = None
    text_char_count: Optional[int] = None
    chunk_count: int = 0

    # Extraction lifecycle — whether this document has been run through TBox (schema) and/or
    # ABox (instance) extraction. Independent: a document may contribute schema only, instances
    # only (data that fits the existing ontology), or both. Set when a covering job completes.
    tbox_extracted_at: Optional[datetime] = None
    abox_extracted_at: Optional[datetime] = None


class Chunk(SQLModel, table=True):
    """A contiguous text slice of a parsed document."""

    id: Optional[int] = Field(default=None, primary_key=True)
    document_id: int = Field(index=True, foreign_key="document.id")
    idx: int = 0  # 0-based order within the document
    text: str = Field(sa_column=Column(Text))
    char_start: int = 0
    char_end: int = 0
    token_estimate: int = 0
    created_at: datetime = Field(default_factory=utcnow)


# --------------------------------------------------------------------------- #
# Knowledge systems (ontology graphs)
# --------------------------------------------------------------------------- #
class KnowledgeSystem(SQLModel, table=True):
    """A named ontology graph. Maps to one named graph (IRI) in the Oxigraph store."""

    id: Optional[int] = Field(default=None, primary_key=True)
    name: str = Field(index=True)
    description: str = Field(default="", sa_column=Column(Text))
    owner_id: Optional[int] = Field(default=None, index=True, foreign_key="user.id")
    graph_iri: str = ""  # e.g. "http://ontopilot.local/ks/3"
    base_iri: str = ""    # entity namespace, e.g. "http://ontopilot.local/ks/3/onto#"
    created_at: datetime = Field(default_factory=utcnow)
    updated_at: datetime = Field(default_factory=utcnow)

    # Cached stats (kept in sync after each extraction / manual edit)
    class_count: int = 0
    property_count: int = 0
    axiom_count: int = 0

    # Per-KS model/provider overrides (None -> system default). Switching a KS's embedding model
    # re-embeds only that KS's in-memory caches (vectors aren't persisted) — safe, see model_config.
    llm_model: Optional[str] = None
    llm_provider_id: Optional[int] = Field(default=None, foreign_key="provider.id")
    embedding_provider_id: Optional[int] = Field(default=None, foreign_key="provider.id")
    embedding_model: Optional[str] = None


class Provider(SQLModel, table=True):
    """One model endpoint entry: an OpenAI-compatible connection (base_url + api_key) bundled with a
    specific model and its kind (llm | embedding). A flat list of these is the whole config surface;
    the system default and each knowledge system just point at one llm entry + one embedding entry.
    The api_key is stored server-side and never returned raw by the API."""

    id: Optional[int] = Field(default=None, primary_key=True)
    name: str = Field(index=True)
    base_url: str = ""
    api_key: str = ""
    model: str = ""
    kind: str = Field(default="llm")  # "llm" | "embedding"
    last_test_ok: Optional[bool] = None       # result of the most recent connection test (persisted)
    last_tested_at: Optional[datetime] = None
    created_at: datetime = Field(default_factory=utcnow)


class SystemConfig(SQLModel, table=True):
    """Singleton (id=1) runtime config an admin can change WITHOUT a restart, overlaying the .env
    defaults in config.py. Holds the SYSTEM DEFAULT provider/model for chat + embeddings; a KS may
    override either. The embedding *dimension* is not swappable live (a change re-embeds), but its
    endpoint/model can point at a different provider."""

    id: Optional[int] = Field(default=1, primary_key=True)
    extract_model: Optional[str] = None       # default LLM model; None -> settings.llm_extract_model
    embedding_model: Optional[str] = None      # default embedding model; None -> settings.embedding_model
    llm_provider_id: Optional[int] = Field(default=None, foreign_key="provider.id")
    embedding_provider_id: Optional[int] = Field(default=None, foreign_key="provider.id")
    # Legacy (pre-Provider) single-connection fields; seeded into the "Default" provider on upgrade.
    base_url: Optional[str] = None
    api_key: Optional[str] = None
    updated_at: datetime = Field(default_factory=utcnow)


# --------------------------------------------------------------------------- #
# Extraction jobs & provenance
# --------------------------------------------------------------------------- #
class ExtractionJob(SQLModel, table=True):
    """One run of TBox extraction: a set of chunks -> ontology axioms for a KS."""

    id: Optional[int] = Field(default=None, primary_key=True)
    knowledge_system_id: int = Field(index=True, foreign_key="knowledgesystem.id")
    kind: str = Field(default="tbox", index=True)  # tbox | abox
    status: str = Field(default="pending", index=True)  # pending|running|completed|failed
    model: str = ""
    chunk_ids: list[int] = Field(default_factory=list, sa_column=Column(JSON))
    created_at: datetime = Field(default_factory=utcnow)
    finished_at: Optional[datetime] = None
    log: str = Field(default="", sa_column=Column(Text))
    error: Optional[str] = None

    # Progress (updated live while the job runs in the background; client polls these).
    total_chunks: int = 0
    processed_chunks: int = 0

    # TBox jobs use these; ABox jobs reuse the same row via the *_added counters below.
    classes_added: int = 0
    properties_added: int = 0
    axioms_added: int = 0

    # ABox (instance) extraction metrics.
    individuals_added: int = 0
    assertions_added: int = 0
    pending_added: int = 0  # mentions sent to the manual resolution queue
    # Class labels the instance extractor referenced that don't exist in the TBox yet
    # ({label: times_seen}) — surfaced as "add these to your ontology" suggestions.
    unknown_classes: dict = Field(default_factory=dict, sa_column=Column(JSON))


class AxiomProvenance(SQLModel, table=True):
    """Links an ontology axiom (by canonical key) back to the chunk/job that produced it."""

    id: Optional[int] = Field(default=None, primary_key=True)
    knowledge_system_id: int = Field(index=True, foreign_key="knowledgesystem.id")
    axiom_key: str = Field(index=True)  # canonical string, e.g. "subClassOf|dog|animal"
    chunk_id: Optional[int] = Field(default=None, foreign_key="chunk.id")
    job_id: Optional[int] = Field(default=None, foreign_key="extractionjob.id")
    created_at: datetime = Field(default_factory=utcnow)


class AboxProvenance(SQLModel, table=True):
    """Links an ABox fact — an individual, or a data/object assertion (by canonical key) — back to
    the chunk/job that produced it. A fact may have several rows: many chunks can mention the same
    individual or assert the same value, so ABox provenance is multi-source by design."""

    id: Optional[int] = Field(default=None, primary_key=True)
    knowledge_system_id: int = Field(index=True, foreign_key="knowledgesystem.id")
    # "ind|<iri>" | "data|<subject>|<prop>|<value>" | "obj|<subject>|<prop>|<target>"
    fact_key: str = Field(index=True)
    chunk_id: Optional[int] = Field(default=None, foreign_key="chunk.id")
    job_id: Optional[int] = Field(default=None, foreign_key="extractionjob.id")
    created_at: datetime = Field(default_factory=utcnow)


class AuditEvent(SQLModel, table=True):
    """Append-only change log for a knowledge system: who did what, when, with details.

    ``action`` is a dotted string (``ontology.edit``, ``conflict.resolve``,
    ``extraction.run``, ``document.upload``, ``member.add`` …); the part before the dot is
    the category used for filtering. ``actor_name`` is denormalized so records survive a
    user deletion. ``detail`` holds the operation payload (kept for a future rollback)."""

    id: Optional[int] = Field(default=None, primary_key=True)
    knowledge_system_id: int = Field(index=True, foreign_key="knowledgesystem.id")
    actor_id: Optional[int] = Field(default=None, foreign_key="user.id")
    actor_name: str = ""
    action: str = Field(index=True)
    summary: str = Field(default="", sa_column=Column(Text))
    detail: dict = Field(default_factory=dict, sa_column=Column(JSON))
    # Which named graph the diff below applies to. Null = the KS's TBox graph (graph_iri),
    # for backward compatibility; ABox events set it to the ABox graph. Rollback is scoped to
    # a single graph so undoing TBox history never touches instance data and vice versa.
    graph: Optional[str] = Field(default=None)
    # Events from ONE user action that spans multiple graphs (a delete/merge that cascades
    # TBox → ABox) share a group_id, so rolling back any one reverts the whole group across graphs.
    group_id: Optional[str] = Field(default=None, index=True)
    # Graph diff for rollback: gzipped N-Triples of the triples this event added / removed
    # (null for events that don't change the ontology graph, e.g. document upload).
    added: Optional[bytes] = Field(default=None, sa_column=Column(LargeBinary))
    removed: Optional[bytes] = Field(default=None, sa_column=Column(LargeBinary))
    created_at: datetime = Field(default_factory=utcnow, index=True)


class Conflict(SQLModel, table=True):
    """A detected ontology conflict/contradiction awaiting user resolution.

    ``signature`` is a stable key derived from the conflict's nature so that re-running
    detection does not re-open a conflict the user already resolved/dismissed.
    """

    id: Optional[int] = Field(default=None, primary_key=True)
    knowledge_system_id: int = Field(index=True, foreign_key="knowledgesystem.id")
    signature: str = Field(index=True)
    ctype: str = Field(index=True)  # cycle|disjoint_subclass|disjoint_common|domain_multi|range_multi|equiv_disjoint|duplicate
    severity: str = "error"  # error|warning
    status: str = Field(default="open", index=True)  # open|resolved|dismissed
    title: str = ""
    detail: str = Field(default="", sa_column=Column(Text))
    # payload: {"entities": [{"iri","label"}], "resolutions": [{"id","label","op"}]}
    payload: dict = Field(default_factory=dict, sa_column=Column(JSON))
    created_at: datetime = Field(default_factory=utcnow)
    resolved_at: Optional[datetime] = None
    resolution: Optional[str] = None  # resolution id applied, or "dismissed"


class EntityResolution(SQLModel, table=True):
    """Learned, human-in-the-loop entity-resolution memory for the ABox.

    One row = one decision about a mention (surface form of an individual, in the context of
    a class). It is both a *queue* (``status == "pending"`` = awaiting a human) and a
    *lookup table* the extraction agent consults before minting a new individual:

      pending  — the agent could not decide; waiting for manual resolution
      matched  — this surface form IS the existing individual ``individual_iri`` (alias)
      new      — this surface form is a distinct new individual (``individual_iri`` = the one created)
      distinct — recorded as NOT the same as ``individual_iri`` (a negative decision)

    ``surface_form`` + ``class_iri`` are the lookup key (scoped per KS). ``resolved_by`` is
    "agent" for automatic decisions above the confidence threshold, or a username for manual
    ones. ``source_chunk_id`` ties the mention back to its evidence.
    """

    id: Optional[int] = Field(default=None, primary_key=True)
    knowledge_system_id: int = Field(index=True, foreign_key="knowledgesystem.id")
    surface_form: str = Field(index=True)
    class_iri: Optional[str] = Field(default=None, index=True)
    status: str = Field(default="pending", index=True)  # pending|matched|new|distinct
    individual_iri: Optional[str] = None
    confidence: Optional[float] = None
    resolved_by: Optional[str] = None  # "agent" or a username
    source_chunk_id: Optional[int] = Field(default=None, foreign_key="chunk.id")
    context: dict = Field(default_factory=dict, sa_column=Column(JSON))  # candidates, evidence, notes
    created_at: datetime = Field(default_factory=utcnow, index=True)
    resolved_at: Optional[datetime] = None


class TboxReconciliation(SQLModel, table=True):
    """Learned memory for TBox domain/range reconciliation — the analog of EntityResolution
    for the schema. One row = a decision about how a property that accrued several
    domains/ranges was reconciled (a common superclass, a union, or keeping one). Written by
    both the reconciliation agent and by humans resolving a domain_multi/range_multi conflict,
    so the agent can query past decisions and "go by experience".
    """

    id: Optional[int] = Field(default=None, primary_key=True)
    knowledge_system_id: int = Field(index=True, foreign_key="knowledgesystem.id")
    slot: str = Field(index=True)  # domain | range
    property_label: str = Field(index=True)
    property_iri: Optional[str] = None
    candidates: list = Field(default_factory=list, sa_column=Column(JSON))  # candidate class labels
    choice: str = ""  # common_super | union | keep
    chosen_label: Optional[str] = None
    reason: Optional[str] = None  # the decider's rationale — shown to humans + fed back to the agent
    resolved_by: Optional[str] = None  # "agent" or a username
    created_at: datetime = Field(default_factory=utcnow, index=True)


class ValidationDecision(SQLModel, table=True):
    """Learned memory for datatype-violation fixes — the validation analog of the other queues.
    One row = "for this numeric-typed data property, the fix is X" (relax the range to text, or
    remove non-numeric values as noise). Written by the validation agent and by humans applying a
    datatype fix, so the agent consults it first and self-resolves instead of re-judging (and so a
    human's correction sticks)."""

    id: Optional[int] = Field(default=None, primary_key=True)
    knowledge_system_id: int = Field(index=True, foreign_key="knowledgesystem.id")
    property_label: str = Field(index=True)
    property_iri: Optional[str] = Field(default=None, index=True)
    xsd_type: Optional[str] = None  # the declared numeric type at decision time (decimal/integer/…)
    action: str = ""  # relax | remove
    reason: Optional[str] = None
    resolved_by: Optional[str] = None  # "agent" or a username
    created_at: datetime = Field(default_factory=utcnow, index=True)
