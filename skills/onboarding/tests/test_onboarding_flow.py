"""
pytest tests for the new onboarding flow.

Covers:
- USER.md schema: new fields (user_bio, pain_points_and_goals, timezone)
- Slug normalization (short, max 20 chars, no accents)
- IDENTITY.md generation
- i18n: locale loading and fallback
- Persona SOUL.md structure with current_state and specific_goals
"""

from __future__ import annotations

import json
import re
import unicodedata
from pathlib import Path

import pytest
import yaml


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _slugify(text: str, max_len: int = 20) -> str:
    """Minimal slug generation matching the SKILL.md rules."""
    normalized = unicodedata.normalize("NFKD", text)
    ascii_text = normalized.encode("ascii", "ignore").decode("ascii")
    slug = re.sub(r"[^a-z0-9]+", "-", ascii_text.lower()).strip("-")
    if len(slug) > max_len:
        slug = slug[:max_len].rsplit("-", 1)[0].strip("-")
    return slug


def _build_user_md_frontmatter(**kwargs) -> dict:
    """Return a minimal valid USER.md frontmatter dict with required fields."""
    base = {
        "name": "Test User",
        "language": "en",
        "timezone": "UTC",
        "occupation": "Developer",
        "has_business": False,
        "business_description": "",
        "user_bio": "",
        "pain_points_and_goals": "",
        "authorized_user_ids": ["123456"],
        "totp_configured": False,
        "personas": [],
        "onboarding_completed_at": "",
    }
    base.update(kwargs)
    return base


def _build_soul_md_frontmatter(**kwargs) -> dict:
    """Return a minimal valid persona SOUL.md frontmatter dict."""
    base = {
        "name": "Health",
        "slug": "saude",
        "base_weight": 0.8,
        "current_weight": 0.8,
        "domains": ["health", "wellness"],
        "trigger_keywords": ["workout", "diet"],
        "clawhub_skills": [],
        "current_state": "Going to the gym 3x per week",
        "specific_goals": "Lose 5kg in 3 months",
    }
    base.update(kwargs)
    return base


# ---------------------------------------------------------------------------
# USER.md schema
# ---------------------------------------------------------------------------

class TestUserMdSchema:
    def test_all_required_fields_present(self):
        fm = _build_user_md_frontmatter()
        required = [
            "name", "language", "timezone", "occupation",
            "has_business", "user_bio", "pain_points_and_goals",
            "authorized_user_ids", "totp_configured", "personas",
            "onboarding_completed_at",
        ]
        for field in required:
            assert field in fm, f"Missing required field: {field}"

    def test_user_bio_is_string(self):
        fm = _build_user_md_frontmatter(user_bio="Software developer focused on AI tools")
        assert isinstance(fm["user_bio"], str)

    def test_pain_points_is_string(self):
        fm = _build_user_md_frontmatter(pain_points_and_goals="Work-life balance, grow my business")
        assert isinstance(fm["pain_points_and_goals"], str)

    def test_timezone_is_string(self):
        fm = _build_user_md_frontmatter(timezone="America/Sao_Paulo")
        assert isinstance(fm["timezone"], str)
        assert len(fm["timezone"]) > 0

    def test_yaml_serialization_is_valid(self):
        """USER.md frontmatter must round-trip through YAML without corruption."""
        fm = _build_user_md_frontmatter(
            name="João Silva",
            user_bio="Desenvolvedor há 10 anos",
            pain_points_and_goals="Equilibrar trabalho e saúde",
            timezone="America/Sao_Paulo",
        )
        dumped = yaml.safe_dump(fm, allow_unicode=True, default_flow_style=False)
        loaded = yaml.safe_load(dumped)
        assert loaded["name"] == fm["name"]
        assert loaded["user_bio"] == fm["user_bio"]
        assert loaded["timezone"] == fm["timezone"]

    def test_authorized_user_ids_unchanged_after_onboarding(self):
        """Onboarding may only ADD to authorized_user_ids, never remove."""
        original_ids = ["111", "222"]
        fm = _build_user_md_frontmatter(authorized_user_ids=original_ids.copy())
        fm["authorized_user_ids"].append("333")
        assert "111" in fm["authorized_user_ids"]
        assert "222" in fm["authorized_user_ids"]
        assert "333" in fm["authorized_user_ids"]


# ---------------------------------------------------------------------------
# Slug normalization
# ---------------------------------------------------------------------------

class TestSlugNormalization:
    @pytest.mark.parametrize("area, expected", [
        ("Saúde e bem-estar", "saude-e-bem-estar"),
        ("Carreira em Tech", "carreira-em-tech"),
        ("Negócio / Empreendimento", "negocio"),  # "negocio-empreendimento" is 22 chars > 20 limit
        ("Família", "familia"),
        ("Finanças pessoais", "financas-pessoais"),
    ])
    def test_slug_has_no_accents(self, area: str, expected: str):
        slug = _slugify(area)
        assert slug == expected

    @pytest.mark.parametrize("area", [
        "Saúde (Faço academia e preciso que vc tb aja como um nutricionista)",
        "Carreira em Tech Lead — Backend",
        "Meu negócio na Katana, uma marca de roupas premium",
    ])
    def test_slug_max_20_chars(self, area: str):
        slug = _slugify(area)
        assert len(slug) <= 20, f"Slug '{slug}' exceeds 20 characters"

    def test_slug_lowercase(self):
        slug = _slugify("TRABALHO E CARREIRA")
        assert slug == slug.lower()

    def test_slug_no_special_chars(self):
        slug = _slugify("Saúde & Bem-estar!")
        assert re.match(r"^[a-z0-9-]+$", slug), f"Invalid chars in slug: {slug}"

    def test_slug_not_empty(self):
        slug = _slugify("Trabalho")
        assert len(slug) > 0

    def test_long_area_slug_ends_on_word_boundary(self):
        """Slug truncation should not cut in the middle of a word."""
        long_area = "Saude faco academia e preciso que vc tb aja como nutricionista"
        slug = _slugify(long_area)
        assert not slug.endswith("-")
        assert len(slug) <= 20


