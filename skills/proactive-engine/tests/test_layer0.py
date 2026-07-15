"""Tests for Layer 0 detectors."""

from __future__ import annotations

import asyncio
from datetime import datetime, timedelta, timezone
from unittest.mock import AsyncMock, MagicMock

import pytest
from hypothesis import HealthCheck, given, settings as h_settings
from hypothesis import strategies as st

from event_bus import EventBus
from layer0.cve import CVEDetector
from layer0.inactivity import InactivityDetector
from layer0.intent_tracker import IntentTracker
from layer0.staleness import MemoryStalenessDetector
from layer0.temporal import TemporalPatternDetector
from models import DetectionEvent
from protocols import PersonaConfig
from settings import ProactiveSettings


# ---------------------------------------------------------------------------
# InactivityDetector
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_inactivity_ignores_low_weight_persona(default_settings):
    bus = EventBus(default_settings)
    life_log = AsyncMock()
    life_log.get_persona_summary.return_value = {"records": [], "last_interaction": None}
    detector = InactivityDetector(life_log, bus, default_settings)
    low_weight = [PersonaConfig(slug="musica", current_weight=0.05)]
    await detector.run(low_weight)
    assert bus._events == []


@pytest.mark.asyncio
async def test_inactivity_no_records_emits_event(default_settings):
    bus = EventBus(default_settings)
    life_log = AsyncMock()
    life_log.get_persona_summary.return_value = {"records": [], "last_interaction": None}
    detector = InactivityDetector(life_log, bus, default_settings)
    personas = [PersonaConfig(slug="carreira", current_weight=0.8)]
    await detector.run(personas)
    assert len(bus._events) == 1
    assert bus._events[0].type == "inactivity"
    assert bus._events[0].payload["last_interaction"] is None


@pytest.mark.asyncio
async def test_inactivity_with_records_no_event(default_settings):
    bus = EventBus(default_settings)
    life_log = AsyncMock()
    life_log.get_persona_summary.return_value = {
        "records": [{"intent": "study", "tools": [], "timestamp": datetime.now(tz=timezone.utc)}],
        "last_interaction": datetime.now(tz=timezone.utc),
    }
    detector = InactivityDetector(life_log, bus, default_settings)
    await detector.run([PersonaConfig(slug="carreira", current_weight=0.8)])
    assert bus._events == []


@pytest.mark.asyncio
async def test_inactivity_empty_personas_no_events(default_settings):
    bus = EventBus(default_settings)
    life_log = AsyncMock()
    detector = InactivityDetector(life_log, bus, default_settings)
    await detector.run([])
    assert bus._events == []


# Propriedade 6: Filtro de Peso do InactivityDetector
@given(
    weights=st.lists(
        st.floats(min_value=0.0, max_value=0.099, allow_nan=False),
        min_size=1,
        max_size=10,
    )
)
@h_settings(max_examples=50, suppress_health_check=[HealthCheck.function_scoped_fixture])
def test_inactivity_weight_filter(weights, default_settings):
    """All personas with weight < 0.1 produce no events."""
    bus = EventBus(default_settings)
    life_log = AsyncMock()
    life_log.get_persona_summary.return_value = {"records": [], "last_interaction": None}
    personas = [PersonaConfig(slug=f"p{i}", current_weight=w) for i, w in enumerate(weights)]
    detector = InactivityDetector(life_log, bus, default_settings)
    asyncio.run(detector.run(personas))
    assert bus._events == []


# ---------------------------------------------------------------------------
# MemoryStalenessDetector
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_staleness_with_no_memupalace(default_settings):
    bus = EventBus(default_settings)
    detector = MemoryStalenessDetector(None, bus, default_settings)
    await detector.run()
    assert bus._events == []


@pytest.mark.asyncio
async def test_staleness_ignores_proactive_intent_wing(default_settings, mock_memupalace):
    bus = EventBus(default_settings)
    memory = MagicMock()
    memory.wing = "proactive/intent"
    mock_memupalace.get_stale.return_value = [memory]
    detector = MemoryStalenessDetector(mock_memupalace, bus, default_settings)
    await detector.run()
    assert bus._events == []


