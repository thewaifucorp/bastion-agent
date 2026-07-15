"""
Property-based tests for bastion/self-improving.

**Validates: Requirements 12.1, 12.2, 12.4, 12.6**

Properties tested:
  - Property 30: Padrão com 3+ ocorrências em 7 dias é promovido para HOT (Req 12.1)
  - Property 31: Persona com peso < 0.3 não promove padrões para HOT global (Req 12.2)
  - Property 32: Conflict resolution segue ordem de precedência especificada (Req 12.4)
  - Property 33: Isolamento completo entre namespaces de personas (Req 12.6)
"""

from __future__ import annotations

from datetime import datetime, timedelta, timezone

from hypothesis import given, settings, assume
from hypothesis import strategies as st

from promotion import (
    MemoryTier,
    Pattern,
    conflict_resolution,
    promote_pattern,
    should_promote,
    PROMOTION_MIN_OCCURRENCES,
    PROMOTION_WINDOW_DAYS,
    MIN_WEIGHT_FOR_GLOBAL_HOT,
)
from self_improving_helpers import InMemoryPromotionAdapter

# ---------------------------------------------------------------------------
# Hypothesis strategies
# ---------------------------------------------------------------------------

_slug = st.text(
    alphabet="abcdefghijklmnopqrstuvwxyz0123456789-",
    min_size=1,
    max_size=20,
)

_pattern_id = st.text(
    alphabet="abcdefghijklmnopqrstuvwxyz0123456789-",
    min_size=1,
    max_size=20,
)

_description = st.text(min_size=1, max_size=80)

_weight = st.floats(min_value=0.0, max_value=1.0, allow_nan=False, allow_infinity=False)

_weight_below_gate = st.floats(
    min_value=0.0, max_value=MIN_WEIGHT_FOR_GLOBAL_HOT - 1e-9,
    allow_nan=False, allow_infinity=False,
)

_weight_above_gate = st.floats(
    min_value=MIN_WEIGHT_FOR_GLOBAL_HOT, max_value=1.0,
    allow_nan=False, allow_infinity=False,
)

_specificity = st.integers(min_value=0, max_value=100)

_datetime_recent = st.datetimes(
    min_value=datetime(2020, 1, 1),
    max_value=datetime(2030, 1, 1),
    timezones=st.just(timezone.utc),
)


def _make_pattern(
    slug: str,
    pattern_id: str,
    specificity: int,
    persona_weight: float,
    occurrences: list[datetime],
    tier: MemoryTier = MemoryTier.WARM,
    updated_at: datetime | None = None,
) -> Pattern:
    now = datetime.now(tz=timezone.utc)
    return Pattern(
        id=pattern_id,
        persona_slug=slug,
        description="test pattern",
        tier=tier,
        specificity=specificity,
        persona_weight=persona_weight,
        occurrences=occurrences,
        updated_at=updated_at or now,
    )


def _recent_occurrences(count: int) -> list[datetime]:
    """Generate *count* occurrences within the last 7 days."""
    now = datetime.now(tz=timezone.utc)
    return [now - timedelta(days=i) for i in range(count)]


def _old_occurrences(count: int) -> list[datetime]:
    """Generate *count* occurrences older than 7 days."""
    now = datetime.now(tz=timezone.utc)
    return [now - timedelta(days=PROMOTION_WINDOW_DAYS + 1 + i) for i in range(count)]


# ---------------------------------------------------------------------------
# Property 30 — Padrão com 3+ ocorrências em 7 dias é promovido para HOT
# Validates: Requirements 12.1
# ---------------------------------------------------------------------------


