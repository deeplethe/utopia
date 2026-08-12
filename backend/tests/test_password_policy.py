from __future__ import annotations

import pytest

from app.config import Settings
from app.security import validate_new_password


@pytest.mark.parametrize("password", ["", "admin", "short-pass"])
def test_new_password_rejects_short_values(password: str) -> None:
    with pytest.raises(ValueError, match="at least 12 characters"):
        validate_new_password(password)


@pytest.mark.parametrize("password", ["change-me", "replace-with-a-strong-password"])
def test_bootstrap_rejects_published_examples(password: str) -> None:
    with pytest.raises(ValueError, match="published example"):
        validate_new_password(password, bootstrap=True)


def test_new_password_rejects_values_beyond_bcrypt_limit() -> None:
    with pytest.raises(ValueError, match="72 UTF-8 bytes"):
        validate_new_password("密" * 25)


def test_strong_password_is_accepted() -> None:
    validate_new_password("correct-horse-battery-staple")


def test_admin_password_has_no_source_default() -> None:
    assert Settings(_env_file=None).admin_password == ""


def test_database_components_encode_special_password_characters() -> None:
    configured = Settings(
        _env_file=None,
        database_host="postgres",
        database_password="p@ss:/?#[] strong",
    )
    assert configured.db_url == (
        "postgresql+psycopg://ontopilot:p%40ss%3A%2F%3F%23%5B%5D strong@postgres:5432/ontopilot"
    )
