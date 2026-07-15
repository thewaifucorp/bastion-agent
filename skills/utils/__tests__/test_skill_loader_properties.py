"""
Property-based tests for skills/utils/skill_loader.py using hypothesis.

**Validates: Requirements 1.2**
"""

from __future__ import annotations

import re
import tempfile
from pathlib import Path

from hypothesis import given, settings
from hypothesis import strategies as st

from skills.utils.skill_loader import load_skill_md

# Regex to detect any remaining {locale:*} tokens in output
_REMAINING_TOKEN_RE = re.compile(r"\{locale:[a-zA-Z0-9_]+\}")

# Strategy: valid token keys — alphanumeric + underscore, at least 1 char
_key_strategy = st.text(
    alphabet=st.characters(whitelist_categories=(), whitelist_characters="abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_"),
    min_size=1,
    max_size=20,
)

# Strategy: locale values — printable text that does NOT contain {locale: patterns
_value_strategy = st.text(
    alphabet=st.characters(blacklist_characters="\x00"),
    min_size=0,
    max_size=100,
).filter(lambda v: "{locale:" not in v)


@given(
    keys=st.lists(_key_strategy, min_size=1, max_size=5, unique=True),
    values=st.lists(_value_strategy, min_size=1, max_size=5),
    surrounding_text=st.text(
        alphabet=st.characters(blacklist_characters="\x00"),
        min_size=0,
        max_size=200,
    ).filter(lambda t: not _REMAINING_TOKEN_RE.search(t)),
)
@settings(max_examples=100)
def test_property1_complete_substitution(
    keys: list[str],
    values: list[str],
    surrounding_text: str,
) -> None:
    """
    Property 1: Substituição completa de tokens.

    For any SKILL.md content and locale dictionary where every token key
    present in the content exists in the locale, calling `load_skill_md`
    should produce output that contains no remaining {locale:*} tokens.

    **Validates: Requirements 1.2**
    """
    # Pair keys with values (cycle values if fewer than keys)
    locale: dict[str, str] = {
        key: values[i % len(values)] for i, key in enumerate(keys)
    }

    # Build SKILL.md content: interleave surrounding_text with tokens for all keys
    tokens_block = " ".join(f"{{locale:{key}}}" for key in keys)
    skill_md_content = f"{surrounding_text}\n{tokens_block}\n"

    with tempfile.TemporaryDirectory() as tmp_dir:
        skill_dir = Path(tmp_dir)

        # Write SKILL.md
        (skill_dir / "SKILL.md").write_text(skill_md_content, encoding="utf-8")

        # Write locale file
        import json
        locales_dir = skill_dir / "locales"
        locales_dir.mkdir()
        (locales_dir / "pt-BR.json").write_text(
            json.dumps(locale), encoding="utf-8"
        )

        result = load_skill_md(skill_dir, language="pt-BR")

        # Assert: no {locale:*} tokens remain in the output
        assert not _REMAINING_TOKEN_RE.search(result), (
            f"Remaining tokens found in output.\n"
            f"Keys: {keys}\n"
            f"Locale: {locale}\n"
            f"Input: {skill_md_content!r}\n"
            f"Output: {result!r}"
        )


# ---------------------------------------------------------------------------
# Property 2: Não-interferência
# **Validates: Requirements 1.3, 5.1**
# ---------------------------------------------------------------------------

# Strategy: text that contains NO {locale:*} pattern but MAY contain other
# {placeholder} patterns like {persona}, {hours}, etc.
# Excludes \r and \x00 because Python's universal-newline read_text() normalises
# bare \r → \n, which would cause a spurious round-trip mismatch unrelated to
# the property under test.
_no_locale_token_strategy = st.text(
    alphabet=st.characters(blacklist_characters="\x00\r", blacklist_categories=("Cs",)),
    min_size=0,
    max_size=300,
).filter(lambda t: not _REMAINING_TOKEN_RE.search(t))

# Strategy: placeholder names like "persona", "hours", "name" (no colon)
_placeholder_name_strategy = st.text(
    alphabet=st.characters(
        whitelist_categories=(),
        whitelist_characters="abcdefghijklmnopqrstuvwxyz_",
    ),
    min_size=1,
    max_size=15,
).filter(lambda s: "locale" not in s)


