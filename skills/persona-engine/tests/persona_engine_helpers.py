"""
Test helpers for persona-engine tests — in-memory persistence adapter.
"""

from __future__ import annotations

from persona_engine import Persona, PersonaPersistenceProtocol


class InMemoryPersistence:
    """
    Minimal in-memory implementation of PersonaPersistenceProtocol.
    Stores personas in a dict keyed by slug — no filesystem I/O.
    """

    def __init__(self) -> None:
        self._store: dict[str, Persona] = {}

    def write_soul_md(self, persona: Persona) -> None:
        self._store[persona.slug] = persona

    def read_soul_md(self, slug: str) -> Persona:
        if slug not in self._store:
            raise KeyError(f"Persona not found: {slug}")
        return self._store[slug]

    def slug_exists(self, slug: str) -> bool:
        return slug in self._store

    @property
    def all_personas(self) -> list[Persona]:
        return list(self._store.values())
