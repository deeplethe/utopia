from __future__ import annotations

import pytest
from pydantic import ValidationError

from app import prompt_config
from app.config import Settings
from app.prompt_locales import ZH_CN_PROMPTS


@pytest.mark.parametrize("configured", ["zh", "zh_CN", "zh-cn", "zh-Hans"])
def test_system_language_normalizes_chinese_aliases(configured: str) -> None:
    assert Settings(_env_file=None, system_language=configured).system_language == "zh-CN"


@pytest.mark.parametrize("configured", ["en", "en-US", "en_gb"])
def test_system_language_normalizes_english_aliases(configured: str) -> None:
    assert Settings(_env_file=None, system_language=configured).system_language == "en"


def test_system_language_rejects_unsupported_value() -> None:
    with pytest.raises(ValidationError):
        Settings(_env_file=None, system_language="fr")


def test_system_language_can_be_loaded_from_env_file(tmp_path) -> None:
    config_file = tmp_path / ".env"
    config_file.write_text("SYSTEM_LANGUAGE=zh-CN\n", encoding="utf-8")
    assert Settings(_env_file=config_file).system_language == "zh-CN"


def test_chinese_catalog_covers_every_registered_prompt() -> None:
    definitions = prompt_config.definitions()
    assert set(ZH_CN_PROMPTS) == {item.key for item in definitions}
    for item in definitions:
        localized = ZH_CN_PROMPTS[item.key]
        assert localized.strip()
        prompt_config.validate_content(item, localized)


def test_chinese_default_selection(monkeypatch) -> None:
    monkeypatch.setattr(prompt_config.settings, "system_language", "zh-CN")
    assert prompt_config._localized_default(
        "conflict.duplicate_judge",
        "English fallback",
    ) == ZH_CN_PROMPTS["conflict.duplicate_judge"]


def test_english_default_selection(monkeypatch) -> None:
    monkeypatch.setattr(prompt_config.settings, "system_language", "en")
    assert prompt_config._localized_default("conflict.duplicate_judge", "English fallback") == "English fallback"