@pytest.mark.asyncio
async def test_staleness_groups_by_wing(default_settings, mock_memupalace):
    bus = EventBus(default_settings)

    m1 = MagicMock()
    m1.wing = "carreira/metas"
    m2 = MagicMock()
    m2.wing = "carreira/metas"
    m3 = MagicMock()
    m3.wing = "estudos/notas"

    mock_memupalace.get_stale.return_value = [m1, m2, m3]
    detector = MemoryStalenessDetector(mock_memupalace, bus, default_settings)
    await detector.run()
    wings = {e.payload["wing"] for e in bus._events}
    assert wings == {"carreira/metas", "estudos/notas"}


# ---------------------------------------------------------------------------
# CVEDetector
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_cve_consolidates_multiple_skills(default_settings, mock_clawhub):
    bus = EventBus(default_settings)
    mock_clawhub.get_batch_cves.return_value = {
        "life-log": [{"cve_id": "CVE-001", "severity": "HIGH", "description": "test"}],
        "guardrails": [{"cve_id": "CVE-002", "severity": "MEDIUM", "description": "test2"}],
    }
    detector = CVEDetector(mock_clawhub, bus, default_settings)
    await detector.run(["life-log", "guardrails"])
    assert len(bus._events) == 1
    assert bus._events[0].type == "cve"
    assert len(bus._events[0].payload["cves"]) == 2


@pytest.mark.asyncio
async def test_cve_clawhub_unavailable_no_event(default_settings, mock_clawhub):
    bus = EventBus(default_settings)
    mock_clawhub.get_batch_cves.side_effect = ConnectionError("unavailable")
    detector = CVEDetector(mock_clawhub, bus, default_settings)
    await detector.run(["life-log"])
    assert bus._events == []


# ---------------------------------------------------------------------------
# IntentTracker
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_intent_tracker_queues_when_memupalace_none(default_settings, tmp_path):
    import json
    tracker = IntentTracker(None, default_settings)
    now = datetime.now(tz=timezone.utc)
    await tracker.track("study Python", "estudos", now)

    import pathlib
    q = pathlib.Path(default_settings.intent_queue_path)
    assert q.exists()
    data = json.loads(q.read_text())
    assert len(data) == 1
    assert data[0]["intent"] == "study Python"


@pytest.mark.asyncio
async def test_intent_tracker_flush_queue(default_settings, mock_memupalace):
    import json
    import pathlib

    q = pathlib.Path(default_settings.intent_queue_path)
    q.parent.mkdir(parents=True, exist_ok=True)
    q.write_text(json.dumps([{
        "intent": "study",
        "persona": "estudos",
        "timestamp": datetime.now(tz=timezone.utc).isoformat(),
        "day_of_week": "Monday",
        "hour_of_day": 10,
    }]))

    tracker = IntentTracker(mock_memupalace, default_settings)
    await tracker.flush_queue()
    mock_memupalace.add.assert_called_once()
    # Queue should be empty after successful flush
    data = json.loads(q.read_text())
    assert data == []


# ---------------------------------------------------------------------------
# TemporalPatternDetector
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_temporal_insufficient_data(default_settings, mock_life_log):
    bus = EventBus(default_settings)
    # total cnt < 10
    mock_life_log.query_temporal_patterns.return_value = [
        {"persona": "carreira", "day_of_week": "Monday", "hour_bucket": 10, "cnt": 3}
    ]
    detector = TemporalPatternDetector(mock_life_log, bus, default_settings)
    await detector.run([PersonaConfig(slug="carreira", current_weight=0.8)])
    assert bus._events == []


@pytest.mark.asyncio
async def test_temporal_emits_at_most_3(default_settings, mock_life_log):
    bus = EventBus(default_settings)
    mock_life_log.query_temporal_patterns.return_value = [
        {"persona": "carreira", "day_of_week": f"Day{i}", "hour_bucket": i, "cnt": 5}
        for i in range(10)
    ]
    detector = TemporalPatternDetector(mock_life_log, bus, default_settings)
    await detector.run([PersonaConfig(slug="carreira", current_weight=0.8)])
    assert len(bus._events) <= 3


