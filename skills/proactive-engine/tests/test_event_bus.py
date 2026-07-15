"""Tests for event_bus.py — EventBus."""

from __future__ import annotations

import json
from datetime import datetime, timedelta, timezone

import pytest
from hypothesis import HealthCheck, given, settings as h_settings
from hypothesis import strategies as st

from event_bus import EventBus
from models import DetectionEvent
from settings import ProactiveSettings


def make_event(
    type_="inactivity",
    persona="carreira",
    offset_minutes=0,
    now=None,
) -> DetectionEvent:
    if now is None:
        now = datetime.now(tz=timezone.utc)
    return DetectionEvent(
        type=type_,
        persona=persona,
        payload={},
        timestamp=now + timedelta(minutes=offset_minutes),
    )


# ---------------------------------------------------------------------------
# Unit tests
# ---------------------------------------------------------------------------


def test_emit_returns_true_for_new_event(default_settings):
    bus = EventBus(default_settings)
    event = make_event()
    assert bus.emit(event) is True


def test_emit_returns_false_for_duplicate_within_window(default_settings):
    bus = EventBus(default_settings)
    now = datetime.now(tz=timezone.utc)
    e1 = make_event(now=now)
    e2 = make_event(now=now, offset_minutes=30)  # within 6h window
    assert bus.emit(e1) is True
    assert bus.emit(e2) is False


def test_emit_returns_true_outside_window(default_settings):
    bus = EventBus(default_settings)
    now = datetime.now(tz=timezone.utc)
    e1 = make_event(now=now)
    e2 = make_event(now=now, offset_minutes=400)  # beyond 6h = 360 min
    assert bus.emit(e1) is True
    assert bus.emit(e2) is True


def test_consume_returns_unprocessed_sorted(default_settings):
    bus = EventBus(default_settings)
    now = datetime.now(tz=timezone.utc)
    e1 = make_event("inactivity", "a", 0, now)
    e2 = make_event("cve", "system", 10, now)
    bus.emit(e1)
    bus.emit(e2)
    result = bus.consume()
    assert len(result) == 2
    assert result[0].timestamp <= result[1].timestamp


def test_consume_marks_as_processed(default_settings):
    bus = EventBus(default_settings)
    bus.emit(make_event())
    result1 = bus.consume()
    assert len(result1) == 1
    result2 = bus.consume()
    assert result2 == []


def test_consume_empty_bus(default_settings):
    bus = EventBus(default_settings)
    assert bus.consume() == []


def test_flush_and_load_roundtrip(default_settings):
    bus = EventBus(default_settings)
    now = datetime.now(tz=timezone.utc)
    bus.emit(make_event("inactivity", "carreira", 0, now))
    bus.flush()

    bus2 = EventBus(default_settings)
    unprocessed = [e for e in bus2._events if not e.processed]
    assert len(unprocessed) == 1
    assert unprocessed[0].persona == "carreira"


def test_load_corrupted_file_starts_empty(default_settings, tmp_path):
    import pathlib
    p = pathlib.Path(default_settings.pending_events_path)
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text("not valid json{{{{")

    bus = EventBus(default_settings)
    assert bus._events == []


# ---------------------------------------------------------------------------
# Property-based tests
# ---------------------------------------------------------------------------

event_type_st = st.sampled_from(["inactivity", "memory_staleness", "cve", "temporal_pattern"])
persona_st = st.text(min_size=1, max_size=20, alphabet=st.characters(whitelist_categories=("Ll",)))


# Propriedade 3: Deduplicação do EventBus
@given(
    event_type=event_type_st,
    persona=persona_st,
    n=st.integers(min_value=2, max_value=10),
)
@h_settings(max_examples=100, suppress_health_check=[HealthCheck.function_scoped_fixture])
def test_dedup_within_window(event_type, persona, n, default_settings):
    """N emissions of same (type, persona) within window → exactly 1 unprocessed event."""
    bus = EventBus(default_settings)
    base_time = datetime.now(tz=timezone.utc)
    for i in range(n):
        event = DetectionEvent(
            type=event_type,
            persona=persona,
            payload={},
            timestamp=base_time + timedelta(minutes=i),
        )
        bus.emit(event)
    unprocessed = [e for e in bus._events if not e.processed]
    assert len(unprocessed) == 1


# Propriedade 4: Atomicidade do consume()
@given(
    event_type=event_type_st,
    persona=persona_st,
    n=st.integers(min_value=1, max_value=5),
)
@h_settings(max_examples=100, suppress_health_check=[HealthCheck.function_scoped_fixture])
def test_consume_atomicity(event_type, persona, n, default_settings):
    """After consume(), all returned events have processed=True, second call returns []."""
    bus = EventBus(default_settings)
    base = datetime.now(tz=timezone.utc)
    for i in range(n):
        p = f"{persona}{i}"
        bus.emit(DetectionEvent(type=event_type, persona=p, payload={}, timestamp=base + timedelta(hours=i * 10)))
    result = bus.consume()
    assert all(e.processed for e in result)
    assert bus.consume() == []


# Propriedade 5: Round-Trip de Persistência do EventBus
@given(
    events=st.lists(
        st.builds(
            DetectionEvent,
            type=event_type_st,
            persona=persona_st,
            payload=st.fixed_dictionaries({}),
            timestamp=st.datetimes(timezones=st.just(timezone.utc)),
            processed=st.just(False),
        ),
        min_size=1,
        max_size=5,
        unique_by=lambda e: (e.type, e.persona),  # avoid dedup conflicts
    )
)
@h_settings(max_examples=50)
def test_flush_load_preserves_unprocessed(events):
    """flush() + load() in new instance preserves all unprocessed events."""
    import tempfile

    with tempfile.TemporaryDirectory() as tmpdir:
        s = ProactiveSettings(
            pending_events_path=f"{tmpdir}/pending-events.json",
            intent_queue_path=f"{tmpdir}/intent-queue.json",
            heartbeat_state_path=f"{tmpdir}/heartbeat-state.json",
        )
        bus = EventBus(s)
        for e in events:
            bus._events.append(e)
        bus.flush()

        bus2 = EventBus(s)
        unprocessed2 = [e for e in bus2._events if not e.processed]
        assert len(unprocessed2) == len(events)
