"""Tests for Layer 1 generators."""

from __future__ import annotations

import asyncio
from datetime import datetime, timezone
from unittest.mock import AsyncMock, MagicMock, patch

import pytest
from hypothesis import HealthCheck, given, settings as h_settings
from hypothesis import strategies as st

from layer1.suggestion_generator import SuggestionGenerator
from layer1.weekly_synthesizer import WeeklySynthesizer
from models import DetectionEvent
from protocols import PersonaConfig
from settings import ProactiveSettings


def make_event(type_="inactivity", persona="carreira") -> DetectionEvent:
    return DetectionEvent(
        type=type_,
        persona=persona,
        payload={},
        timestamp=datetime.now(tz=timezone.utc),
    )


# ---------------------------------------------------------------------------
# SuggestionGenerator
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_suggestion_prompt_no_verbatim(default_settings, mock_life_log, mock_memupalace):
    """_build_prompt must not include raw message content."""
    gen = SuggestionGenerator(mock_life_log, mock_memupalace, default_settings)
    events = [make_event()]
    from protocols import InteractionRecord
    records = [
        InteractionRecord(intent="study", tools=["python"], timestamp=datetime.now(tz=timezone.utc))
    ]
    prompt = gen._build_prompt(events, records, {}, "en")
    # Should include safe fields
    assert "intent" in prompt
    assert "tools" in prompt
    # Should NOT include any forbidden field names
    assert "raw_message" not in prompt
    assert "embedding" not in prompt


@pytest.mark.asyncio
async def test_suggestion_ignores_low_weight_persona(default_settings, mock_life_log, mock_memupalace):
    """Persona with weight < 0.1 must not contribute records."""
    gen = SuggestionGenerator(mock_life_log, mock_memupalace, default_settings)
    personas = [PersonaConfig(slug="musica", current_weight=0.05)]
    with patch.object(gen, "_call_llm", new_callable=AsyncMock, return_value=[]):
        result = await gen.run([], personas)
    mock_life_log.get_persona_summary.assert_not_called()


@pytest.mark.asyncio
async def test_suggestion_skips_when_no_events_no_new_records(
    default_settings, mock_life_log, mock_memupalace
):
    """Skip LLM call when no events and no new records since last cycle."""
    gen = SuggestionGenerator(mock_life_log, mock_memupalace, default_settings)
    gen._last_cycle_record_count = 0
    mock_life_log.get_persona_summary.return_value = {"records": [], "last_interaction": None}
    with patch.object(gen, "_call_llm", new_callable=AsyncMock) as mock_llm:
        result = await gen.run([], [PersonaConfig(slug="carreira", current_weight=0.8)])
        mock_llm.assert_not_called()
    assert result == []


# Propriedade 9: Fallback do SuggestionGenerator
@given(
    types=st.lists(
        st.sampled_from(["inactivity", "memory_staleness", "cve", "temporal_pattern"]),
        min_size=1,
        max_size=5,
        unique=True,
    )
)
@h_settings(max_examples=50, suppress_health_check=[HealthCheck.function_scoped_fixture])
def test_fallback_on_llm_exception(types, default_settings):
    """If LLM raises, fallback returns is_fallback=True suggestions without propagating."""
    mock_ll = AsyncMock()
    mock_ll.get_persona_summary.return_value = {
        "records": [{"intent": "x", "tools": [], "timestamp": datetime.now(tz=timezone.utc)}],
        "last_interaction": datetime.now(tz=timezone.utc),
    }
    gen = SuggestionGenerator(mock_ll, None, default_settings)
    events = [make_event(t, "carreira") for t in types]

    async def run():
        with patch.object(gen, "_call_llm", side_effect=RuntimeError("LLM down")):
            return await gen.run(events, [PersonaConfig(slug="carreira", current_weight=0.8)])

    result = asyncio.run(run())
    assert all(s.is_fallback for s in result)
    assert len(result) == len(types)


# ---------------------------------------------------------------------------
# WeeklySynthesizer
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_weekly_no_events_returns_minimal_summary(default_settings, mock_life_log):
    synth = WeeklySynthesizer(mock_life_log, None, default_settings)
    summary = await synth.run([])
    assert "No proactive events" in summary
    assert "Next suggested actions" in summary


@pytest.mark.asyncio
async def test_weekly_summary_includes_next_actions_max_3(default_settings, mock_life_log, mock_memupalace):
    synth = WeeklySynthesizer(mock_life_log, mock_memupalace, default_settings)
    events = [make_event(t) for t in ["inactivity", "cve", "memory_staleness", "temporal_pattern"]]

    # Use fallback (no API key in tests)
    with patch.object(synth, "_synthesize_with_llm", new_callable=AsyncMock) as mock_llm:
        fallback = (
            "# Weekly\n- item 1\n- item 2\n\n## Next suggested actions\n"
            "- action 1\n- action 2\n- action 3"
        )
        mock_llm.return_value = fallback
        summary = await synth.run(events)

    actions_section = summary.split("## Next suggested actions")[-1] if "## Next suggested actions" in summary else ""
    action_lines = [l for l in actions_section.strip().splitlines() if l.strip().startswith("-")]
    assert len(action_lines) <= 3
