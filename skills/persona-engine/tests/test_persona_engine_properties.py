"""
Property-based tests for the Persona Engine.

**Validates: Requirements 1.2, 2.2, 2.3, 2.4, 2.6**

Properties tested:
  - Property 1: Onboarding cria N personas para N áreas informadas (Req 1.2)
  - Property 2: Persona criada contém todos os campos obrigatórios no SOUL.md (Req 2.2)
  - Property 3: Persona matching ativa todas as personas com keywords correspondentes (Req 2.3, 2.4)
  - Property 4: Fallback usa persona de maior current_weight (Req 2.6)
"""

from __future__ import annotations

import pytest
from hypothesis import given, settings, assume
from hypothesis import strategies as st

from persona_engine import (
    ActivePersona,
    Persona,
    create_persona,
    match_personas,
)
from persona_engine_helpers import InMemoryPersistence

# ---------------------------------------------------------------------------
# Hypothesis strategies
# ---------------------------------------------------------------------------

# Non-empty, printable text (avoids empty strings that would produce empty slugs)
_text = st.text(
    alphabet=st.characters(whitelist_categories=("Lu", "Ll", "Nd", "Zs")),
    min_size=1,
    max_size=30,
)

_keyword = st.text(
    alphabet=st.characters(whitelist_categories=("Lu", "Ll", "Nd")),
    min_size=2,
    max_size=20,
)

_weight = st.floats(min_value=0.0, max_value=1.0, allow_nan=False, allow_infinity=False)


def _persona_strategy(
    weight_strategy: st.SearchStrategy[float] | None = None,
) -> st.SearchStrategy[Persona]:
    """Strategy that generates valid Persona instances."""
    w = weight_strategy or _weight
    return st.builds(
        Persona,
        name=_text,
        slug=_keyword,
        base_weight=w,
        current_weight=w,
        domains=st.lists(_text, min_size=1, max_size=5),
        trigger_keywords=st.lists(_keyword, min_size=1, max_size=8),
        clawhub_skills=st.lists(_keyword, min_size=0, max_size=4),
    )


# ---------------------------------------------------------------------------
# Property 1 — Onboarding cria N personas para N áreas informadas
# Validates: Requirements 1.2
# ---------------------------------------------------------------------------


@given(
    areas=st.lists(
        st.tuples(
            _text,                                          # name
            st.lists(_text, min_size=1, max_size=3),       # domains
            st.lists(_keyword, min_size=1, max_size=5),    # trigger_keywords
            st.lists(_keyword, min_size=0, max_size=3),    # clawhub_skills
            _weight,                                        # base_weight
        ),
        min_size=1,
        max_size=10,
    )
)
@settings(max_examples=50)
def test_property1_onboarding_creates_n_personas_for_n_areas(
    areas: list[tuple],
) -> None:
    """
    **Property 1: Onboarding cria N personas para N áreas informadas**

    For any list of N life areas provided during onboarding, the system must
    create exactly N personas — one per area.

    **Validates: Requirements 1.2**
    """
    persistence = InMemoryPersistence()

    created: list[Persona] = []
    for name, domains, keywords, skills, weight in areas:
        persona = create_persona(
            name=name,
            domains=domains,
            trigger_keywords=keywords,
            clawhub_skills=skills,
            base_weight=weight,
            persistence=persistence,
        )
        created.append(persona)

    # Exactly N personas must have been created
    assert len(created) == len(areas), (
        f"Expected {len(areas)} personas, got {len(created)}"
    )

    # Each area maps to exactly one persona (one-to-one)
    assert len(persistence.all_personas) == len(areas), (
        "Persistence must contain exactly N personas"
    )


# ---------------------------------------------------------------------------
# Property 2 — Persona criada contém todos os campos obrigatórios no SOUL.md
# Validates: Requirements 2.2
# ---------------------------------------------------------------------------

REQUIRED_SOUL_FIELDS = ("name", "slug", "base_weight", "domains", "trigger_keywords", "clawhub_skills")


