"""Central configuration for the OntoPilot backend.

Reads settings from ``backend/.env`` (OpenRouter key, model choice) and derives
all runtime data paths under ``backend/data/`` (git-ignored).
"""
from __future__ import annotations

from functools import lru_cache
from pathlib import Path

from pydantic_settings import BaseSettings, SettingsConfigDict

# backend/ directory (this file lives at backend/app/config.py)
BACKEND_DIR = Path(__file__).resolve().parent.parent


class Settings(BaseSettings):
    model_config = SettingsConfigDict(
        env_file=str(BACKEND_DIR / ".env"),
        env_file_encoding="utf-8",
        extra="ignore",
    )

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
    extraction_concurrency: int = 5  # max chunks extracted in parallel per job

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
    resolution_candidate_floor: float = 0.55  # min similarity to bother judging a candidate
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
    admin_password: str = "admin"     # CHANGE via backend/.env

    @property
    def blob_dir(self) -> Path:
        return self.data_dir / "blobs"

    @property
    def db_path(self) -> Path:
        return self.data_dir / "ontopilot.db"

    @property
    def db_url(self) -> str:
        return f"sqlite:///{self.db_path.as_posix()}"

    @property
    def oxigraph_dir(self) -> Path:
        return self.data_dir / "oxigraph"

    def ensure_dirs(self) -> None:
        self.data_dir.mkdir(parents=True, exist_ok=True)
        self.blob_dir.mkdir(parents=True, exist_ok=True)
        self.oxigraph_dir.mkdir(parents=True, exist_ok=True)


@lru_cache
def get_settings() -> Settings:
    settings = Settings()
    settings.ensure_dirs()
    return settings


settings = get_settings()
