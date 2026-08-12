"""Central configuration for the OntoPilot backend.

Reads settings from ``backend/.env`` (OpenRouter key, model choice) and derives
all runtime data paths under ``backend/data/`` (git-ignored).
"""
from __future__ import annotations

from functools import lru_cache
from pathlib import Path

from pydantic import field_validator
from pydantic_settings import BaseSettings, SettingsConfigDict
from sqlalchemy.engine import URL

# backend/ directory (this file lives at backend/app/config.py)
BACKEND_DIR = Path(__file__).resolve().parent.parent


class Settings(BaseSettings):
    model_config = SettingsConfigDict(
        env_file=str(BACKEND_DIR / ".env"),
        env_file_encoding="utf-8",
        extra="ignore",
    )

    # --- System language ---
    # Backend-only language for built-in model prompts. This is intentionally independent from
    # the browser UI locale. Docker/local deployments may override it with SYSTEM_LANGUAGE.
    system_language: str = "en"

    @field_validator("system_language")
    @classmethod
    def normalize_system_language(cls, value: str) -> str:
        normalized = value.strip().lower().replace("_", "-")
        if normalized in {"en", "en-us", "en-gb"}:
            return "en"
        if normalized in {"zh", "zh-cn", "zh-hans"}:
            return "zh-CN"
        raise ValueError("SYSTEM_LANGUAGE must be 'en' or 'zh-CN'")

    # --- LLM / OpenRouter ---
    openrouter_api_key: str = ""
    openrouter_base_url: str = "https://openrouter.ai/api/v1"
    llm_extract_model: str = "deepseek/deepseek-chat"
    llm_temperature: float = 0.1
    llm_max_tokens: int = 4000
    llm_timeout_s: float = 120.0
    # Models an admin/UI may pick from (allowlist bounds cost/safety). Extend via
    # LLM_MODEL_CHOICES in .env (a JSON list). Cheap-first per this project's model policy.
    llm_model_choices: list[str] = [
        "deepseek/deepseek-chat",
        "deepseek/deepseek-reasoner",
        "google/gemini-2.0-flash-001",
        "openai/gpt-4o-mini",
        "qwen/qwen-2.5-72b-instruct",
    ]

    # --- Paths (all under backend/data) ---
    data_dir: Path = BACKEND_DIR / "data"
    database_url: str = ""
    database_host: str = ""
    database_port: int = 5432
    database_name: str = "ontopilot"
    database_user: str = "ontopilot"
    database_password: str = ""

    # --- Chunking defaults ---
    chunk_size_chars: int = 1200
    chunk_overlap_chars: int = 150
    chunk_size_tokens: int = 900  # Docling HybridChunker budget (multilingual estimate)

    # --- Extraction mode ---
    #   rag      : retrieval-augmented single-shot (fast, scalable)
    #   agentic  : LLM tool-use loop (search_ontology / get_neighborhood) — most powerful
    #   auto     : agentic once the KS has >= agentic_min_classes, else rag
    extraction_mode: str = "auto"
    agentic_min_classes: int = 12
    agentic_max_steps: int = 6
    # Legacy/fallback limit: seeds model endpoints on upgrade and applies only when no endpoint exists.
    extraction_concurrency: int = 10

    # --- Independent TBox/ABox role verification ---
    # The first extractor proposes candidates; a separate critic classifies each candidate from
    # exact source evidence. High-confidence decisions are accepted, medium-confidence individual
    # decisions enter human review, and everything else is rejected rather than guessed.
    role_auto_accept_floor: float = 0.85
    role_review_floor: float = 0.55

    # --- Semantic (embedding-based) duplicate-class detection (via OpenRouter) ---
    # Embeddings are only a candidate generator (they conflate "related" with "same");
    # a cheap LLM then judges which candidates are truly the same concept.
    enable_semantic_conflicts: bool = True
    embedding_model: str = "baai/bge-m3"  # strong multilingual (CN+EN); dim 1024
    semantic_candidate_threshold: float = 0.75  # cosine to become a candidate pair
    verify_duplicates_with_llm: bool = True  # LLM judges candidates -> high precision

    # --- ABox entity resolution ---
    # Embeddings/strings only RETRIEVE candidates; a multi-step LLM agent (it can inspect a
    # candidate's facts and query past decisions via tools) makes the same/new/uncertain call.
    # When disabled, resolution falls back to embedding thresholds.
    agentic_resolution: bool = True
    resolution_candidate_floor: float = 0.78  # only plausible duplicates warrant an LLM judgment
    resolution_max_candidates: int = 5
    resolution_max_steps: int = 4  # ReAct tool-call budget per mention

    # --- TBox domain/range reconciliation ---
    # After concurrent per-chunk extraction, a property can accrue several domains/ranges
    # (each chunk picked one). A multi-step agent decides the right one (common superclass /
    # union / keep) instead of leaving an over-narrow domain. Falls back to the conflict queue
    # if the LLM is unavailable.
    agentic_tbox_reconcile: bool = True

    # --- Agentic conflict resolution ---
    # After conflict detection, an agent triages the auto-resolvable conflicts (duplicate classes,
    # over-specialized predicates): it picks the best resolution and — only when very confident —
    # applies it; otherwise it attaches a recommendation for a human's one-click confirmation.
    # Structural conflicts (cycles, disjoint contradictions) are always left to humans.
    agentic_conflict_resolution: bool = True
    conflict_auto_apply_floor: float = 0.85  # min confidence for the agent to apply without a human
    conflict_agent_max_steps: int = 3  # ReAct tool-call budget per conflict

    # After extraction, some classes end up with no parent and no relationships (the LLM extracted
    # a concept but never abstracted a parent for it). An agent proposes a broader parent class
    # (an existing one, or a new general kind) and attaches it via subclass_of when confident.
    agentic_isolated_classes: bool = True
    # A parent the agent proposes for MORE than this many isolated classes in one pass is treated as
    # an over-general "dumping ground" (a systematic mis-guess) and left for a human rather than
    # auto-attached — prevents e.g. dozens of unrelated classes all landing under one wrong parent.
    structure_max_same_parent: int = 5

    # --- Agentic validation triage ---
    # A datatype violation (a numeric data property carrying a qualitative value like "正常") means
    # either the value is noise (remove it) or the property is really qualitative (relax its range
    # to text). An agent judges each affected property from its value distribution and — when
    # confident — applies the fix; otherwise it leaves both options for a human.
    agentic_validation: bool = True
    validation_auto_apply_floor: float = 0.85

    # --- Controlled terminology formation ---
    # Every extraction deterministically mirrors named ontology entities into mapped SKOS concepts.
    # An optional LLM stage then proposes uncertain aliases/domain terms for human review.
    automatic_terminology: bool = True
    terminology_suggest_during_extraction: bool = True
    terminology_suggestion_max_chunks: int = 12
    terminology_suggestion_max_chars: int = 18_000

    # --- External read-only API ---
    external_query_max_rows: int = 500
    external_query_max_chars: int = 20_000
    # Optional Fernet key used to encrypt revealable API-token secrets. When empty, a random key
    # is created in data_dir/token-encryption.key and therefore persists with the data volume.
    token_encryption_key: str = ""

    # --- MCP ---
    # MCP is mounted into the FastAPI process and starts with the normal backend. This public URL
    # is used only for protocol metadata; reverse proxies may override it for their external host.
    mcp_public_url: str = "http://localhost:8000/mcp"
    mcp_token_ttl_minutes: int = 60
    mcp_max_token_ttl_minutes: int = 30 * 24 * 60

    # --- Direct RDF import ---
    rdf_import_max_bytes: int = 25 * 1024 * 1024
    rdf_import_max_triples: int = 250_000

    # --- CORS (frontend dev server) ---
    cors_origins: list[str] = [
        "http://localhost:5173",
        "http://127.0.0.1:5173",
    ]

    # --- Auth ---
    session_cookie: str = "ontopilot_session"
    session_ttl_hours: int = 24 * 14  # 2 weeks
    cookie_secure: bool = False       # set True when served over HTTPS
    admin_username: str = "admin"     # seed account created on first boot (empty user table)
    admin_password: str = ""          # required only when bootstrapping an empty user database
    seed_demo_data: bool = False       # create a deterministic no-LLM demo KS on first boot

    @property
    def blob_dir(self) -> Path:
        return self.data_dir / "blobs"

    @property
    def db_path(self) -> Path:
        return self.data_dir / "ontopilot.db"

    @property
    def db_url(self) -> str:
        if self.database_url:
            return self.database_url
        if self.database_host:
            return URL.create(
                "postgresql+psycopg",
                username=self.database_user,
                password=self.database_password,
                host=self.database_host,
                port=self.database_port,
                database=self.database_name,
            ).render_as_string(hide_password=False)
        return f"sqlite:///{self.db_path.as_posix()}"

    @property
    def oxigraph_dir(self) -> Path:
        return self.data_dir / "oxigraph"

    @property
    def serving_oxigraph_dir(self) -> Path:
        return self.data_dir / "serving-oxigraph"

    @property
    def release_dir(self) -> Path:
        return self.data_dir / "releases"

    @property
    def export_dir(self) -> Path:
        return self.data_dir / "exports"

    def ensure_dirs(self) -> None:
        self.data_dir.mkdir(parents=True, exist_ok=True)
        self.blob_dir.mkdir(parents=True, exist_ok=True)
        self.oxigraph_dir.mkdir(parents=True, exist_ok=True)
        self.serving_oxigraph_dir.mkdir(parents=True, exist_ok=True)
        self.release_dir.mkdir(parents=True, exist_ok=True)
        self.export_dir.mkdir(parents=True, exist_ok=True)


@lru_cache
def get_settings() -> Settings:
    settings = Settings()
    settings.ensure_dirs()
    return settings


settings = get_settings()