@given(
    slug=_slug,
    pattern_id=_pattern_id,
    extra_occurrences=st.integers(min_value=0, max_value=10),
    current_weight=_weight_above_gate,
)
@settings(max_examples=200)
def test_property30_pattern_with_3_plus_occurrences_promoted_to_hot(
    slug: str,
    pattern_id: str,
    extra_occurrences: int,
    current_weight: float,
) -> None:
    """
    **Property 30: Padrão com 3+ ocorrências em 7 dias é promovido para HOT**

    For any pattern observed PROMOTION_MIN_OCCURRENCES or more times within
    the last PROMOTION_WINDOW_DAYS days, and for a persona with
    current_weight >= MIN_WEIGHT_FOR_GLOBAL_HOT, promote_pattern() must
    promote the pattern to HOT tier.

    **Validates: Requirements 12.1**
    """
    count = PROMOTION_MIN_OCCURRENCES + extra_occurrences
    occurrences = _recent_occurrences(count)

    pattern = _make_pattern(slug, pattern_id, specificity=1, persona_weight=current_weight, occurrences=occurrences)
    adapter = InMemoryPromotionAdapter(weights={slug: current_weight})

    promoted = promote_pattern(pattern, adapter, is_crisis=False)

    assert promoted is True, (
        f"Expected promotion with {count} recent occurrences and weight={current_weight:.4f}, "
        f"but promote_pattern returned False"
    )

    saved = adapter.get_saved_pattern(slug, pattern_id)
    assert saved is not None, "Pattern was not saved after promotion"
    assert saved.tier == MemoryTier.HOT, (
        f"Expected tier=HOT after promotion, got {saved.tier}"
    )

    history = adapter.get_history(slug)
    assert len(history) == 1, f"Expected 1 history entry after promotion, got {len(history)}"
    assert "HOT" in history[0][2], f"History action should mention HOT: {history[0][2]}"


@given(
    slug=_slug,
    pattern_id=_pattern_id,
    count_below=st.integers(min_value=0, max_value=PROMOTION_MIN_OCCURRENCES - 1),
    current_weight=_weight_above_gate,
)
@settings(max_examples=100)
def test_property30_pattern_below_threshold_not_promoted(
    slug: str,
    pattern_id: str,
    count_below: int,
    current_weight: float,
) -> None:
    """
    **Property 30 (negative case): Fewer than 3 occurrences → not promoted**

    **Validates: Requirements 12.1**
    """
    occurrences = _recent_occurrences(count_below)
    pattern = _make_pattern(slug, pattern_id, specificity=1, persona_weight=current_weight, occurrences=occurrences)
    adapter = InMemoryPromotionAdapter(weights={slug: current_weight})

    promoted = promote_pattern(pattern, adapter, is_crisis=False)

    assert promoted is False, (
        f"Expected no promotion with only {count_below} recent occurrences, "
        f"but promote_pattern returned True"
    )


@given(
    slug=_slug,
    pattern_id=_pattern_id,
    old_count=st.integers(min_value=3, max_value=10),
    current_weight=_weight_above_gate,
)
@settings(max_examples=100)
def test_property30_old_occurrences_outside_window_not_counted(
    slug: str,
    pattern_id: str,
    old_count: int,
    current_weight: float,
) -> None:
    """
    **Property 30 (window variant): Occurrences outside 7-day window don't count**

    **Validates: Requirements 12.1**
    """
    occurrences = _old_occurrences(old_count)
    pattern = _make_pattern(slug, pattern_id, specificity=1, persona_weight=current_weight, occurrences=occurrences)
    adapter = InMemoryPromotionAdapter(weights={slug: current_weight})

    promoted = promote_pattern(pattern, adapter, is_crisis=False)

    assert promoted is False, (
        f"Expected no promotion: {old_count} occurrences are all outside the 7-day window"
    )


# ---------------------------------------------------------------------------
# Property 31 — Persona com peso < 0.3 não promove padrões para HOT global
# Validates: Requirements 12.2
# ---------------------------------------------------------------------------


