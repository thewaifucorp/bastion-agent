"""Shared fixtures for life-log tests."""

from __future__ import annotations

import pytest

from life_log_helpers import InMemoryLifeLogAdapter


@pytest.fixture
def adapter() -> InMemoryLifeLogAdapter:
    """Fresh in-memory adapter for each test."""
    return InMemoryLifeLogAdapter()
