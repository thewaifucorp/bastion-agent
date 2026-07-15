"""Tests for InsightCache (MUPL-02)."""

from __future__ import annotations

import time

import pytest

from skills.memupalace.insight_cache import InsightCache


class TestInsightCache:
    def test_set_and_get(self):
        cache = InsightCache(ttl_seconds=60)
        cache.set("k1", "insight text")
        assert cache.get("k1") == "insight text"

    def test_miss_returns_none(self):
        cache = InsightCache()
        assert cache.get("nonexistent") is None

    def test_expired_entry_returns_none(self):
        cache = InsightCache(ttl_seconds=0)
        cache.set("k1", "insight")
        time.sleep(0.01)
        assert cache.get("k1") is None

    def test_make_key_stable(self):
        k1 = InsightCache.make_key("same content", "wing-a")
        k2 = InsightCache.make_key("same content", "wing-a")
        assert k1 == k2

    def test_make_key_differs_by_wing(self):
        k1 = InsightCache.make_key("content", "wing-a")
        k2 = InsightCache.make_key("content", "wing-b")
        assert k1 != k2

    def test_make_key_differs_by_content(self):
        k1 = InsightCache.make_key("content-a", "wing")
        k2 = InsightCache.make_key("content-b", "wing")
        assert k1 != k2

    def test_make_key_no_prefix_collision(self):
        """Regression: keys must hash full content, not a prefix.

        Two distinct contents sharing the first 100 chars must produce
        different keys — otherwise the cache-aside guard in memory_add silently
        drops the second store (data loss).
        """
        shared_prefix = "x" * 100
        k1 = InsightCache.make_key(shared_prefix + "-alpha", "wing")
        k2 = InsightCache.make_key(shared_prefix + "-beta", "wing")
        assert k1 != k2

    def test_invalidate_removes_entry(self):
        cache = InsightCache()
        cache.set("k1", "insight")
        cache.invalidate("k1")
        assert cache.get("k1") is None

    def test_invalidate_nonexistent_is_noop(self):
        cache = InsightCache()
        cache.invalidate("does-not-exist")  # should not raise

    def test_len_counts_entries(self):
        cache = InsightCache(ttl_seconds=60)
        assert len(cache) == 0
        cache.set("k1", "v1")
        cache.set("k2", "v2")
        assert len(cache) == 2

    def test_overwrite_updates_value(self):
        cache = InsightCache(ttl_seconds=60)
        cache.set("k1", "first")
        cache.set("k1", "second")
        assert cache.get("k1") == "second"