@pytest.mark.asyncio
async def test_temporal_filters_min_occurrences(default_settings, mock_life_log):
    bus = EventBus(default_settings)
    # cnt=1 < pattern_min_occurrences=3
    mock_life_log.query_temporal_patterns.return_value = [
        {"persona": "carreira", "day_of_week": "Monday", "hour_bucket": 10, "cnt": 1},
        {"persona": "carreira", "day_of_week": "Tuesday", "hour_bucket": 11, "cnt": 1},
        {"persona": "carreira", "day_of_week": "Wednesday", "hour_bucket": 12, "cnt": 1},
        {"persona": "carreira", "day_of_week": "Thursday", "hour_bucket": 13, "cnt": 1},
        {"persona": "carreira", "day_of_week": "Friday", "hour_bucket": 14, "cnt": 1},
        {"persona": "carreira", "day_of_week": "Saturday", "hour_bucket": 15, "cnt": 1},
        {"persona": "carreira", "day_of_week": "Sunday", "hour_bucket": 16, "cnt": 1},
        {"persona": "estudos", "day_of_week": "Monday", "hour_bucket": 8, "cnt": 1},
        {"persona": "estudos", "day_of_week": "Tuesday", "hour_bucket": 9, "cnt": 1},
        {"persona": "estudos", "day_of_week": "Wednesday", "hour_bucket": 10, "cnt": 1},
    ]
    detector = TemporalPatternDetector(mock_life_log, bus, default_settings)
    await detector.run([PersonaConfig(slug="carreira", current_weight=0.8)])
    assert bus._events == []


# Propriedade 7: Guarda de Dados Mínimos
@given(
    records=st.lists(
        st.fixed_dictionaries({
            "persona": st.just("carreira"),
            "day_of_week": st.sampled_from(["Monday", "Tuesday"]),
            "hour_bucket": st.integers(min_value=0, max_value=23),
            "cnt": st.integers(min_value=1, max_value=2),
        }),
        min_size=0,
        max_size=4,  # total cnt always < 10
    )
)
@h_settings(max_examples=50, suppress_health_check=[HealthCheck.function_scoped_fixture])
def test_temporal_min_data_guard(records, default_settings):
    """With < 10 total records, no events emitted."""
    bus = EventBus(default_settings)
    life_log = AsyncMock()
    life_log.query_temporal_patterns.return_value = records
    detector = TemporalPatternDetector(life_log, bus, default_settings)
    asyncio.run(detector.run([PersonaConfig(slug="carreira", current_weight=0.8)]))
    total = sum(r["cnt"] for r in records)
    if total < 10:
        assert bus._events == []


# Propriedade 8: Idempotência do TemporalPatternDetector
@given(
    records=st.lists(
        st.fixed_dictionaries({
            "persona": st.sampled_from(["carreira", "estudos"]),
            "day_of_week": st.sampled_from(["Monday", "Tuesday", "Wednesday"]),
            "hour_bucket": st.integers(min_value=0, max_value=23),
            "cnt": st.integers(min_value=3, max_value=5),
        }),
        min_size=4,
        max_size=10,
    )
)
@h_settings(max_examples=30, suppress_health_check=[HealthCheck.function_scoped_fixture])
def test_temporal_idempotent(records, default_settings):
    """Two runs on same data produce the same set of events (same payloads)."""
    import copy
    personas = [
        PersonaConfig(slug="carreira", current_weight=0.8),
        PersonaConfig(slug="estudos", current_weight=0.5),
    ]

    bus1 = EventBus(default_settings)
    ll1 = AsyncMock()
    ll1.query_temporal_patterns.return_value = copy.deepcopy(records)
    detector1 = TemporalPatternDetector(ll1, bus1, default_settings)
    asyncio.run(detector1.run(personas))

    s2 = ProactiveSettings(
        pending_events_path=default_settings.pending_events_path + ".2",
        intent_queue_path=default_settings.intent_queue_path,
        heartbeat_state_path=default_settings.heartbeat_state_path,
    )
    bus2 = EventBus(s2)
    ll2 = AsyncMock()
    ll2.query_temporal_patterns.return_value = copy.deepcopy(records)
    detector2 = TemporalPatternDetector(ll2, bus2, s2)
    asyncio.run(detector2.run(personas))

    payloads1 = sorted(str(e.payload) for e in bus1._events)
    payloads2 = sorted(str(e.payload) for e in bus2._events)
    assert payloads1 == payloads2