@given(content=_no_locale_token_strategy)
@settings(max_examples=100)
def test_property2a_no_tokens_identical_content(content: str) -> None:
    """
    Property 2a: Não-interferência — conteúdo sem tokens {locale:*}.

    For any SKILL.md content that contains no {locale:key} tokens
    (including content with other {placeholder} patterns like {persona},
    {hours}), load_skill_md must return a string byte-for-byte identical
    to the original content.

    **Validates: Requirements 1.3, 5.1**
    """
    import json

    with tempfile.TemporaryDirectory() as tmp_dir:
        skill_dir = Path(tmp_dir)
        (skill_dir / "SKILL.md").write_text(content, encoding="utf-8")

        # Locale file present but irrelevant — no tokens to replace
        locales_dir = skill_dir / "locales"
        locales_dir.mkdir()
        (locales_dir / "pt-BR.json").write_text(
            json.dumps({"some_key": "some_value"}), encoding="utf-8"
        )

        result = load_skill_md(skill_dir, language="pt-BR")

        assert result == content, (
            f"Output differs from input for content with no {{locale:*}} tokens.\n"
            f"Input:  {content!r}\n"
            f"Output: {result!r}"
        )


@given(
    keys=st.lists(_key_strategy, min_size=1, max_size=5, unique=True),
    placeholder_names=st.lists(
        _placeholder_name_strategy, min_size=1, max_size=3, unique=True
    ),
    surrounding_text=_no_locale_token_strategy,
)
@settings(max_examples=100)
def test_property2b_runtime_placeholders_in_locale_values_preserved(
    keys: list[str],
    placeholder_names: list[str],
    surrounding_text: str,
) -> None:
    """
    Property 2b: Não-interferência — placeholders de runtime em valores de locale.

    For any locale value that itself contains {placeholder} patterns (e.g.
    {persona}, {hours}), those patterns must remain unmodified in the output
    after load_skill_md performs {locale:key} substitution.

    **Validates: Requirements 1.3, 5.1**
    """
    import json

    # Build locale values that embed runtime placeholders like {persona}
    # Each value contains at least one {placeholder_name} pattern
    locale: dict[str, str] = {}
    for i, key in enumerate(keys):
        ph = placeholder_names[i % len(placeholder_names)]
        locale[key] = f"value with {{{ph}}} inside"

    # Build SKILL.md: surrounding text + one {locale:key} token per key
    tokens_block = " ".join(f"{{locale:{key}}}" for key in keys)
    skill_md_content = f"{surrounding_text}\n{tokens_block}\n"

    with tempfile.TemporaryDirectory() as tmp_dir:
        skill_dir = Path(tmp_dir)
        (skill_dir / "SKILL.md").write_text(skill_md_content, encoding="utf-8")

        locales_dir = skill_dir / "locales"
        locales_dir.mkdir()
        (locales_dir / "pt-BR.json").write_text(
            json.dumps(locale), encoding="utf-8"
        )

        result = load_skill_md(skill_dir, language="pt-BR")

        # Every runtime placeholder that appeared in locale values must survive
        for key in keys:
            ph = placeholder_names[keys.index(key) % len(placeholder_names)]
            expected_fragment = f"{{{ph}}}"
            assert expected_fragment in result, (
                f"Runtime placeholder {expected_fragment!r} was lost after substitution.\n"
                f"Locale key: {key!r}, locale value: {locale[key]!r}\n"
                f"Input:  {skill_md_content!r}\n"
                f"Output: {result!r}"
            )


# ---------------------------------------------------------------------------
# Property 3: Preservação de tokens com chaves ausentes
# **Validates: Requirements 2.1, 2.2**
# ---------------------------------------------------------------------------

# Strategy: keys that will be MISSING from the locale
_missing_key_strategy = _key_strategy

