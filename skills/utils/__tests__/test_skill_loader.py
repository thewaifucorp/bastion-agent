"""Unit tests for skills.utils.skill_loader.load_skill_md."""

from __future__ import annotations

import json

import pytest

from skills.utils.skill_loader import load_skill_md


def make_skill(tmp_path, content, locales=None):
    skill_dir = tmp_path / "my-skill"
    skill_dir.mkdir()
    (skill_dir / "SKILL.md").write_text(content, encoding="utf-8")
    if locales:
        loc_dir = skill_dir / "locales"
        loc_dir.mkdir()
        for lang, data in locales.items():
            (loc_dir / f"{lang}.json").write_text(json.dumps(data), encoding="utf-8")
    return skill_dir


def test_single_token_substitution(tmp_path):
    """SKILL.md has {locale:hello}, locale has {"hello": "Olá"} → output is "Olá"."""
    skill_dir = make_skill(tmp_path, "{locale:hello}", locales={"pt-BR": {"hello": "Olá"}})
    result = load_skill_md(skill_dir, language="pt-BR")
    assert result == "Olá"


def test_multiple_distinct_tokens(tmp_path):
    """SKILL.md has {locale:a} {locale:b}, locale has both keys → both replaced."""
    skill_dir = make_skill(
        tmp_path,
        "{locale:a} {locale:b}",
        locales={"pt-BR": {"a": "Alpha", "b": "Beta"}},
    )
    result = load_skill_md(skill_dir, language="pt-BR")
    assert result == "Alpha Beta"


def test_missing_key_preserves_token(tmp_path):
    """Token {locale:missing} not in locale → token preserved as {locale:missing}."""
    skill_dir = make_skill(tmp_path, "{locale:missing}", locales={"pt-BR": {"other": "value"}})
    result = load_skill_md(skill_dir, language="pt-BR")
    assert result == "{locale:missing}"


def test_no_tokens_returns_identical_content(tmp_path):
    """SKILL.md has no {locale:*} → content returned byte-for-byte identical."""
    content = "# My Skill\n\nThis is a plain skill with no tokens.\n"
    skill_dir = make_skill(tmp_path, content)
    result = load_skill_md(skill_dir, language="pt-BR")
    assert result == content


def test_language_fallback_to_pt_br(tmp_path):
    """Request language="en" but only pt-BR.json exists → uses pt-BR values."""
    skill_dir = make_skill(
        tmp_path,
        "{locale:greeting}",
        locales={"pt-BR": {"greeting": "Olá"}},
    )
    result = load_skill_md(skill_dir, language="en")
    assert result == "Olá"


def test_missing_skill_md_raises_file_not_found(tmp_path):
    """SKILL.md does not exist → raises FileNotFoundError."""
    skill_dir = tmp_path / "empty-skill"
    skill_dir.mkdir()
    with pytest.raises(FileNotFoundError):
        load_skill_md(skill_dir, language="pt-BR")
