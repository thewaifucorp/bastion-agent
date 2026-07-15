"""
Property-based tests for the Crisis Mode skill.

**Validates: Requirements 4.2, 4.3, 4.4**

Properties tested:
  - Property 5: current_weight sempre permanece no intervalo [0.0, 1.0] após crisis boost
  - Property 8: Sacrifice algorithm filtra tarefas pelos critérios corretos
                (movable=True AND priority < crisis_weight * 0.6)
  - Property 9: Sacrifice algorithm libera no mínimo 2 horas de Deep Work
                (quando há tarefas suficientes)
"""

from __future__ import annotations

import sys
from pathlib import Path

# Allow importing crisis_mode from the parent directory
sys.path.insert(0, str(Path(__file__).parent.parent))

from hypothesis import assume, given, settings
from hypothesis import strategies as st

from crisis_mode import (
    CRISIS_DEEP_WORK_TARGET_HOURS,
    CrisisResult,
    SacrificeResult,
    Task,
    detect_crisis,
    sacrifice_algorithm,
)

# ---------------------------------------------------------------------------
# Hypothesis strategies
# ---------------------------------------------------------------------------

_weight = st.floats(
    min_value=0.0,
    max_value=1.0,
    allow_nan=False,
    allow_infinity=False,
)

_priority = st.floats(
    min_value=0.0,
    max_value=1.0,
    allow_nan=False,
    allow_infinity=False,
)

_duration = st.floats(
    min_value=0.1,
    max_value=8.0,
    allow_nan=False,
    allow_infinity=False,
)

_slug = st.text(
    alphabet="abcdefghijklmnopqrstuvwxyz0123456789-",
    min_size=1,
    max_size=30,
)

_task_id = st.text(
    alphabet="abcdefghijklmnopqrstuvwxyz0123456789-",
    min_size=1,
    max_size=20,
)

_task_title = st.text(min_size=1, max_size=80)


def _task_strategy() -> st.SearchStrategy[Task]:
    """Generate a random Task with valid fields."""
    return st.builds(
        Task,
        id=_task_id,
        title=_task_title,
        duration_hours=_duration,
        movable=st.booleans(),
        priority=_priority,
    )


def _task_list() -> st.SearchStrategy[list[Task]]:
    return st.lists(_task_strategy(), min_size=0, max_size=20)


# ---------------------------------------------------------------------------
# Property 5 — current_weight sempre permanece no intervalo [0.0, 1.0]
# Validates: Requirements 4.2
# ---------------------------------------------------------------------------


@given(
    persona_slug=_slug,
    current_weight=_weight,
    tasks=_task_list(),
)
@settings(max_examples=300)
def test_property5_new_weight_always_in_unit_interval(
    persona_slug: str,
    current_weight: float,
    tasks: list[Task],
) -> None:
    """
    **Property 5: current_weight sempre permanece no intervalo [0.0, 1.0] após crisis boost**

    For any persona and any current_weight in [0.0, 1.0], applying the crisis
    boost (current_weight + 0.3, capped at 1.0) must always produce a
    new_weight in [0.0, 1.0].

    **Validates: Requirements 4.2**
    """
    result = sacrifice_algorithm(persona_slug, current_weight, tasks)

    assert 0.0 <= result.new_weight <= 1.0, (
        f"new_weight={result.new_weight} is outside [0.0, 1.0]. "
        f"current_weight={current_weight}"
    )


@given(current_weight=_weight)
@settings(max_examples=200)
def test_property5_crisis_boost_formula(current_weight: float) -> None:
    """
    **Property 5 (formula variant): new_weight = min(current_weight + 0.3, 1.0)**

    The crisis boost must apply exactly the formula min(current_weight + 0.3, 1.0).

    **Validates: Requirements 4.2**
    """
    result = sacrifice_algorithm("test-persona", current_weight, tasks=[])

    expected_weight = min(current_weight + 0.3, 1.0)
    assert abs(result.new_weight - expected_weight) < 1e-9, (
        f"new_weight={result.new_weight}, expected min({current_weight} + 0.3, 1.0) = {expected_weight}"
    )


