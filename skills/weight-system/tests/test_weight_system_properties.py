"""
Property-based tests for the Weight System.

**Validates: Requirements 3.1, 3.2, 3.3, 4.2**

Properties tested:
  - Property 5: current_weight sempre permanece no intervalo [0.0, 1.0] (Req 3.2, 4.2)
  - Property 6: Cálculo de prioridade segue a fórmula especificada (Req 3.1)
  - Property 7: Ajuste de peso é persistido e registrado (Req 3.3)
"""

from __future__ import annotations

from hypothesis import given, settings
from hypothesis import strategies as st
from weight_system import adjust_weight, calculate_priority
from weight_system_helpers import InMemoryWeightAdapter

# ---------------------------------------------------------------------------
# Hypothesis strategies
# ---------------------------------------------------------------------------

_weight = st.floats(min_value=0.0, max_value=1.0, allow_nan=False, allow_infinity=False)

_delta = st.floats(
    min_value=-2.0, max_value=2.0, allow_nan=False, allow_infinity=False
)

_deadline = st.one_of(
    st.none(),
    st.floats(min_value=0.0, max_value=200.0, allow_nan=False, allow_infinity=False),
)

_slug = st.text(
    alphabet="abcdefghijklmnopqrstuvwxyz0123456789-",
    min_size=1,
    max_size=30,
)

_justification = st.text(min_size=1, max_size=100)

# A sequence of (delta, justification) pairs representing a series of adjustments
_adjustment_sequence = st.lists(
    st.tuples(_delta, _justification),
    min_size=1,
    max_size=20,
)


# ---------------------------------------------------------------------------
# Property 5 — current_weight sempre permanece no intervalo [0.0, 1.0]
# Validates: Requirements 3.2, 4.2
# ---------------------------------------------------------------------------


@given(
    initial_weight=_weight,
    adjustments=_adjustment_sequence,
)
@settings(max_examples=200)
def test_property5_current_weight_always_in_unit_interval(
    initial_weight: float,
    adjustments: list[tuple[float, str]],
) -> None:
    """
    **Property 5: current_weight sempre permanece no intervalo [0.0, 1.0]**

    For any persona and any sequence of weight adjustment operations
    (crisis boost, manual adjustment, decay), the resulting current_weight
    must always remain in [0.0, 1.0].

    This includes the specific crisis boost case: min(current_weight + 0.3, 1.0).

    **Validates: Requirements 3.2, 4.2**
    """
    slug = "test-persona"
    adapter = InMemoryWeightAdapter(initial_weights={slug: initial_weight})

    for delta, justification in adjustments:
        new_weight = adjust_weight(
            persona_slug=slug,
            delta=delta,
            justification=justification,
            persistence=adapter,
        )

        assert 0.0 <= new_weight <= 1.0, (
            f"adjust_weight returned {new_weight} which is outside [0.0, 1.0]. "
            f"delta={delta}, previous weight before this step was "
            f"{adapter.get_current_weight(slug)}"
        )

        stored = adapter.get_current_weight(slug)
        assert 0.0 <= stored <= 1.0, (
            f"Stored weight {stored} is outside [0.0, 1.0] after delta={delta}"
        )


@given(initial_weight=_weight)
@settings(max_examples=100)
def test_property5_crisis_boost_stays_in_unit_interval(
    initial_weight: float,
) -> None:
    """
    **Property 5 (crisis boost variant): min(current_weight + 0.3, 1.0)**

    Applying the crisis boost delta of +0.3 must never produce a weight
    outside [0.0, 1.0].

    **Validates: Requirements 4.2**
    """
    slug = "crisis-persona"
    adapter = InMemoryWeightAdapter(initial_weights={slug: initial_weight})

    new_weight = adjust_weight(
        persona_slug=slug,
        delta=0.3,
        justification="Crisis boost",
        persistence=adapter,
    )

    assert 0.0 <= new_weight <= 1.0
    assert new_weight == min(initial_weight + 0.3, 1.0)


# ---------------------------------------------------------------------------
# Property 6 — Cálculo de prioridade segue a fórmula especificada
# Validates: Requirements 3.1
# ---------------------------------------------------------------------------


