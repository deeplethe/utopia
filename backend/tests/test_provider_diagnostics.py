from __future__ import annotations

from unittest.mock import MagicMock

import httpx
from pytest import MonkeyPatch

from app.api.providers import TestReq as ProviderTestRequest
from app.api.providers import test_provider as run_provider_test


def test_provider_test_reports_final_url_and_actionable_404_diagnostics(
    monkeypatch: MonkeyPatch,
) -> None:
    captured: dict[str, str] = {}

    def fake_post(url: str, **_: object) -> httpx.Response:
        captured["url"] = url
        return httpx.Response(404, json={"detail": "Not Found"})

    monkeypatch.setattr("app.api.providers.httpx.post", fake_post)

    result = run_provider_test(
        ProviderTestRequest(
            base_url="http://0.0.0.0:5004/api/v1/chat/completions",
            api_key="test-key",
            model="test-model",
            kind="llm",
        ),
        None,  # type: ignore[arg-type]
        MagicMock(),
    )

    assert captured["url"] == (
        "http://0.0.0.0:5004/api/v1/chat/completions/chat/completions"
    )
    assert result["ok"] is False
    assert result["status_code"] == 404
    assert result["detail"] == "Not Found"
    assert result["request_url"] == captured["url"]
    assert result["suggested_base_url"] == "http://0.0.0.0:5004/api/v1"
    assert result["diagnostic_codes"] == [
        "wildcard_host",
        "endpoint_path_in_base_url",
        "route_not_found",
    ]


def test_provider_test_reports_transport_error_type(monkeypatch: MonkeyPatch) -> None:
    def fake_post(_: str, **__: object) -> httpx.Response:
        raise httpx.ConnectError("Connection refused")

    monkeypatch.setattr("app.api.providers.httpx.post", fake_post)

    result = run_provider_test(
        ProviderTestRequest(
            base_url="http://127.0.0.1:5004/api/v1",
            api_key="test-key",
            model="test-model",
            kind="llm",
        ),
        None,  # type: ignore[arg-type]
        MagicMock(),
    )

    assert result["ok"] is False
    assert result["status_code"] is None
    assert result["error_type"] == "ConnectError"
    assert result["detail"] == "Connection refused"
    assert result["diagnostic_codes"] == ["connection_failed"]
