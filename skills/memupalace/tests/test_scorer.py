"""Property-based tests for salience_score().

Properties covered:
- P9:  Salience Formula Correctness       (Validates: Requirements 6.1, 6.5)
- P12: Salience Monotone Increasing in Reinforcement Count (Validates: Requirements 6.6)
- P13: Salience Monotone Decreasing in Days Ago            (Validates: Requirements 6.6)
"""

from __future__ import annotations

import math

import pytest
from hypothesis import given, settings
from hypothesis import strategies as st

from scorer import salience_score


# ---------------------------------------------------------------------------
# Property 9: Salience Formula Correctness
# Validates: Requirements 6.1, 6.5
# ---------------------------------------------------------------------------


@given(
    similarity=st.floats(min_value=0.0, max_value=1.0, allow_nan=False),
    reinforcement_count=st.integers(min_value=0, max_value=1000),
    days_ago=st.floats(min_value=0.0, max_value=3650.0, allow_nan=False),
    recency_decay_days=st.integers(min_value=1, max_value=365),
)
@settings(max_examples=100)
def test_salience_formula_correctness(
    similarity: float,
    reinforcement_count: int,
    days_ago: float,
    recency_decay_days: int,
) -> None:
    """Property 9: salience_score matches the formula exactly for all valid inputs.

    Validates: Requirements 6.1, 6.5
    """
    expected_rf = max(1.0, math.log(reinforcement_count + 1))
    expected_rec = math.exp(-0.693 * days_ago / recency_decay_days)
    expected = similarity * expected_rf * expected_rec

    result = salience_score(similarity, reinforcement_count, days_ago, recency_decay_days)

    assert abs(result - expected) < 1e-12, (
        f"Formula mismatch: got {result}, expected {expected} "
        f"(sim={similarity}, count={reinforcement_count}, days={days_ago}, decay={recency_decay_days})"
    )


def test_salience_base_case_equals_similarity() -> None:
    """Property 9 special case: count=0, days_ago=0 → score == similarity exactly.

    Validates: Requirements 6.1, 6.5
    """
    for sim in [0.0, 0.25, 0.5, 0.75, 1.0]:
        result = salience_score(sim, reinforcement_count=0, days_ago=0.0)
        assert result == sim, f"Base case failed: expected {sim}, got {result}"


# ---------------------------------------------------------------------------
# Property 12: Salience Monotone Increasing in Reinforcement Count
# Validates: Requirements 6.6
# ---------------------------------------------------------------------------


@given(
    similarity=st.floats(min_value=0.0, max_value=1.0, allow_nan=False),
    count=st.integers(min_value=0, max_value=999),
    days_ago=st.floats(min_value=0.0, max_value=365.0, allow_nan=False),
    decay=st.integers(min_value=1, max_value=365),
)
@settings(max_examples=100)
def test_salience_monotone_increasing_reinforcement(
    similarity: float,
    count: int,
    days_ago: float,
    decay: int,
) -> None:
    """Property 12: salience_score(sim, count+1, ...) >= salience_score(sim, count, ...).

    Validates: Requirements 6.6
    """
    score_base = salience_score(similarity, count, days_ago, decay)
    score_more = salience_score(similarity, count + 1, days_ago, decay)
    assert score_more >= score_base, (
        f"Monotonicity violated: score({count+1})={score_more} < score({count})={score_base} "
        f"(sim={similarity}, days={days_ago}, decay={decay})"
    )


# ---------------------------------------------------------------------------
# Property 13: Salience Monotone Decreasing in Days Ago
# Validates: Requirements 6.6
# ---------------------------------------------------------------------------


@given(
    similarity=st.floats(min_value=0.0, max_value=1.0, allow_nan=False),
    count=st.integers(min_value=0, max_value=1000),
    d1=st.floats(min_value=0.0, max_value=1825.0, allow_nan=False),
    d2=st.floats(min_value=0.0, max_value=1825.0, allow_nan=False),
    decay=st.integers(min_value=1, max_value=365),
)
@settings(max_examples=100)
def test_salience_monotone_decreasing_days_ago(
    similarity: float,
    count: int,
    d1: float,
    d2: float,
    decay: int,
) -> None:
    """Property 13: when d1 <= d2, salience_score(..., d1, ...) >= salience_score(..., d2, ...).

    Validates: Requirements 6.6
    """
    lo, hi = (d1, d2) if d1 <= d2 else (d2, d1)
    score_recent = salience_score(similarity, count, lo, decay)
    score_older = salience_score(similarity, count, hi, decay)
    assert score_recent >= score_older, (
        f"Monotonicity violated: score(d={lo})={score_recent} < score(d={hi})={score_older} "
        f"(sim={similarity}, count={count}, decay={decay})"
    )