@given(
    current_weight=_weight,
    deep_work=st.booleans(),
    deadline_hours=_deadline,
)
@settings(max_examples=300)
def test_property6_priority_follows_specified_formula(
    current_weight: float,
    deep_work: bool,
    deadline_hours: float | None,
) -> None:
    """
    **Property 6: Cálculo de prioridade segue a fórmula especificada**

    For any combination of current_weight and deadline/deep_work flags,
    the calculated priority must be exactly:

        current_weight
        + 0.1  * deep_work
        + 0.2  * (deadline <= 4h)
        + 0.15 * (deadline <= 12h)
        + 0.1  * (deadline <= 24h)
        + 0.05 * (deadline <= 48h)

    clamped to [0.0, 1.0].

    **Validates: Requirements 3.1**
    """
    result = calculate_priority(current_weight, deep_work, deadline_hours)

    # Compute expected value according to the spec formula
    expected = current_weight
    if deep_work:
        expected += 0.1
    if deadline_hours is not None:
        if deadline_hours <= 4:
            expected += 0.2
        if deadline_hours <= 12:
            expected += 0.15
        if deadline_hours <= 24:
            expected += 0.1
        if deadline_hours <= 48:
            expected += 0.05
    expected = max(0.0, min(1.0, expected))

    assert abs(result - expected) < 1e-9, (
        f"calculate_priority({current_weight}, {deep_work}, {deadline_hours}) "
        f"returned {result}, expected {expected}"
    )

    # Result must always be in [0.0, 1.0]
    assert 0.0 <= result <= 1.0, (
        f"Priority {result} is outside [0.0, 1.0]"
    )


@given(
    current_weight=_weight,
    deep_work=st.booleans(),
)
@settings(max_examples=100)
def test_property6_no_deadline_ignores_deadline_bonuses(
    current_weight: float,
    deep_work: bool,
) -> None:
    """
    **Property 6 (no-deadline variant)**

    When deadline_hours is None, no deadline bonus is applied.

    **Validates: Requirements 3.1**
    """
    result = calculate_priority(current_weight, deep_work, deadline_hours=None)
    expected = max(0.0, min(1.0, current_weight + (0.1 if deep_work else 0.0)))

    assert abs(result - expected) < 1e-9


# ---------------------------------------------------------------------------
# Property 7 — Ajuste de peso é persistido e registrado
# Validates: Requirements 3.3
# ---------------------------------------------------------------------------


@given(
    initial_weight=_weight,
    delta=_delta,
    justification=_justification,
)
@settings(max_examples=200)
def test_property7_weight_adjustment_is_persisted(
    initial_weight: float,
    delta: float,
    justification: str,
) -> None:
    """
    **Property 7: Ajuste de peso é persistido e registrado**

    For any weight adjustment, the new value must be persisted (set_current_weight
    called) and a history entry with timestamp and justification must be appended
    (append_weight_history called).

    **Validates: Requirements 3.3**
    """
    slug = "persist-persona"
    adapter = InMemoryWeightAdapter(initial_weights={slug: initial_weight})

    new_weight = adjust_weight(
        persona_slug=slug,
        delta=delta,
        justification=justification,
        persistence=adapter,
    )

    # 1. New weight must be persisted in the store
    stored = adapter.get_current_weight(slug)
    assert stored == new_weight, (
        f"Persisted weight {stored} does not match returned weight {new_weight}"
    )

    # 2. A history entry must have been appended
    history = adapter.get_history(slug)
    assert len(history) == 1, (
        f"Expected 1 history entry, got {len(history)}"
    )

    entry = history[0]

    # 3. History entry must record old and new weights correctly
    assert entry.old_weight == initial_weight, (
        f"History entry old_weight={entry.old_weight}, expected {initial_weight}"
    )
    assert entry.new_weight == new_weight, (
        f"History entry new_weight={entry.new_weight}, expected {new_weight}"
    )

    # 4. History entry must contain the justification
    assert entry.justification == justification, (
        f"History entry justification={entry.justification!r}, expected {justification!r}"
    )

    # 5. History entry must have a timestamp
    assert entry.timestamp is not None, "History entry must have a timestamp"


@given(
    initial_weight=_weight,
    adjustments=_adjustment_sequence,
)
@settings(max_examples=100)
def test_property7_each_adjustment_appends_one_history_entry(
    initial_weight: float,
    adjustments: list[tuple[float, str]],
) -> None:
    """
    **Property 7 (history accumulation variant)**

    After N adjustments, the history must contain exactly N entries —
    one per adjustment, in order.

    **Validates: Requirements 3.3**
    """
    slug = "history-persona"
    adapter = InMemoryWeightAdapter(initial_weights={slug: initial_weight})

    for i, (delta, justification) in enumerate(adjustments):
        adjust_weight(
            persona_slug=slug,
            delta=delta,
            justification=justification,
            persistence=adapter,
        )

        history = adapter.get_history(slug)
        assert len(history) == i + 1, (
            f"After {i + 1} adjustments, expected {i + 1} history entries, "
            f"got {len(history)}"
        )

        # The latest entry must match the justification just applied
        assert history[-1].justification == justification
