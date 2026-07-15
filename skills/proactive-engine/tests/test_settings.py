"""Tests for settings.py — ProactiveSettings."""

from __future__ import annotations

import os

import pytest
from hypothesis import HealthCheck, given, settings as h_settings
from hypothesis import strategies as st

from settings import ProactiveSettings


def test_defaults_when_no_env(monkeypatch):
    for key in list(os.environ.keys()):
        if key.startswith("PROACTIVE_"):
            monkeypatch.delenv(key, raising=False)
    s = ProactiveSettings.from_env()
    assert s.inactivity_days == 3
    assert s.staleness_days == 14
    assert s.pattern_min_occurrences == 3
    assert s.lifelog_window == 50
    assert s.dedup_window_hours == 6
    assert s.enabled is True


def test_enabled_false(monkeypatch):
    monkeypatch.setenv("PROACTIVE_ENABLED", "false")
    s = ProactiveSettings.from_env()
    assert s.enabled is False


def test_enabled_zero(monkeypatch):
    monkeypatch.setenv("PROACTIVE_ENABLED", "0")
    s = ProactiveSettings.from_env()
    assert s.enabled is False


def test_custom_inactivity_days(monkeypatch):
    monkeypatch.setenv("PROACTIVE_INACTIVITY_DAYS", "7")
    s = ProactiveSettings.from_env()
    assert s.inactivity_days == 7


def test_invalid_non_integer(monkeypatch):
    monkeypatch.setenv("PROACTIVE_INACTIVITY_DAYS", "abc")
    with pytest.raises(ValueError, match="must be an integer"):
        ProactiveSettings.from_env()


def test_invalid_negative(monkeypatch):
    monkeypatch.setenv("PROACTIVE_INACTIVITY_DAYS", "-1")
    with pytest.raises(ValueError):
        ProactiveSettings.from_env()


def test_invalid_zero(monkeypatch):
    monkeypatch.setenv("PROACTIVE_INACTIVITY_DAYS", "0")
    with pytest.raises(ValueError):
        ProactiveSettings.from_env()


# Propriedade 10: Validação de ProactiveSettings
@given(
    val=st.one_of(
        st.integers(max_value=0),  # zero or negative
        st.text(min_size=1, max_size=10, alphabet=st.characters(blacklist_characters="\x00")).filter(
            lambda s: not s.lstrip("-").isdigit()
        ),
    )
)
@h_settings(max_examples=50)
def test_invalid_numeric_env_raises(val):
    """Any invalid value for a numeric field must raise ValueError."""
    import os

    original = os.environ.pop("PROACTIVE_INACTIVITY_DAYS", None)
    try:
        os.environ["PROACTIVE_INACTIVITY_DAYS"] = str(val)
        with pytest.raises(ValueError):
            ProactiveSettings.from_env()
    finally:
        if original is not None:
            os.environ["PROACTIVE_INACTIVITY_DAYS"] = original
        else:
            os.environ.pop("PROACTIVE_INACTIVITY_DAYS", None)
