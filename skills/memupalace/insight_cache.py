"""Insight cache — avoids redundant LLM round-trips (MUPL-02).

TTL-based in-memory cache for distilled insights. Key = hash(full content + wing).
Not persisted — intentional: insights are derived, not source of truth.
"""

from __future__ import annotations

import hashlib
import time
from dataclasses import dataclass


@dataclass
class CachedInsight:
    key: str
    insight: str
    expires_at: float


class InsightCache:
    """In-memory TTL cache for distilled insights.

    Prevents repeated gateway calls for the same content.
    Not persisted — intentional: insights are derived, not source of truth.
    """

    def __init__(self, ttl_seconds: int = 3600) -> None:
        self._cache: dict[str, CachedInsight] = {}
        self._ttl = ttl_seconds

    @staticmethod
    def make_key(content: str, wing: str = "general") -> str:
        """Stable key from full content + wing.

        Hashes the entire content (not a prefix): when used as a cache-aside
        guard over a write path (MUPL-02), keying on a prefix would let two
        distinct entries sharing a leading prefix collide, silently dropping
        the second store. Full-content hashing makes collisions cryptographically
        improbable.
        """
        raw = f"{content}::{wing}"
        return hashlib.sha256(raw.encode()).hexdigest()[:16]

    def get(self, key: str) -> str | None:
        """Return cached insight if not expired, else None."""
        entry = self._cache.get(key)
        if entry is None:
            return None
        if time.time() > entry.expires_at:
            del self._cache[key]
            return None
        return entry.insight

    def set(self, key: str, insight: str) -> None:
        """Cache an insight under *key* with TTL from construction."""
        self._cache[key] = CachedInsight(
            key=key,
            insight=insight,
            expires_at=time.time() + self._ttl,
        )

    def invalidate(self, key: str) -> None:
        """Remove a cached entry immediately."""
        self._cache.pop(key, None)

    def __len__(self) -> int:
        return len(self._cache)
