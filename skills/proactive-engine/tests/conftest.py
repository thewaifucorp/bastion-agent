"""Shared fixtures for proactive-engine tests."""

from __future__ import annotations

from datetime import datetime, timezone
from typing import Any
from unittest.mock import AsyncMock

import pytest

from models import DetectionEvent
from protocols import InteractionRecord, PersonaConfig
from settings import ProactiveSettings


@pytest.fixture
def default_settings(tmp_path) -> ProactiveSettings:
    return ProactiveSettings(
        pending_events_path=str(tmp_path / "pending-events.json"),
        intent_queue_path=str(tmp_path / "intent-queue.json"),
        heartbeat_state_path=str(tmp_path / "heartbeat-state.json"),
    )


@pytest.fixture
def now() -> datetime:
    return datetime.now(tz=timezone.utc)


@pytest.fixture
def mock_life_log():
    mock = AsyncMock()
    mock.get_persona_summary.return_value = {"records": [], "last_interaction": None}
    mock.query_temporal_patterns.return_value = []
    return mock


@pytest.fixture
def mock_memupalace():
    mock = AsyncMock()
    mock.search.return_value = []
    mock.add.return_value = None
    mock.get_stale.return_value = []
    return mock


@pytest.fixture
def mock_clawhub():
    mock = AsyncMock()
    mock.get_cves.return_value = []
    mock.get_batch_cves.return_value = {}
    return mock


@pytest.fixture
def sample_event(now) -> DetectionEvent:
    return DetectionEvent(
        type="inactivity",
        persona="carreira",
        payload={"days_inactive": 3, "last_interaction": None},
        timestamp=now,
    )


@pytest.fixture
def active_personas() -> list[PersonaConfig]:
    return [
        PersonaConfig(slug="carreira", current_weight=0.8),
        PersonaConfig(slug="estudos", current_weight=0.5),
        PersonaConfig(slug="musica", current_weight=0.05),  # below threshold
    ]
