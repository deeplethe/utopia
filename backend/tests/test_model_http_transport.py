from __future__ import annotations

import pytest

from app.llm.http_transport import trust_environment_proxy


@pytest.mark.parametrize(
    "url",
    [
        "http://localhost:11434/v1/embeddings",
        "http://localhost.:11434/v1/embeddings",
        "http://model.localhost:11434/v1/embeddings",
        "http://127.0.0.1:11434/v1/embeddings",
        "http://127.9.8.7:11434/v1/embeddings",
        "http://[::1]:11434/v1/embeddings",
    ],
)
def test_loopback_model_endpoints_bypass_environment_proxy(url: str) -> None:
    assert trust_environment_proxy(url) is False


@pytest.mark.parametrize(
    "url",
    [
        "https://openrouter.ai/api/v1/embeddings",
        "https://models.example/v1/chat/completions",
        "http://host.docker.internal:11434/v1/embeddings",
    ],
)
def test_remote_model_endpoints_keep_environment_proxy(url: str) -> None:
    assert trust_environment_proxy(url) is True