# Strategy: locale dict that may have other keys but NOT the missing ones
# (may also be empty)
def _locale_without_keys(missing_keys: list[str]) -> st.SearchStrategy[dict[str, str]]:
    """Build a locale dict that does NOT contain any of the missing_keys."""
    other_key = st.text(
        alphabet=st.characters(
            whitelist_categories=(),
            whitelist_characters="abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_",
        ),
        min_size=1,
        max_size=20,
    ).filter(lambda k: k not in missing_keys)

    return st.one_of(
        # Empty locale (no locales/ directory scenario is tested separately below)
        st.just({}),
        # Locale with unrelated keys only
        st.dictionaries(
            keys=other_key,
            values=_value_strategy,
            min_size=0,
            max_size=5,
        ).map(lambda d: {k: v for k, v in d.items() if k not in missing_keys}),
    )


@given(
    missing_keys=st.lists(_missing_key_strategy, min_size=1, max_size=5, unique=True),
    surrounding_text=st.text(
        alphabet=st.characters(blacklist_characters="\x00\r", blacklist_categories=("Cs",)),
        min_size=0,
        max_size=200,
    ).filter(lambda t: not _REMAINING_TOKEN_RE.search(t)),
    use_empty_locale=st.booleans(),
)
@settings(max_examples=100)
def test_property3_missing_keys_preserved(
    missing_keys: list[str],
    surrounding_text: str,
    use_empty_locale: bool,
) -> None:
    """
    Property 3: Preservação de tokens com chaves ausentes.

    For any SKILL.md content containing tokens {locale:key} where key is
    absent from the loaded locale dictionary (including the case where the
    locale dict is empty), load_skill_md should preserve those tokens
    exactly as they appear in the original content.

    **Validates: Requirements 2.1, 2.2**
    """
    import json

    # Build SKILL.md content with tokens for all missing keys
    tokens_block = " ".join(f"{{locale:{key}}}" for key in missing_keys)
    skill_md_content = f"{surrounding_text}\n{tokens_block}\n"

    with tempfile.TemporaryDirectory() as tmp_dir:
        skill_dir = Path(tmp_dir)
        (skill_dir / "SKILL.md").write_text(skill_md_content, encoding="utf-8")

        if use_empty_locale:
            # Sub-case: empty locale dict — locales/ dir exists but JSON is empty
            locales_dir = skill_dir / "locales"
            locales_dir.mkdir()
            (locales_dir / "pt-BR.json").write_text(json.dumps({}), encoding="utf-8")
        else:
            # Sub-case: locale has other keys but NOT the missing ones
            # Build a locale that excludes all missing_keys
            other_keys = [f"other_{k}" for k in missing_keys]
            locale: dict[str, str] = {k: f"value_for_{k}" for k in other_keys}
            locales_dir = skill_dir / "locales"
            locales_dir.mkdir()
            (locales_dir / "pt-BR.json").write_text(
                json.dumps(locale), encoding="utf-8"
            )

        result = load_skill_md(skill_dir, language="pt-BR")

        # Assert: every {locale:missing_key} token is preserved verbatim
        for key in missing_keys:
            expected_token = f"{{locale:{key}}}"
            assert expected_token in result, (
                f"Token {expected_token!r} was NOT preserved in output.\n"
                f"Missing keys: {missing_keys}\n"
                f"use_empty_locale: {use_empty_locale}\n"
                f"Input:  {skill_md_content!r}\n"
                f"Output: {result!r}"
            )


def test_property3_empty_locale_no_locales_dir() -> None:
    """
    Property 3 — edge case: no locales/ directory at all.

    When there is no locales/ directory (and thus no locale file), all
    {locale:key} tokens must be preserved exactly as-is.

    **Validates: Requirements 2.2**
    """
    import json

    missing_keys = ["alpha", "beta", "gamma_key"]
    tokens_block = " ".join(f"{{locale:{key}}}" for key in missing_keys)
    skill_md_content = f"Header\n{tokens_block}\nFooter\n"

    with tempfile.TemporaryDirectory() as tmp_dir:
        skill_dir = Path(tmp_dir)
        (skill_dir / "SKILL.md").write_text(skill_md_content, encoding="utf-8")
        # Intentionally do NOT create locales/ directory

        result = load_skill_md(skill_dir, language="pt-BR")

        for key in missing_keys:
            expected_token = f"{{locale:{key}}}"
            assert expected_token in result, (
                f"Token {expected_token!r} was NOT preserved when no locales/ dir exists.\n"
                f"Output: {result!r}"
            )