@given(
    slug=_slug,
    pattern_id=_pattern_id,
    extra_occurrences=st.integers(min_value=0, max_value=10),
    current_weight=_weight_below_gate,
)
@settings(max_examples=200)
def test_property31_low_weight_persona_blocked_from_hot_global(
    slug: str,
    pattern_id: str,
    extra_occurrences: int,
    current_weight: float,
) -> None:
    """
    **Property 31: Persona com peso < 0.3 não promove padrões para HOT global**

    For any persona with current_weight < MIN_WEIGHT_FOR_GLOBAL_HOT (0.3),
    no pattern should be promoted to HOT global, regardless of occurrence count.

    **Validates: Requirements 12.2**
    """
    count = PROMOTION_MIN_OCCURRENCES + extra_occurrences
    occurrences = _recent_occurrences(count)

    pattern = _make_pattern(slug, pattern_id, specificity=1, persona_weight=current_weight, occurrences=occurrences)
    adapter = InMemoryPromotionAdapter(weights={slug: current_weight})

    promoted = promote_pattern(pattern, adapter, is_crisis=False)

    assert promoted is False, (
        f"Persona with weight={current_weight:.4f} (< {MIN_WEIGHT_FOR_GLOBAL_HOT}) "
        f"should NOT promote patterns to HOT global, but promote_pattern returned True"
    )

    saved = adapter.get_saved_pattern(slug, pattern_id)
    assert saved is None or saved.tier != MemoryTier.HOT, (
        f"Pattern tier should not be HOT for low-weight persona (weight={current_weight:.4f})"
    )


@given(
    slug=_slug,
    pattern_id=_pattern_id,
    extra_occurrences=st.integers(min_value=0, max_value=10),
    current_weight=_weight_below_gate,
)
@settings(max_examples=100)
def test_property31_crisis_bypasses_weight_gate(
    slug: str,
    pattern_id: str,
    extra_occurrences: int,
    current_weight: float,
) -> None:
    """
    **Property 31 (crisis override): Crisis bypasses weight gate**

    When is_crisis=True, even a persona with weight < 0.3 can promote
    patterns to HOT (crisis priority, Requirement 12.3).

    **Validates: Requirements 12.2, 12.3**
    """
    count = PROMOTION_MIN_OCCURRENCES + extra_occurrences
    occurrences = _recent_occurrences(count)

    pattern = _make_pattern(slug, pattern_id, specificity=1, persona_weight=current_weight, occurrences=occurrences)
    adapter = InMemoryPromotionAdapter(weights={slug: current_weight})

    promoted = promote_pattern(pattern, adapter, is_crisis=True)

    assert promoted is True, (
        f"Crisis mode should bypass weight gate (weight={current_weight:.4f}), "
        f"but promote_pattern returned False"
    )


# ---------------------------------------------------------------------------
# Property 32 — Conflict resolution segue ordem de precedência especificada
# Validates: Requirements 12.4
# ---------------------------------------------------------------------------


@given(
    slug_a=_slug,
    slug_b=_slug,
    id_a=_pattern_id,
    id_b=_pattern_id,
    spec_a=st.integers(min_value=1, max_value=100),
    spec_b=st.integers(min_value=1, max_value=100),
    weight_a=_weight,
    weight_b=_weight,
)
@settings(max_examples=300)
def test_property32_conflict_resolution_specificity_first(
    slug_a: str,
    slug_b: str,
    id_a: str,
    id_b: str,
    spec_a: int,
    spec_b: int,
    weight_a: float,
    weight_b: float,
) -> None:
    """
    **Property 32: Conflict resolution — mais específico vence primeiro**

    When two patterns have different specificity values, the more specific
    one (higher specificity) must always win, regardless of recency or weight.

    **Validates: Requirements 12.4**
    """
    assume(spec_a != spec_b)

    now = datetime.now(tz=timezone.utc)
    pattern_a = _make_pattern(slug_a, id_a, specificity=spec_a, persona_weight=weight_a, occurrences=[], updated_at=now)
    pattern_b = _make_pattern(slug_b, id_b, specificity=spec_b, persona_weight=weight_b, occurrences=[], updated_at=now)

    winner = conflict_resolution(pattern_a, pattern_b)

    expected_winner = pattern_a if spec_a > spec_b else pattern_b
    assert winner is expected_winner, (
        f"Expected winner with specificity={max(spec_a, spec_b)}, "
        f"got winner with specificity={winner.specificity}"
    )