@given(
    current_weight=st.floats(min_value=0.7, max_value=1.0, allow_nan=False, allow_infinity=False),
)
@settings(max_examples=100)
def test_property5_weight_capped_at_one(current_weight: float) -> None:
    """
    **Property 5 (cap variant)**

    When current_weight >= 0.7, adding 0.3 would exceed 1.0 — the result
    must be capped at exactly 1.0.

    **Validates: Requirements 4.2**
    """
    result = sacrifice_algorithm("cap-persona", current_weight, tasks=[])

    assert result.new_weight <= 1.0, (
        f"new_weight={result.new_weight} exceeds 1.0 for current_weight={current_weight}"
    )
    assert result.new_weight == min(current_weight + 0.3, 1.0)


# ---------------------------------------------------------------------------
# Property 8 — Sacrifice algorithm filtra tarefas pelos critérios corretos
# Validates: Requirements 4.3
# ---------------------------------------------------------------------------


@given(
    persona_slug=_slug,
    current_weight=_weight,
    tasks=_task_list(),
)
@settings(max_examples=300)
def test_property8_only_sacrificable_tasks_selected(
    persona_slug: str,
    current_weight: float,
    tasks: list[Task],
) -> None:
    """
    **Property 8: Sacrifice algorithm filtra tarefas pelos critérios corretos**

    For any set of tasks, the sacrifice algorithm must identify as sacrificable
    exactly the tasks with movable=True AND priority < crisis_weight * 0.6,
    without including tasks that do not satisfy both criteria.

    **Validates: Requirements 4.3**
    """
    result = sacrifice_algorithm(persona_slug, current_weight, tasks)
    crisis_threshold = result.new_weight * 0.6

    for task in result.sacrificed_tasks:
        assert task.movable, (
            f"Task {task.id!r} is in sacrificed_tasks but movable=False"
        )
        assert task.priority < crisis_threshold, (
            f"Task {task.id!r} is in sacrificed_tasks but priority={task.priority:.4f} "
            f">= crisis_threshold={crisis_threshold:.4f}"
        )


@given(
    persona_slug=_slug,
    current_weight=_weight,
    tasks=_task_list(),
)
@settings(max_examples=200)
def test_property8_non_movable_tasks_never_sacrificed(
    persona_slug: str,
    current_weight: float,
    tasks: list[Task],
) -> None:
    """
    **Property 8 (non-movable variant)**

    Tasks with movable=False must never appear in sacrificed_tasks,
    regardless of their priority.

    **Validates: Requirements 4.3**
    """
    result = sacrifice_algorithm(persona_slug, current_weight, tasks)

    # Use object identity (id()) to avoid false positives from duplicate task IDs
    non_movable_objects = {id(t) for t in tasks if not t.movable}
    sacrificed_objects = {id(t) for t in result.sacrificed_tasks}

    overlap = non_movable_objects & sacrificed_objects
    assert not overlap, (
        "Non-movable task objects found in sacrificed_tasks"
    )


@given(
    persona_slug=_slug,
    current_weight=_weight,
    tasks=_task_list(),
)
@settings(max_examples=200)
def test_property8_high_priority_tasks_never_sacrificed(
    persona_slug: str,
    current_weight: float,
    tasks: list[Task],
) -> None:
    """
    **Property 8 (high-priority variant)**

    Tasks with priority >= crisis_weight * 0.6 must never appear in
    sacrificed_tasks, even if they are movable.

    **Validates: Requirements 4.3**
    """
    result = sacrifice_algorithm(persona_slug, current_weight, tasks)
    crisis_threshold = result.new_weight * 0.6

    high_priority_objects = {id(t) for t in tasks if t.priority >= crisis_threshold}
    sacrificed_objects = {id(t) for t in result.sacrificed_tasks}

    overlap = high_priority_objects & sacrificed_objects
    assert not overlap, (
        f"High-priority tasks (priority >= {crisis_threshold:.4f}) found in sacrificed_tasks"
    )


# ---------------------------------------------------------------------------
# Property 9 — Sacrifice algorithm libera no mínimo 2 horas de Deep Work
# Validates: Requirements 4.4
# ---------------------------------------------------------------------------


