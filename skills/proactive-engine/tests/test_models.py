"""Tests for models.py — DetectionEvent and ProactiveSuggestion."""

from __future__ import annotations

from datetime import datetime, timezone, timedelta

import pytest
from hypothesis import given, settings as h_settings
from hypothesis import strategies as st
from pydantic import ValidationError

from models import DetectionEvent, ProactiveSuggestion


# ---------------------------------------------------------------------------
# Unit tests
# ---------------------------------------------------------------------------


def test_detection_event_requires_type():
    with pytest.raises(ValidationError):
        DetectionEvent(persona="x", payload={}, timestamp=datetime.now(tz=timezone.utc))


def test_detection_event_requires_persona():
    with pytest.raises(ValidationError):
        DetectionEvent(type="inactivity", payload={}, timestamp=datetime.now(tz=timezone.utc))


def test_detection_event_requires_timestamp():
    with pytest.raises(ValidationError):
        DetectionEvent(type="inactivity", persona="x", payload={})


def test_detection_event_rejects_naive_timestamp():
    with pytest.raises(ValidationError):
        DetectionEvent(
            type="inactivity",
            persona="x",
            payload={},
            timestamp=datetime(2026, 1, 1),  # no timezone
        )


def test_proactive_suggestion_rejects_naive_timestamp():
    with pytest.raises(ValidationError):
        ProactiveSuggestion(
            event_id=None,
            text="test",
            event_type="inactivity",
            persona="x",
            timestamp=datetime(2026, 1, 1),  # no timezone
            model_used="fallback",
        )


def test_detection_event_defaults():
    event = DetectionEvent(
        type="cve",
        persona="system",
        payload={},
        timestamp=datetime.now(tz=timezone.utc),
    )
    assert event.processed is False
    assert len(event.id) == 36  # UUID4


def test_proactive_suggestion_defaults():
    s = ProactiveSuggestion(
        event_id=None,
        text="hello",
        event_type=None,
        persona="system",
        timestamp=datetime.now(tz=timezone.utc),
        model_used="test",
    )
    assert s.is_fallback is False


# ---------------------------------------------------------------------------
# Property-based tests
# ---------------------------------------------------------------------------

event_strategy = st.builds(
    DetectionEvent,
    id=st.uuids().map(str),
    type=st.sampled_from(["inactivity", "memory_staleness", "cve", "temporal_pattern"]),
    persona=st.text(
        min_size=1, max_size=50, alphabet=st.characters(whitelist_categories=("Ll",), whitelist_characters="-_")
    ),
    payload=st.fixed_dictionaries({}),
    timestamp=st.datetimes(timezones=st.just(timezone.utc)),
    processed=st.booleans(),
)

suggestion_strategy = st.builds(
    ProactiveSuggestion,
    id=st.uuids().map(str),
    event_id=st.one_of(st.none(), st.uuids().map(str)),
    text=st.text(min_size=1, max_size=200),
    event_type=st.one_of(
        st.none(),
        st.sampled_from(["inactivity", "memory_staleness", "cve", "temporal_pattern"]),
    ),
    persona=st.text(min_size=1, max_size=50, alphabet=st.characters(whitelist_categories=("Ll",), whitelist_characters="-_")),
    timestamp=st.datetimes(timezones=st.just(timezone.utc)),
    model_used=st.text(min_size=1, max_size=100),
    is_fallback=st.booleans(),
)


# Propriedade 1: Round-Trip de Serialização de DetectionEvent
@given(event_strategy)
@h_settings(max_examples=100)
def test_detection_event_round_trip(event: DetectionEvent):
    """Serialize to JSON and back — must produce equal object."""
    restored = DetectionEvent.model_validate(event.model_dump(mode="json"))
    assert restored == event


# Propriedade 2: Round-Trip de Serialização de ProactiveSuggestion
@given(suggestion_strategy)
@h_settings(max_examples=100)
def test_proactive_suggestion_round_trip(s: ProactiveSuggestion):
    """Serialize to JSON and back — must produce equal object."""
    restored = ProactiveSuggestion.model_validate(s.model_dump(mode="json"))
    assert restored == s