@given(
    slug_a=_slug,
    slug_b=_slug,
    id_a=_pattern_id,
    id_b=_pattern_id,
    spec=st.integers(min_value=0, max_value=100),
    weight_a=_weight,
    weight_b=_weight,
    delta_seconds=st.integers(min_value=1, max_value=86400),
)
@settings(max_examples=200)
def test_property32_conflict_resolution_recency_second(
    slug_a: str,
    slug_b: str,
    id_a: str,
    id_b: str,
    spec: int,
    weight_a: float,
    weight_b: float,
    delta_seconds: int,
) -> None:
    """
    **Property 32: Conflict resolution — mais recente vence quando especificidade empata**

    When specificity is equal, the more recently updated pattern must win.

    **Validates: Requirements 12.4**
    """
    now = datetime.now(tz=timezone.utc)
    older = now - timedelta(seconds=delta_seconds)

    pattern_a = _make_pattern(slug_a, id_a, specificity=spec, persona_weight=weight_a, occurrences=[], updated_at=now)
    pattern_b = _make_pattern(slug_b, id_b, specificity=spec, persona_weight=weight_b, occurrences=[], updated_at=older)

    winner = conflict_resolution(pattern_a, pattern_b)

    assert winner is pattern_a, (
        f"Expected more recent pattern_a (updated_at={now.isoformat()}) to win, "
        f"but got pattern_b (updated_at={older.isoformat()})"
    )


@given(
    slug_a=_slug,
    slug_b=_slug,
    id_a=_pattern_id,
    id_b=_pattern_id,
    spec=st.integers(min_value=0, max_value=100),
    weight_a=_weight,
    weight_b=_weight,
)
@settings(max_examples=200)
def test_property32_conflict_resolution_weight_third(
    slug_a: str,
    slug_b: str,
    id_a: str,
    id_b: str,
    spec: int,
    weight_a: float,
    weight_b: float,
) -> None:
    """
    **Property 32: Conflict resolution — maior peso vence quando especificidade e recência empatam**

    When specificity and updated_at are equal, the pattern with higher
    persona_weight must win.

    **Validates: Requirements 12.4**
    """
    assume(abs(weight_a - weight_b) > 1e-9)

    now = datetime.now(tz=timezone.utc)
    pattern_a = _make_pattern(slug_a, id_a, specificity=spec, persona_weight=weight_a, occurrences=[], updated_at=now)
    pattern_b = _make_pattern(slug_b, id_b, specificity=spec, persona_weight=weight_b, occurrences=[], updated_at=now)

    winner = conflict_resolution(pattern_a, pattern_b)

    expected_winner = pattern_a if weight_a > weight_b else pattern_b
    assert winner is expected_winner, (
        f"Expected winner with persona_weight={max(weight_a, weight_b):.4f}, "
        f"got winner with persona_weight={winner.persona_weight:.4f}"
    )


@given(
    slug_a=_slug,
    slug_b=_slug,
    id_a=_pattern_id,
    id_b=_pattern_id,
    spec=st.integers(min_value=0, max_value=100),
    weight=_weight,
)
@settings(max_examples=100)
def test_property32_conflict_resolution_tie_returns_pattern_a(
    slug_a: str,
    slug_b: str,
    id_a: str,
    id_b: str,
    spec: int,
    weight: float,
) -> None:
    """
    **Property 32 (tie variant): Complete tie → pattern_a wins (deterministic)**

    **Validates: Requirements 12.4**
    """
    now = datetime.now(tz=timezone.utc)
    pattern_a = _make_pattern(slug_a, id_a, specificity=spec, persona_weight=weight, occurrences=[], updated_at=now)
    pattern_b = _make_pattern(slug_b, id_b, specificity=spec, persona_weight=weight, occurrences=[], updated_at=now)

    winner = conflict_resolution(pattern_a, pattern_b)

    assert winner is pattern_a, (
        "On a complete tie, pattern_a should win (stable, deterministic)"
    )


# ---------------------------------------------------------------------------
# Property 33 — Isolamento completo entre namespaces de personas
# Validates: Requirements 12.6
# ---------------------------------------------------------------------------


