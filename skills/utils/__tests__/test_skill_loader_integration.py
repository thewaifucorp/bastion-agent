"""Integration tests for load_skill_md using real skill directories."""

from __future__ import annotations

import re
from pathlib import Path

import pytest

from skills.utils.skill_loader import load_skill_md

_LOCALE_TOKEN_RE = re.compile(r"\{locale:[a-zA-Z0-9_]+\}")

CRISIS_MODE_DIR = Path("skills/crisis-mode")
ONBOARDING_DIR = Path("skills/onboarding")


def test_crisis_mode_pt_br():
    """crisis-mode with pt-BR: no {locale:*} tokens remain in output."""
    result = load_skill_md(CRISIS_MODE_DIR, language="pt-BR")
    assert _LOCALE_TOKEN_RE.search(result) is None, (
        f"Unresolved locale tokens found in crisis-mode (pt-BR): "
        f"{_LOCALE_TOKEN_RE.findall(result)}"
    )


def test_crisis_mode_en():
    """crisis-mode with en: no {locale:*} tokens remain in output."""
    result = load_skill_md(CRISIS_MODE_DIR, language="en")
    assert _LOCALE_TOKEN_RE.search(result) is None, (
        f"Unresolved locale tokens found in crisis-mode (en): "
        f"{_LOCALE_TOKEN_RE.findall(result)}"
    )


def test_onboarding_pt_br():
    """onboarding with pt-BR: no {locale:*} tokens remain if locale file exists,
    otherwise content is returned unchanged (no crash)."""
    locales_dir = ONBOARDING_DIR / "locales"
    result = load_skill_md(ONBOARDING_DIR, language="pt-BR")

    if locales_dir.exists() and (locales_dir / "pt-BR.json").exists():
        assert _LOCALE_TOKEN_RE.search(result) is None, (
            f"Unresolved locale tokens found in onboarding (pt-BR): "
            f"{_LOCALE_TOKEN_RE.findall(result)}"
        )
    else:
        # No locale file — content returned as-is, no crash
        original = (ONBOARDING_DIR / "SKILL.md").read_text(encoding="utf-8")
        assert result == original


def test_onboarding_en():
    """onboarding with en: no {locale:*} tokens remain if locale file exists,
    otherwise content is returned unchanged (no crash)."""
    locales_dir = ONBOARDING_DIR / "locales"
    result = load_skill_md(ONBOARDING_DIR, language="en")

    if locales_dir.exists() and (locales_dir / "en.json").exists():
        assert _LOCALE_TOKEN_RE.search(result) is None, (
            f"Unresolved locale tokens found in onboarding (en): "
            f"{_LOCALE_TOKEN_RE.findall(result)}"
        )
    else:
        # No locale file — falls back to pt-BR or returns content as-is, no crash
        assert isinstance(result, str)
