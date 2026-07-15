"""
Test helpers for weight-system tests — in-memory persistence adapter.
"""

from __future__ import annotations

from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from weight_system import WeightHistoryEntry


class InMemoryWeightAdapter:
    """
    Minimal in-memory implementation of WeightPersistenceProtocol.
    Stores weights and history in dicts — no filesystem I/O.
    """

    def __init__(self, initial_weights: dict[str, float] | None = None) -> None:
        self._weights: dict[str, float] = dict(initial_weights or {})
        self._history: dict[str, list[WeightHistoryEntry]] = {}

    def get_current_weight(self, slug: str) -> float:
        if slug not in self._weights:
            raise KeyError(f"Persona slug '{slug}' not found")
        return self._weights[slug]

    def set_current_weight(self, slug: str, weight: float) -> None:
        self._weights[slug] = weight

    def append_weight_history(self, slug: str, entry: WeightHistoryEntry) -> None:
        self._history.setdefault(slug, []).append(entry)

    def get_history(self, slug: str) -> list[WeightHistoryEntry]:
        return list(self._history.get(slug, []))

    @property
    def all_weights(self) -> dict[str, float]:
        return dict(self._weights)
