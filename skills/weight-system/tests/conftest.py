"""Shared fixtures for weight-system tests."""

from __future__ import annotations

import pytest

from weight_system_helpers import InMemoryWeightAdapter


@pytest.fixture
def adapter() -> InMemoryWeightAdapter:
    """Fresh in-memory adapter with a default persona for each test."""
    return InMemoryWeightAdapter(initial_weights={"tech-lead": 0.7})
