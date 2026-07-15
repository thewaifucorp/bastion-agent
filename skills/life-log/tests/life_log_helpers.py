"""
Test helpers for life-log tests — in-memory adapter.
"""

from __future__ import annotations

import uuid
from datetime import datetime, timezone

from db.protocols import InteractionRecord


def _cosine_similarity(a: list[float], b: list[float]) -> float:
    """Pure-Python cosine similarity."""
    if len(a) != len(b):
        raise ValueError("Vectors must have the same dimension")
    dot = sum(x * y for x, y in zip(a, b))
    norm_a = sum(x * x for x in a) ** 0.5
    norm_b = sum(x * x for x in b) ** 0.5
    if norm_a == 0.0 or norm_b == 0.0:
        return 0.0
    return dot / (norm_a * norm_b)


class InMemoryLifeLogAdapter:
    """
    Minimal in-memory LifeLogProtocol implementation.
    Stores records in a list — no filesystem or SQLite I/O.
    """

    def __init__(self) -> None:
        self._records: list[InteractionRecord] = []

    async def log_interaction(
        self,
        persona: str,
        intent: str,
        tools: list[str],
        embedding: list[float],
        timestamp: datetime,
    ) -> str:
        interaction_id = str(uuid.uuid4())
        self._records.append(
            InteractionRecord(
                id=interaction_id,
                persona=persona,
                intent=intent,
                tools=list(tools),
                embedding=list(embedding),
                timestamp=timestamp,
            )
        )
        return interaction_id

    async def search_similar(
        self,
        query_embedding: list[float],
        persona: str | None,
        limit: int,
        threshold: float,
    ) -> list[InteractionRecord]:
        candidates = (
            [r for r in self._records if r.persona == persona]
            if persona is not None
            else list(self._records)
        )
        scored = [
            (sim, r)
            for r in candidates
            if (sim := _cosine_similarity(query_embedding, r.embedding)) >= threshold
        ]
        scored.sort(key=lambda t: t[0], reverse=True)
        return [r for _, r in scored[:limit]]

    async def get_persona_summary(
        self,
        persona: str,
        days: int,
    ) -> list[InteractionRecord]:
        from datetime import timedelta

        cutoff = datetime.now(tz=timezone.utc) - timedelta(days=days)
        return sorted(
            [
                r
                for r in self._records
                if r.persona == persona and r.timestamp.astimezone(timezone.utc) >= cutoff
            ],
            key=lambda r: r.timestamp,
            reverse=True,
        )

    async def get_last_interactions(
        self,
        personas: list[str],
    ) -> dict[str, datetime | None]:
        result: dict[str, datetime | None] = {p: None for p in personas}
        for record in self._records:
            if record.persona in result:
                current_last = result[record.persona]
                if current_last is None or record.timestamp > current_last:
                    result[record.persona] = record.timestamp
        return result

    @property
    def all_records(self) -> list[InteractionRecord]:
        return list(self._records)
