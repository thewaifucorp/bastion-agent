"""Hexagonal ports (Protocols) for the proactive-engine skill."""

from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime
from typing import Any, Protocol, runtime_checkable


@dataclass
class PersonaConfig:
    slug: str
    current_weight: float


@dataclass
class InteractionRecord:
    intent: str
    tools: list[str]
    timestamp: datetime


@runtime_checkable
class LifeLogProtocol(Protocol):
    async def get_persona_summary(
        self, persona: str, days: int
    ) -> dict[str, Any]:
        """Return summary dict with 'records' list and 'last_interaction' datetime|None."""
        ...

    async def query_temporal_patterns(
        self, personas: list[str], min_occurrences: int
    ) -> list[dict[str, Any]]:
        """Return rows with persona, day_of_week, hour_bucket, cnt."""
        ...


@runtime_checkable
class MemupalaceProtocol(Protocol):
    async def search(
        self, query: str, location: str | None, limit: int
    ) -> list[Any]:
        """Search memories by query."""
        ...

    async def add(
        self,
        content: str,
        wing: str,
        hall: str,
        room: str,
        metadata: dict[str, Any] | None = None,
    ) -> None:
        """Persist a memory."""
        ...

    async def get_stale(
        self, before_days: int, exclude_wing: str | None = None
    ) -> list[Any]:
        """Return memories not reinforced in the last before_days days."""
        ...


@runtime_checkable
class ClawHubClient(Protocol):
    async def get_cves(self, skill_name: str) -> list[dict[str, str]]:
        """Return CVE records for the given skill."""
        ...

    async def get_batch_cves(
        self, skill_names: list[str]
    ) -> dict[str, list[dict[str, str]]]:
        """Return mapping of skill name to CVE records."""
        ...
