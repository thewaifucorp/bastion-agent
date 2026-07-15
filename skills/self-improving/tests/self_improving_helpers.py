"""
Test helpers for self-improving tests — in-memory persistence adapter.
"""

from __future__ import annotations

from datetime import datetime, timezone

from promotion import Pattern, MemoryTier, PromotionPersistenceProtocol


class InMemoryPromotionAdapter:
    """
    Minimal in-memory implementation of PromotionPersistenceProtocol.
    No filesystem I/O — safe for property-based tests.
    """

    def __init__(self, weights: dict[str, float] | None = None) -> None:
        self._weights: dict[str, float] = dict(weights or {})
        self._patterns: dict[str, dict[str, Pattern]] = {}  # slug → {id → Pattern}
        self._history: dict[str, list[tuple[datetime, str, str, str]]] = {}

    # ------------------------------------------------------------------
    # PromotionPersistenceProtocol
    # ------------------------------------------------------------------

    def get_pattern(self, persona_slug: str, pattern_id: str) -> Pattern | None:
        return self._patterns.get(persona_slug, {}).get(pattern_id)

    def save_pattern(self, pattern: Pattern) -> None:
        self._patterns.setdefault(pattern.persona_slug, {})[pattern.id] = pattern

    def get_current_weight(self, persona_slug: str) -> float:
        return self._weights.get(persona_slug, 0.0)

    def append_promotion_history(
        self,
        persona_slug: str,
        timestamp: datetime,
        pattern_id: str,
        action: str,
        justification: str,
    ) -> None:
        self._history.setdefault(persona_slug, []).append(
            (timestamp, pattern_id, action, justification)
        )

    # ------------------------------------------------------------------
    # Test helpers
    # ------------------------------------------------------------------

    def get_history(self, persona_slug: str) -> list[tuple[datetime, str, str, str]]:
        return list(self._history.get(persona_slug, []))

    def get_saved_pattern(self, persona_slug: str, pattern_id: str) -> Pattern | None:
        return self._patterns.get(persona_slug, {}).get(pattern_id)

    def set_weight(self, persona_slug: str, weight: float) -> None:
        self._weights[persona_slug] = weight
