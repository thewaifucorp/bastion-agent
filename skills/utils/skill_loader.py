"""
Skill SKILL.md loader with i18n token substitution.

Reads SKILL.md from a skill directory and replaces all {locale:key} tokens
with their localized values for the given language.

Usage:
    from pathlib import Path
    from skills.utils.skill_loader import load_skill_md

    content = load_skill_md(Path("skills/crisis-mode"), language="pt-BR")
"""

from __future__ import annotations

import re
from pathlib import Path

from skills.utils import i18n

_TOKEN_RE = re.compile(r"\{locale:([a-zA-Z0-9_]+)\}")


def load_skill_md(skill_dir: Path, language: str) -> str:
    """Read SKILL.md from skill_dir and replace all {locale:key} tokens.

    Args:
        skill_dir: Root directory of the skill (contains SKILL.md and locales/).
        language:  BCP-47 language tag (e.g. "pt-BR", "en").

    Returns:
        Content of SKILL.md with all tokens substituted.
        Tokens whose keys are absent from the locale are preserved as-is.

    Raises:
        FileNotFoundError: if SKILL.md does not exist in skill_dir.
    """
    skill_md = skill_dir / "SKILL.md"
    content = skill_md.read_text(encoding="utf-8")

    if not _TOKEN_RE.search(content):
        return content

    locale = i18n.load_locale(language, skill_dir)

    def _replace(match: re.Match[str]) -> str:
        key = match.group(1)
        value = i18n.get_string(locale, key)
        # get_string returns the key itself when not found — restore full token
        if value == key and key not in locale:
            return match.group(0)
        return value

    return _TOKEN_RE.sub(_replace, content)
