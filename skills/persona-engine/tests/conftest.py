"""Shared fixtures for persona-engine tests."""

from __future__ import annotations

import pytest

from persona_engine import Persona
from persona_engine_helpers import InMemoryPersistence


@pytest.fixture
def persistence() -> InMemoryPersistence:
    """Fresh in-memory persistence adapter for each test."""
    return InMemoryPersistence()


@pytest.fixture
def sample_persona() -> Persona:
    """A minimal valid Persona for use in tests."""
    return Persona(
        name="Tech Lead",
        slug="tech-lead",
        base_weight=0.9,
        current_weight=0.9,
        domains=["code", "architecture"],
        trigger_keywords=["PR", "deploy", "bug"],
        clawhub_skills=["github-integration"],
    )