@given(
    slug_a=_slug,
    slug_b=_slug,
    pattern_id=_pattern_id,
    weight_a=_weight_above_gate,
    extra_occurrences=st.integers(min_value=0, max_value=5),
)
@settings(max_examples=200)
def test_property33_namespace_isolation_write_to_a_does_not_affect_b(
    slug_a: str,
    slug_b: str,
    pattern_id: str,
    weight_a: float,
    extra_occurrences: int,
) -> None:
    """
    **Property 33: Isolamento completo entre namespaces de personas**

    For any two distinct personas A and B, any write operation on
    personas/{slug-a}/ must not modify anything in personas/{slug-b}/.

    **Validates: Requirements 12.6**
    """
    assume(slug_a != slug_b)

    count = PROMOTION_MIN_OCCURRENCES + extra_occurrences
    occurrences = _recent_occurrences(count)

    pattern_a = _make_pattern(slug_a, pattern_id, specificity=1, persona_weight=weight_a, occurrences=occurrences)
    adapter = InMemoryPromotionAdapter(weights={slug_a: weight_a, slug_b: 0.5})

    # Snapshot of slug_b state before any operation on slug_a
    history_b_before = adapter.get_history(slug_b)
    pattern_b_before = adapter.get_saved_pattern(slug_b, pattern_id)

    # Perform operations on slug_a
    promote_pattern(pattern_a, adapter, is_crisis=False)

    # slug_b state must be unchanged
    history_b_after = adapter.get_history(slug_b)
    pattern_b_after = adapter.get_saved_pattern(slug_b, pattern_id)

    assert history_b_after == history_b_before, (
        f"History of persona '{slug_b}' was modified by an operation on '{slug_a}'. "
        f"Before: {history_b_before}, After: {history_b_after}"
    )
    assert pattern_b_after == pattern_b_before, (
        f"Pattern in persona '{slug_b}' was modified by an operation on '{slug_a}'"
    )


@given(
    slug_a=_slug,
    slug_b=_slug,
    id_a=_pattern_id,
    id_b=_pattern_id,
    weight_a=_weight_above_gate,
    weight_b=_weight_above_gate,
    extra_occurrences=st.integers(min_value=0, max_value=5),
)
@settings(max_examples=100)
def test_property33_independent_promotion_histories(
    slug_a: str,
    slug_b: str,
    id_a: str,
    id_b: str,
    weight_a: float,
    weight_b: float,
    extra_occurrences: int,
) -> None:
    """
    **Property 33 (independent histories): Each persona has its own history**

    Promoting a pattern for persona A must only append to A's history,
    not to B's history.

    **Validates: Requirements 12.6**
    """
    assume(slug_a != slug_b)

    count = PROMOTION_MIN_OCCURRENCES + extra_occurrences
    occurrences = _recent_occurrences(count)

    pattern_a = _make_pattern(slug_a, id_a, specificity=1, persona_weight=weight_a, occurrences=occurrences)
    pattern_b = _make_pattern(slug_b, id_b, specificity=1, persona_weight=weight_b, occurrences=occurrences)

    adapter = InMemoryPromotionAdapter(weights={slug_a: weight_a, slug_b: weight_b})

    promote_pattern(pattern_a, adapter, is_crisis=False)
    promote_pattern(pattern_b, adapter, is_crisis=False)

    history_a = adapter.get_history(slug_a)
    history_b = adapter.get_history(slug_b)

    # Each persona must have exactly its own history entry
    assert len(history_a) == 1, (
        f"Expected 1 history entry for '{slug_a}', got {len(history_a)}"
    )
    assert len(history_b) == 1, (
        f"Expected 1 history entry for '{slug_b}', got {len(history_b)}"
    )

    # Verify the history entries belong to the correct persona
    assert history_a[0][1] == id_a, (
        f"History entry for '{slug_a}' has wrong pattern_id: {history_a[0][1]}"
    )
    assert history_b[0][1] == id_b, (
        f"History entry for '{slug_b}' has wrong pattern_id: {history_b[0][1]}"
    )
