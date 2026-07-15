"""Tests for memupalace data models — unit tests and property-based tests."""

from __future__ import annotations

from datetime import datetime, timezone

import pytest
from hypothesis import given, settings
from hypothesis import strategies as st
from pydantic import ValidationError

from models import (
    AddResult,
    Memory,
    MemupalaceSettings,
    SearchResult,
)

# ---------------------------------------------------------------------------
# Helpers / strategies
# ---------------------------------------------------------------------------

UTC = timezone.utc

# Valid location slug strategy (alphanumeric + hyphen + underscore, min 1 char)
valid_slug = st.text(
    alphabet=st.characters(
        whitelist_categories=("Lu", "Ll", "Nd"),
        whitelist_characters="-_",
    ),
    min_size=1,
    max_size=50,
).filter(lambda s: bool(s) and s[0] not in "-_" or len(s) == 1)

# Simpler valid slug: just alphanumeric
simple_slug = st.text(
    alphabet="abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-",
    min_size=1,
    max_size=30,
)

memory_strategy = st.builds(
    Memory,
    id=st.uuids().map(str),
    content=st.text(min_size=1).filter(lambda s: s.strip()),
    wing=simple_slug,
    hall=st.one_of(st.none(), simple_slug),
    room=st.one_of(st.none(), simple_slug),
    created_at=st.datetimes(timezones=st.just(UTC)),
    reinforcement_count=st.integers(min_value=0, max_value=1000),
    last_reinforced_at=st.datetimes(timezones=st.just(UTC)),
)


# ---------------------------------------------------------------------------
# Unit tests — Memory validation
# ---------------------------------------------------------------------------


def _make_memory(**kwargs: object) -> Memory:
    defaults = dict(
        id="550e8400-e29b-41d4-a716-446655440000",
        content="Hello world",
        wing="work",
        hall=None,
        room=None,
        created_at=datetime(2024, 1, 1, tzinfo=UTC),
        reinforcement_count=0,
        last_reinforced_at=datetime(2024, 1, 1, tzinfo=UTC),
    )
    defaults.update(kwargs)
    return Memory(**defaults)  # type: ignore[arg-type]


class TestMemoryValidation:
    def test_valid_memory_created(self) -> None:
        m = _make_memory()
        assert m.content == "Hello world"
        assert m.wing == "work"

    def test_empty_content_raises(self) -> None:
        with pytest.raises(ValidationError, match="empty"):
            _make_memory(content="")

    def test_whitespace_only_content_raises(self) -> None:
        with pytest.raises(ValidationError, match="empty"):
            _make_memory(content="   ")

    def test_invalid_wing_characters_raises(self) -> None:
        with pytest.raises(ValidationError, match="invalid characters"):
            _make_memory(wing="bad wing!")

    def test_invalid_hall_characters_raises(self) -> None:
        with pytest.raises(ValidationError, match="invalid characters"):
            _make_memory(hall="bad/hall")

    def test_invalid_room_characters_raises(self) -> None:
        with pytest.raises(ValidationError, match="invalid characters"):
            _make_memory(room="bad room")

    def test_none_hall_and_room_allowed(self) -> None:
        m = _make_memory(hall=None, room=None)
        assert m.hall is None
        assert m.room is None

    def test_valid_slug_with_hyphen_and_underscore(self) -> None:
        m = _make_memory(wing="my-wing_01", hall="my-hall", room="room_1")
        assert m.wing == "my-wing_01"

    def test_wing_with_space_raises(self) -> None:
        with pytest.raises(ValidationError, match="invalid characters"):
            _make_memory(wing="my wing")

    def test_wing_with_dot_raises(self) -> None:
        with pytest.raises(ValidationError, match="invalid characters"):
            _make_memory(wing="my.wing")


# ---------------------------------------------------------------------------
# Unit tests — MemupalaceSettings validation
# ---------------------------------------------------------------------------


class TestMemupalaceSettingsValidation:
    def test_defaults_are_valid(self) -> None:
        s = MemupalaceSettings()
        assert s.duplicate_threshold == 0.95
        assert s.recency_decay_days == 30

    def test_threshold_zero_is_valid(self) -> None:
        s = MemupalaceSettings(duplicate_threshold=0.0)
        assert s.duplicate_threshold == 0.0

    def test_threshold_one_is_valid(self) -> None:
        s = MemupalaceSettings(duplicate_threshold=1.0)
        assert s.duplicate_threshold == 1.0

    def test_threshold_above_one_raises(self) -> None:
        with pytest.raises(ValidationError, match="duplicate_threshold"):
            MemupalaceSettings(duplicate_threshold=1.01)

    def test_threshold_below_zero_raises(self) -> None:
        with pytest.raises(ValidationError, match="duplicate_threshold"):
            MemupalaceSettings(duplicate_threshold=-0.01)

    def test_recency_decay_days_one_is_valid(self) -> None:
        s = MemupalaceSettings(recency_decay_days=1)
        assert s.recency_decay_days == 1

    def test_recency_decay_days_zero_raises(self) -> None:
        with pytest.raises(ValidationError, match="recency_decay_days"):
            MemupalaceSettings(recency_decay_days=0)

    def test_recency_decay_days_negative_raises(self) -> None:
        with pytest.raises(ValidationError, match="recency_decay_days"):
            MemupalaceSettings(recency_decay_days=-5)


# ---------------------------------------------------------------------------
# Property 14: Serialization Round-Trip
# Validates: Requirements 9.3, 9.5
# ---------------------------------------------------------------------------


@given(memory_strategy)
@settings(max_examples=100)
def test_serialization_round_trip(memory: Memory) -> None:
    """Property 14: For any valid Memory, serialize → deserialize yields equal object."""
    restored = Memory.model_validate(memory.model_dump())
    assert restored == memory


# ---------------------------------------------------------------------------
# Property 15: Invalid Configuration Raises Descriptive Error
# Validates: Requirements 5.5, 6.4, 10.5
# ---------------------------------------------------------------------------


@given(
    threshold=st.one_of(
        st.floats(max_value=-0.001, allow_nan=False, allow_infinity=False),
        st.floats(min_value=1.001, allow_nan=False, allow_infinity=False),
    )
)
@settings(max_examples=100)
def test_invalid_threshold_raises_descriptive_error(threshold: float) -> None:
    """Property 15 (threshold): Out-of-range duplicate_threshold raises ValueError."""
    with pytest.raises(ValidationError) as exc_info:
        MemupalaceSettings(duplicate_threshold=threshold)
    # Error message must mention the field name
    assert "duplicate_threshold" in str(exc_info.value)


@given(decay=st.integers(max_value=0))
@settings(max_examples=100)
def test_invalid_recency_decay_raises_descriptive_error(decay: int) -> None:
    """Property 15 (recency_decay): recency_decay_days < 1 raises ValueError."""
    with pytest.raises(ValidationError) as exc_info:
        MemupalaceSettings(recency_decay_days=decay)
    assert "recency_decay_days" in str(exc_info.value)