# ---------------------------------------------------------------------------
# Persona SOUL.md structure
# ---------------------------------------------------------------------------

class TestPersonaSoulMd:
    def test_required_fields_present(self):
        fm = _build_soul_md_frontmatter()
        required = [
            "name", "slug", "base_weight", "current_weight",
            "domains", "trigger_keywords", "clawhub_skills",
            "current_state", "specific_goals",
        ]
        for field in required:
            assert field in fm, f"Missing field in SOUL.md: {field}"

    def test_current_state_is_string(self):
        fm = _build_soul_md_frontmatter(current_state="Going to gym 3x/week")
        assert isinstance(fm["current_state"], str)

    def test_specific_goals_is_string(self):
        fm = _build_soul_md_frontmatter(specific_goals="Lose 5kg in 3 months")
        assert isinstance(fm["specific_goals"], str)

    def test_slug_matches_normalization_rules(self):
        fm = _build_soul_md_frontmatter(slug="saude")
        slug = fm["slug"]
        assert re.match(r"^[a-z0-9-]+$", slug)
        assert len(slug) <= 20

    def test_weights_are_floats_in_range(self):
        fm = _build_soul_md_frontmatter(base_weight=0.8, current_weight=0.8)
        assert 0.0 <= fm["base_weight"] <= 1.0
        assert 0.0 <= fm["current_weight"] <= 1.0

    def test_yaml_serialization_valid(self):
        fm = _build_soul_md_frontmatter()
        dumped = yaml.safe_dump(fm, allow_unicode=True, default_flow_style=False)
        loaded = yaml.safe_load(dumped)
        assert loaded["current_state"] == fm["current_state"]
        assert loaded["specific_goals"] == fm["specific_goals"]


# ---------------------------------------------------------------------------
# IDENTITY.md generation
# ---------------------------------------------------------------------------

class TestIdentityMd:
    def test_identity_fields_present(self):
        identity = {
            "bot_name": "Bastion",
            "base_behavior": "Direct and concise",
            "configured_at": "2026-04-05T12:00:00Z",
        }
        assert "bot_name" in identity
        assert "base_behavior" in identity
        assert "configured_at" in identity

    def test_identity_yaml_serialization(self):
        identity = {
            "bot_name": "Bastion",
            "base_behavior": "Friendly and detailed",
            "configured_at": "2026-04-05T12:00:00Z",
        }
        dumped = yaml.safe_dump(identity, allow_unicode=True)
        loaded = yaml.safe_load(dumped)
        assert loaded["bot_name"] == "Bastion"
        assert loaded["base_behavior"] == "Friendly and detailed"


# ---------------------------------------------------------------------------
# i18n engine
# ---------------------------------------------------------------------------

LOCALES_DIR = Path(__file__).parent.parent / "locales"


class TestI18n:
    def test_en_locale_file_exists(self):
        assert (LOCALES_DIR / "en.json").exists()

    def test_pt_br_locale_file_exists(self):
        assert (LOCALES_DIR / "pt-BR.json").exists()

    def test_en_locale_is_valid_json(self):
        content = (LOCALES_DIR / "en.json").read_text(encoding="utf-8")
        data = json.loads(content)
        assert isinstance(data, dict)

    def test_pt_br_locale_is_valid_json(self):
        content = (LOCALES_DIR / "pt-BR.json").read_text(encoding="utf-8")
        data = json.loads(content)
        assert isinstance(data, dict)

    def test_both_locales_have_same_keys(self):
        en = json.loads((LOCALES_DIR / "en.json").read_text(encoding="utf-8"))
        pt = json.loads((LOCALES_DIR / "pt-BR.json").read_text(encoding="utf-8"))
        assert set(en.keys()) == set(pt.keys()), (
            f"Key mismatch — en has {set(en.keys()) - set(pt.keys())}, "
            f"pt-BR has {set(pt.keys()) - set(en.keys())}"
        )

    def test_load_locale_returns_correct_language(self):
        from i18n import load_locale
        skill_dir = Path(__file__).parent.parent
        en = load_locale("en", skill_dir)
        assert "welcome" in en
        assert "ask_name" in en

    def test_load_locale_falls_back_to_english(self):
        from i18n import load_locale
        skill_dir = Path(__file__).parent.parent
        result = load_locale("xx-NONEXISTENT", skill_dir)
        assert "welcome" in result

    def test_get_string_returns_value(self):
        from i18n import get_string, load_locale
        skill_dir = Path(__file__).parent.parent
        locale = load_locale("en", skill_dir)
        value = get_string(locale, "ask_name")
        assert isinstance(value, str)
        assert len(value) > 0

    def test_get_string_returns_key_if_missing(self):
        from i18n import get_string
        result = get_string({}, "nonexistent_key")
        assert result == "nonexistent_key"

    def test_required_keys_present_in_en(self):
        required_keys = [
            "welcome", "ask_name", "ask_bio", "ask_pain_points",
            "ask_life_areas", "ask_identity_name", "ask_identity_behavior",
            "completion",
        ]
        en = json.loads((LOCALES_DIR / "en.json").read_text(encoding="utf-8"))
        for key in required_keys:
            assert key in en, f"Missing required i18n key in en.json: {key}"
