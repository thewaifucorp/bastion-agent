"""Unit tests for MetricsTracker."""

import json
from pathlib import Path

import pytest

from output_validator.metrics_tracker import MetricsTracker


@pytest.fixture
def metrics_file(tmp_path):
    return tmp_path / "metrics.json"


@pytest.fixture
def tracker(metrics_file):
    return MetricsTracker(metrics_file, window_size=5)


class TestRecordValidation:
    def test_increments_total_counter(self, tracker):
        tracker.record_validation("skill-a", True, [])
        tracker.record_validation("skill-a", True, [])
        assert tracker.metrics["skill-a"]["total"] == 2

    def test_increments_valid_counter(self, tracker):
        tracker.record_validation("skill-a", True, [])
        tracker.record_validation("skill-a", False, ["err"])
        assert tracker.metrics["skill-a"]["valid"] == 1

    def test_recent_deque_respects_window_size(self, tracker):
        for i in range(10):
            tracker.record_validation("skill-b", i % 2 == 0, [])
        assert len(tracker.metrics["skill-b"]["recent"]) == 5  # window_size=5

    def test_saves_last_error(self, tracker):
        tracker.record_validation("skill-c", False, ["missing field"])
        assert tracker.metrics["skill-c"]["last_error"] is not None
        assert "missing field" in tracker.metrics["skill-c"]["last_error"]["errors"]

    def test_no_last_error_when_valid(self, tracker):
        tracker.record_validation("skill-d", True, [])
        assert tracker.metrics["skill-d"]["last_error"] is None

    def test_updates_last_updated(self, tracker):
        tracker.record_validation("skill-e", True, [])
        assert tracker.metrics["skill-e"]["last_updated"] is not None


class TestPersistence:
    def test_saves_to_file(self, tracker, metrics_file):
        tracker.record_validation("skill-x", True, [])
        assert metrics_file.exists()
        data = json.loads(metrics_file.read_text())
        assert "skill-x" in data

    def test_loads_from_file(self, metrics_file):
        # Pre-populate file
        data = {
            "skill-y": {
                "total": 5,
                "valid": 4,
                "recent": [True, True, False, True, True],
                "last_error": None,
                "last_updated": "2024-01-01T00:00:00+00:00",
            }
        }
        metrics_file.write_text(json.dumps(data))
        tracker2 = MetricsTracker(metrics_file, window_size=10)
        assert tracker2.metrics["skill-y"]["total"] == 5

    def test_handles_missing_file(self, tmp_path):
        tracker = MetricsTracker(tmp_path / "nonexistent.json")
        assert tracker.metrics == {}


class TestDriftDetection:
    def test_no_warning_below_min_samples(self, tracker):
        # window_size=5, min_samples=20 — no drift warning with only 5 entries
        for _ in range(5):
            tracker.record_validation("skill-drift", False, ["err"])
        # No exception raised — drift check silently skipped

    def test_drift_detected_with_enough_samples(self, tmp_path):
        tracker = MetricsTracker(tmp_path / "m.json", window_size=100)
        # Record 20 failures to trigger drift check
        for _ in range(20):
            tracker.record_validation("skill-low", False, ["err"])
        # Drift warning should have been logged (no assertion on log, just no crash)
        stats = tracker.get_stats("skill-low")
        assert stats["recent_success_rate"] == 0.0


class TestGetStats:
    def test_returns_stats_for_skill(self, tracker):
        tracker.record_validation("skill-s", True, [])
        tracker.record_validation("skill-s", False, ["e"])
        stats = tracker.get_stats("skill-s")
        assert stats["skill"] == "skill-s"
        assert stats["total_validations"] == 2
        assert stats["total_valid"] == 1
        assert stats["overall_success_rate"] == pytest.approx(0.5)

    def test_returns_all_skills_when_no_name(self, tracker):
        tracker.record_validation("skill-1", True, [])
        tracker.record_validation("skill-2", False, ["e"])
        all_stats = tracker.get_stats()
        assert "skill-1" in all_stats
        assert "skill-2" in all_stats

    def test_returns_empty_for_unknown_skill(self, tracker):
        result = tracker.get_stats("unknown-skill")
        assert result == {}

    def test_recent_success_rate_calculation(self, tracker):
        tracker.record_validation("skill-r", True, [])
        tracker.record_validation("skill-r", True, [])
        tracker.record_validation("skill-r", False, ["e"])
        stats = tracker.get_stats("skill-r")
        assert stats["recent_success_rate"] == pytest.approx(2 / 3)