@given(
    persona_slug=_slug,
    current_weight=_weight,
    tasks=_task_list(),
)
@settings(max_examples=300)
def test_property9_frees_at_least_2h_when_sufficient_tasks(
    persona_slug: str,
    current_weight: float,
    tasks: list[Task],
) -> None:
    """
    **Property 9: Sacrifice algorithm libera no mínimo 2 horas de Deep Work**

    For any set of sacrificable tasks whose total duration is >= 2h, the
    sacrifice algorithm must select and cancel/move enough tasks to free
    at least 2 hours of Deep Work.

    When fallback=False (sufficient tasks exist), freed_hours must be >= 2.0.

    **Validates: Requirements 4.4**
    """
    result = sacrifice_algorithm(persona_slug, current_weight, tasks)

    if not result.fallback:
        assert result.freed_hours >= CRISIS_DEEP_WORK_TARGET_HOURS, (
            f"freed_hours={result.freed_hours:.2f}h is less than the required "
            f"{CRISIS_DEEP_WORK_TARGET_HOURS}h when fallback=False"
        )


@given(
    persona_slug=_slug,
    current_weight=_weight,
)
@settings(max_examples=200)
def test_property9_fallback_when_insufficient_hours(
    persona_slug: str,
    current_weight: float,
) -> None:
    """
    **Property 9 (fallback variant)**

    When the total available sacrificable hours is < 2h, the algorithm must
    return fallback=True and must NOT execute any sacrifice (the returned
    tasks are options, not executed actions).

    **Validates: Requirements 4.4, 4.6**
    """
    # Build tasks that are all sacrificable but total < 2h
    new_weight = min(current_weight + 0.3, 1.0)
    crisis_threshold = new_weight * 0.6

    # Create tasks with priority just below threshold and total duration < 2h
    low_priority = max(0.0, crisis_threshold - 0.01)
    tasks = [
        Task(id="t1", title="Task 1", duration_hours=0.5, movable=True, priority=low_priority),
        Task(id="t2", title="Task 2", duration_hours=0.5, movable=True, priority=low_priority),
        # Total: 1.0h < 2.0h
    ]

    result = sacrifice_algorithm(persona_slug, current_weight, tasks)

    assert result.fallback is True, (
        f"Expected fallback=True when total sacrificable hours < 2h, "
        f"got fallback={result.fallback}"
    )
    assert result.freed_hours < CRISIS_DEEP_WORK_TARGET_HOURS, (
        f"freed_hours={result.freed_hours:.2f}h should be < {CRISIS_DEEP_WORK_TARGET_HOURS}h "
        f"in fallback mode"
    )


@given(
    persona_slug=_slug,
    current_weight=_weight,
    extra_duration=st.floats(min_value=0.0, max_value=5.0, allow_nan=False, allow_infinity=False),
)
@settings(max_examples=200)
def test_property9_freed_hours_matches_selected_tasks(
    persona_slug: str,
    current_weight: float,
    extra_duration: float,
) -> None:
    """
    **Property 9 (consistency variant)**

    The freed_hours in SacrificeResult must equal the sum of duration_hours
    of all sacrificed_tasks.

    **Validates: Requirements 4.4**
    """
    new_weight = min(current_weight + 0.3, 1.0)
    crisis_threshold = new_weight * 0.6

    # Build tasks that are definitely sacrificable
    low_priority = max(0.0, crisis_threshold - 0.01)
    tasks = [
        Task(id=f"t{i}", title=f"Task {i}", duration_hours=1.0, movable=True, priority=low_priority)
        for i in range(5)
    ]

    result = sacrifice_algorithm(persona_slug, current_weight, tasks)

    expected_freed = sum(t.duration_hours for t in result.sacrificed_tasks)
    assert abs(result.freed_hours - expected_freed) < 1e-9, (
        f"freed_hours={result.freed_hours:.4f} does not match sum of "
        f"sacrificed task durations={expected_freed:.4f}"
    )


@given(
    persona_slug=_slug,
    current_weight=_weight,
)
@settings(max_examples=100)
def test_property9_no_tasks_triggers_fallback(
    persona_slug: str,
    current_weight: float,
) -> None:
    """
    **Property 9 (empty tasks variant)**

    When there are no tasks at all, the algorithm must return fallback=True
    with freed_hours=0.0.

    **Validates: Requirements 4.4, 4.6**
    """
    result = sacrifice_algorithm(persona_slug, current_weight, tasks=[])

    assert result.fallback is True
    assert result.freed_hours == 0.0
    assert result.sacrificed_tasks == []
