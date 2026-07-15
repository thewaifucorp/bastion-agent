"""
Shared i18n helper for Bastion skills.

Loads locale strings from a skill's locales/ directory based on the user's
language setting. Falls back to English if the requested locale is not found.

Usage:
    from utils.i18n import get_string, load_locale

    t = load_locale("pt-BR", skill_dir=Path(__file__).parent)
    message = get_string(t, "welcome")
"""

from __future__ import annotations

import contextlib
import json
from pathlib import Path
from typing import Any

_FALLBACK_LANGUAGE = "pt-BR"


def load_locale(language: str, skill_dir: Path) -> dict[str, Any]:
    """Load locale strings for the given language from skill_dir/locales/.

    Falls back to English if the requested language file is not found.

    Args:
        language: BCP-47 language tag (e.g. "pt-BR", "en", "es").
        skill_dir: Directory of the skill (contains the locales/ subfolder).

    Returns:
        Dictionary of string keys to localized values.
    """
    locales_dir = skill_dir / "locales"

    locale_file = locales_dir / f"{language}.json"
    if not locale_file.exists():
        locale_file = locales_dir / f"{_FALLBACK_LANGUAGE}.json"

    if not locale_file.exists():
        return {}

    return json.loads(locale_file.read_text(encoding="utf-8"))


def get_string(locale: dict[str, Any], key: str, **kwargs: Any) -> str:
    """Return a localized string by key, with optional format substitutions.

    Args:
        locale: Locale dictionary returned by load_locale().
        key: The string key to look up.
        **kwargs: Format arguments injected into the string via str.format().

    Returns:
        The localized string, or the key itself if not found.
    """
    value = locale.get(key, key)
    if kwargs:
        with contextlib.suppress(KeyError):
            value = value.format(**kwargs)
    return value