@given(
    name=_text,
    domains=st.lists(_text, min_size=1, max_size=5),
    trigger_keywords=st.lists(_keyword, min_size=1, max_size=8),
    clawhub_skills=st.lists(_keyword, min_size=0, max_size=4),
    base_weight=_weight,
)
@settings(max_examples=50)
def test_property2_created_persona_has_all_required_soul_fields(
    name: str,
    domains: list[str],
    trigger_keywords: list[str],
    clawhub_skills: list[str],
    base_weight: float,
) -> None:
    """
    **Property 2: Persona criada contém todos os campos obrigatórios no SOUL.md**

    For any persona created by Persona_Engine, the SOUL.md frontmatter must
    contain all required fields: name, slug, base_weight, domains,
    trigger_keywords, and clawhub_skills.

    **Validates: Requirements 2.2**
    """
    persistence = InMemoryPersistence()

    persona = create_persona(
        name=name,
        domains=domains,
        trigger_keywords=trigger_keywords,
        clawhub_skills=clawhub_skills,
        base_weight=base_weight,
        persistence=persistence,
    )

    # Verify all required fields are present and non-None
    for field_name in REQUIRED_SOUL_FIELDS:
        value = getattr(persona, field_name, None)
        assert value is not None, f"Required field '{field_name}' is None"

    # Verify field types match the spec
    assert isinstance(persona.name, str) and persona.name, "name must be a non-empty string"
    assert isinstance(persona.slug, str) and persona.slug, "slug must be a non-empty string"
    assert isinstance(persona.base_weight, float), "base_weight must be a float"
    assert 0.0 <= persona.base_weight <= 1.0, "base_weight must be in [0.0, 1.0]"
    assert isinstance(persona.domains, list), "domains must be a list"
    assert isinstance(persona.trigger_keywords, list), "trigger_keywords must be a list"
    assert isinstance(persona.clawhub_skills, list), "clawhub_skills must be a list"

    # Verify the persona was persisted (SOUL.md written)
    stored = persistence.read_soul_md(persona.slug)
    assert stored.name == persona.name
    assert stored.slug == persona.slug


# ---------------------------------------------------------------------------
# Property 3 — Persona matching ativa todas as personas com keywords correspondentes
# Validates: Requirements 2.3, 2.4
# ---------------------------------------------------------------------------


@given(
    personas=st.lists(_persona_strategy(), min_size=2, max_size=6),
    extra_words=st.lists(_text, min_size=0, max_size=3),
)
@settings(max_examples=50)
def test_property3_matching_activates_all_personas_with_matching_keywords(
    personas: list[Persona],
    extra_words: list[str],
) -> None:
    """
    **Property 3: Persona matching ativa todas as personas com keywords correspondentes**

    For any message containing keywords from multiple personas, ALL personas
    with matching keywords must be activated simultaneously.

    **Validates: Requirements 2.3, 2.4**
    """
    # Ensure at least 2 personas have distinct, non-overlapping keywords
    # so we can build a message that matches multiple personas
    assume(len(personas) >= 2)

    # Pick the first keyword from each persona to build a message that
    # guarantees matches for all personas that have keywords
    keywords_in_message = [p.trigger_keywords[0] for p in personas if p.trigger_keywords]
    assume(len(keywords_in_message) >= 2)

    # Build a message that contains at least one keyword from every persona
    message_parts = keywords_in_message + extra_words
    message = " ".join(message_parts)

    active = match_personas(message, personas)

    # Every persona whose keyword appears in the message must be in the result
    for persona in personas:
        if any(kw.lower() in message.lower() for kw in persona.trigger_keywords):
            active_slugs = [ap.persona.slug for ap in active]
            assert persona.slug in active_slugs, (
                f"Persona '{persona.slug}' has a matching keyword but was not activated. "
                f"Message: {message!r}, keywords: {persona.trigger_keywords}"
            )

    # Result must never be empty when there are matches
    assert len(active) >= 1


# ---------------------------------------------------------------------------
# Property 4 — Fallback usa persona de maior current_weight
# Validates: Requirements 2.6
# ---------------------------------------------------------------------------


@given(
    personas=st.lists(
        _persona_strategy(),
        min_size=1,
        max_size=8,
    ),
)
@settings(max_examples=50)
def test_property4_fallback_selects_persona_with_highest_current_weight(
    personas: list[Persona],
) -> None:
    """
    **Property 4: Fallback usa persona de maior current_weight**

    When no persona matches the incoming message, the system must always
    select the persona with the highest current_weight as fallback.

    **Validates: Requirements 2.6**
    """
    # Use a message that is guaranteed to match NO keyword in any persona
    # by using a string that cannot appear in any generated keyword
    no_match_message = "___NO_MATCH_SENTINEL___"

    # Confirm the message truly matches nothing
    assume(
        not any(
            kw.lower() in no_match_message.lower()
            for p in personas
            for kw in p.trigger_keywords
        )
    )

    active = match_personas(no_match_message, personas)

    # Fallback must return exactly one persona
    assert len(active) == 1, f"Fallback must return exactly 1 persona, got {len(active)}"

    fallback_persona = active[0].persona
    max_weight = max(p.current_weight for p in personas)

    assert fallback_persona.current_weight == max_weight, (
        f"Fallback persona has weight {fallback_persona.current_weight}, "
        f"but max weight is {max_weight}"
    )