# ---------------------------------------------------------------------------
# Property 5: Idempotência
# **Validates: Requirements 7.1**
# ---------------------------------------------------------------------------


@given(
    content=st.text(
        alphabet=st.characters(blacklist_characters="\x00"),
        min_size=0,
        max_size=300,
    ),
    locale_dict=st.dictionaries(
        keys=_key_strategy,
        values=_value_strategy,
        min_size=0,
        max_size=5,
    ),
)
@settings(max_examples=100)
def test_property5_idempotency(
    content: str,
    locale_dict: dict[str, str],
) -> None:
    """
    Property 5: Idempotência.

    For any valid skill_dir and language, calling load_skill_md(skill_dir, language)
    multiple times with the same arguments should always return the same string.

    **Validates: Requirements 7.1**
    """
    import json

    with tempfile.TemporaryDirectory() as tmp_dir:
        skill_dir = Path(tmp_dir)
        (skill_dir / "SKILL.md").write_text(content, encoding="utf-8")

        locales_dir = skill_dir / "locales"
        locales_dir.mkdir()
        (locales_dir / "pt-BR.json").write_text(
            json.dumps(locale_dict), encoding="utf-8"
        )

        result1 = load_skill_md(skill_dir, language="pt-BR")
        result2 = load_skill_md(skill_dir, language="pt-BR")

        assert result1 == result2, (
            f"load_skill_md returned different results on repeated calls.\n"
            f"Result 1: {result1!r}\n"
            f"Result 2: {result2!r}"
        )


# ---------------------------------------------------------------------------
# Property 4: Fallback para pt-BR
# **Validates: Requirements 3.1, 3.2**
# ---------------------------------------------------------------------------

# Strategy: language tags that are NOT "pt-BR" and won't have a locale file
_non_pt_br_language_strategy = st.text(
    alphabet=st.characters(
        whitelist_categories=(),
        whitelist_characters="abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_",
    ),
    min_size=1,
    max_size=10,
).filter(lambda lang: lang != "pt-BR")


@given(
    language=_non_pt_br_language_strategy,
    keys=st.lists(_key_strategy, min_size=1, max_size=5, unique=True),
    values=st.lists(_value_strategy, min_size=1, max_size=5),
    surrounding_text=st.text(
        alphabet=st.characters(blacklist_characters="\x00\r", blacklist_categories=("Cs",)),
        min_size=0,
        max_size=200,
    ).filter(lambda t: not _REMAINING_TOKEN_RE.search(t)),
)
@settings(max_examples=100)
def test_property4_fallback_to_pt_br(
    language: str,
    keys: list[str],
    values: list[str],
    surrounding_text: str,
) -> None:
    """
    Property 4: Fallback para pt-BR.

    For any language tag for which no locale file exists in locales/, if
    locales/pt-BR.json exists, then load_skill_md should produce the same
    output as if language="pt-BR" had been passed directly.

    **Validates: Requirements 3.1, 3.2**
    """
    import json

    locale: dict[str, str] = {
        key: values[i % len(values)] for i, key in enumerate(keys)
    }

    tokens_block = " ".join(f"{{locale:{key}}}" for key in keys)
    skill_md_content = f"{surrounding_text}\n{tokens_block}\n"

    with tempfile.TemporaryDirectory() as tmp_dir:
        skill_dir = Path(tmp_dir)
        (skill_dir / "SKILL.md").write_text(skill_md_content, encoding="utf-8")

        # Create only locales/pt-BR.json — no file for the generated language
        locales_dir = skill_dir / "locales"
        locales_dir.mkdir()
        (locales_dir / "pt-BR.json").write_text(
            json.dumps(locale), encoding="utf-8"
        )

        result_fallback = load_skill_md(skill_dir, language=language)
        result_direct = load_skill_md(skill_dir, language="pt-BR")

        assert result_fallback == result_direct, (
            f"Fallback result differs from direct pt-BR result.\n"
            f"Language: {language!r}\n"
            f"Locale: {locale}\n"
            f"Input: {skill_md_content!r}\n"
            f"Fallback output: {result_fallback!r}\n"
            f"Direct pt-BR output: {result_direct!r}"
        )
