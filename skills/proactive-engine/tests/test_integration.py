"""End-to-end integration tests for the proactive-engine."""

from __future__ import annotations

import json
from datetime import datetime, timezone
from pathlib import Path
from unittest.mock import AsyncMock, patch

import pytest

from engine import ProactiveEngine
from factory import create_engine
from models import DetectionEvent
from protocols import PersonaConfig
from settings import ProactiveSettings


@pytest.fixture
def full_engine(default_settings, mock_life_log, mock_memupalace, mock_clawhub):
    return ProactiveEngine(
        settings=default_settings,
        life_log=mock_life_log,
        memupalace=mock_memupalace,
        clawhub=mock_clawhub,
        personas=[
            PersonaConfig(slug="carreira", current_weight=0.8),
            PersonaConfig(slug="estudos", current_weight=0.5),
        ],
        installed_skills=["life-log", "guardrails"],
    )


@pytest.mark.asyncio
async def test_full_run_cycle_writes_pending_and_heartbeat(
    full_engine, default_settings, mock_life_log
):
    """Complete run_cycle writes pending-events.json and heartbeat-state.json."""
    # Simulate inactive personas
    mock_life_log.get_persona_summary.return_value = {"records": [], "last_interaction": None}

    await full_engine.run_cycle()

    hb_path = Path(default_settings.heartbeat_state_path)
    assert hb_path.exists()
    state = json.loads(hb_path.read_text())
    assert "proactive-cycle" in state

    # pending-events.json should exist (even if empty after consume+flush)
    pending_path = Path(default_settings.pending_events_path)
    assert pending_path.exists()


@pytest.mark.asyncio
async def test_cve_check_writes_cve_event(full_engine, default_settings, mock_clawhub):
    """run_cve_check emits CVE event and flushes to pending-events.json."""
    mock_clawhub.get_batch_cves.return_value = {
        "life-log": [{"cve_id": "CVE-2026-001", "severity": "CRITICAL", "description": "RCE"}]
    }
    await full_engine.run_cve_check()

    pending_path = Path(default_settings.pending_events_path)
    assert pending_path.exists()
    data = json.loads(pending_path.read_text())
    cve_events = [e for e in data if e["type"] == "cve"]
    assert len(cve_events) == 1


@pytest.mark.asyncio
async def test_weekly_run_with_events(full_engine, default_settings):
    """run_weekly with events from the bus generates a summary."""
    now = datetime.now(tz=timezone.utc)
    full_engine._bus._events = [
        DetectionEvent(type="inactivity", persona="carreira", payload={}, timestamp=now),
        DetectionEvent(type="memory_staleness", persona="system", payload={"wing": "carreira/metas", "stale_count": 3, "staleness_days": 14}, timestamp=now),
    ]

    with patch.object(full_engine._bus, "flush"):
        await full_engine.run_weekly()

    hb_path = Path(default_settings.heartbeat_state_path)
    assert hb_path.exists()
    state = json.loads(hb_path.read_text())
    assert "proactive-weekly" in state


@pytest.mark.asyncio
async def test_factory_creates_engine(default_settings, mock_life_log, mock_clawhub):
    """create_engine returns a ProactiveEngine in degraded mode when memupalace=None."""
    engine = create_engine(
        settings=default_settings,
        life_log=mock_life_log,
        clawhub=mock_clawhub,
        memupalace=None,
    )
    assert isinstance(engine, ProactiveEngine)
    await engine.run_cycle()  # must not raise
