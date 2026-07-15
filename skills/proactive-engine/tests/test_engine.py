"""Tests for ProactiveEngine."""

from __future__ import annotations

import json
from datetime import datetime, timezone
from pathlib import Path
from unittest.mock import AsyncMock, MagicMock, patch

import pytest

from engine import ProactiveEngine
from models import DetectionEvent
from protocols import PersonaConfig
from settings import ProactiveSettings


def make_engine(default_settings, mock_life_log, mock_memupalace, mock_clawhub, personas=None):
    return ProactiveEngine(
        settings=default_settings,
        life_log=mock_life_log,
        memupalace=mock_memupalace,
        clawhub=mock_clawhub,
        personas=personas or [PersonaConfig(slug="carreira", current_weight=0.8)],
        installed_skills=["life-log"],
    )


@pytest.mark.asyncio
async def test_run_cycle_failure_in_one_step_does_not_stop_others(
    default_settings, mock_life_log, mock_memupalace, mock_clawhub
):
    """A failure in one step must not prevent subsequent steps from running."""
    engine = make_engine(default_settings, mock_life_log, mock_memupalace, mock_clawhub)

    # Make InactivityDetector raise
    mock_life_log.get_persona_summary.side_effect = RuntimeError("DB error")

    # Should complete without raising
    await engine.run_cycle()

    # heartbeat-state.json should still be updated
    state_path = Path(default_settings.heartbeat_state_path)
    assert state_path.exists()
    state = json.loads(state_path.read_text())
    assert "proactive-cycle" in state


@pytest.mark.asyncio
async def test_run_cycle_updates_heartbeat_state(
    default_settings, mock_life_log, mock_memupalace, mock_clawhub
):
    engine = make_engine(default_settings, mock_life_log, mock_memupalace, mock_clawhub)
    await engine.run_cycle()

    path = Path(default_settings.heartbeat_state_path)
    assert path.exists()
    state = json.loads(path.read_text())
    assert "proactive-cycle" in state
    assert "last_run" in state["proactive-cycle"]


@pytest.mark.asyncio
async def test_run_cycle_degraded_mode_memupalace_none(
    default_settings, mock_life_log, mock_clawhub
):
    """Engine with memupalace=None should complete run_cycle without error."""
    engine = ProactiveEngine(
        settings=default_settings,
        life_log=mock_life_log,
        memupalace=None,
        clawhub=mock_clawhub,
        personas=[PersonaConfig(slug="carreira", current_weight=0.8)],
    )
    await engine.run_cycle()  # must not raise


@pytest.mark.asyncio
async def test_run_cve_check_flushes_bus(
    default_settings, mock_life_log, mock_memupalace, mock_clawhub
):
    mock_clawhub.get_batch_cves.return_value = {
        "life-log": [{"cve_id": "CVE-001", "severity": "HIGH", "description": "test"}]
    }
    engine = make_engine(default_settings, mock_life_log, mock_memupalace, mock_clawhub)
    await engine.run_cve_check()

    path = Path(default_settings.pending_events_path)
    assert path.exists()


@pytest.mark.asyncio
async def test_run_weekly_no_events_returns(
    default_settings, mock_life_log, mock_memupalace, mock_clawhub
):
    engine = make_engine(default_settings, mock_life_log, mock_memupalace, mock_clawhub)
    await engine.run_weekly()  # must not raise
