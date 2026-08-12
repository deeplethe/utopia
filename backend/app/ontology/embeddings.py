"""Embedding backend via OpenRouter's OpenAI-compatible /embeddings endpoint.

No local model / torch download needed — reuses the OpenRouter key and works where the
HuggingFace hubs are unreachable. Default model is a strong multilingual one (bge-m3),
so Chinese and English labels are both handled. Degrades gracefully (returns None ->
callers fall back to string similarity) if the key is missing or the call fails.

Called only from conflict detection, which runs off the event loop (threadpool /
asyncio.to_thread), so the synchronous HTTP call here never blocks the server.
"""
from __future__ import annotations

import logging

logger = logging.getLogger(__name__)


def embed(texts: list[str]):
    """Return an (N, dim) L2-normalized numpy array, or None if unavailable."""
    from app.config import settings

    # Embeddings power BOTH TBox semantic-conflict detection AND ABox resolution candidate
    # retrieval. Enable if EITHER wants them, so disabling class-conflict detection doesn't
    # silently degrade resolution to difflib and spawn duplicate individuals.
    if (not settings.enable_semantic_conflicts and not settings.agentic_resolution) or not texts:
        return None
    from app import model_config  # effective (base_url, key, model) for embeddings
    from app.llm import capacity

    base_url, key, emodel = model_config.embed_conn()
    if not key:
        return None

    try:
        import httpx
        import numpy as np

        url = base_url.rstrip("/") + "/embeddings"
        with capacity.sync_slot(
            model_config.embedding_capacity_key(), model_config.embedding_concurrency(),
        ):
            resp = httpx.post(
                url,
                headers={"Authorization": f"Bearer {key}", "Content-Type": "application/json"},
                json={"model": emodel, "input": texts},
                timeout=settings.llm_timeout_s,
            )
        resp.raise_for_status()
        data = resp.json()["data"]

        # OpenAI-format responses carry an `index`; restore input order defensively.
        vecs = [None] * len(texts)
        for item in data:
            vecs[item.get("index", data.index(item))] = item["embedding"]

        arr = np.asarray(vecs, dtype="float32")
        norms = np.linalg.norm(arr, axis=1, keepdims=True)
        return arr / np.clip(norms, 1e-12, None)
    except Exception as e:  # noqa: BLE001
        logger.warning("OpenRouter embeddings unavailable (%s); using string similarity", e)
        return None
