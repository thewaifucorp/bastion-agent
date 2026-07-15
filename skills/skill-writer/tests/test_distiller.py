"""Tests for distiller.py (D-04/D-05/SKWR-06).

CR-03 fix: is_distillation_candidate gates on step count alone.
memupalace_search_fn parameter removed — no search injection required.
"""
from __future__ import annotations

import json
from pathlib import Path

import pytest


class TestIsDistillationCandidate:
    """CR-03 fix: step-count-only gate, no memupalace dependency."""

    def test_zero_steps_returns_false(self):
        from distiller import MIN_STEPS, is_distillation_candidate

        ok, reason = is_distillation_candidate([])
        assert ok is False
        assert "0" in reason
        assert str(MIN_STEPS) in reason

    def test_too_few_steps_returns_false(self):
        from distiller import MIN_STEPS, is_distillation_candidate

        short = ["tool_a", "tool_b", "tool_c"]
        ok, reason = is_distillation_candidate(short)
        assert ok is False
        assert str(MIN_STEPS) in reason

    def test_exactly_min_steps_returns_true(self):
        from distiller import MIN_STEPS, is_distillation_candidate

        calls = [f"tool_{i}" for i in range(MIN_STEPS)]
        ok, reason = is_distillation_candidate(calls)
        assert ok is True
        assert str(MIN_STEPS) in reason

    def test_more_than_min_steps_returns_true(self):
        from distiller import MIN_STEPS, is_distillation_candidate

        calls = [f"tool_{i}" for i in range(MIN_STEPS + 2)]
        ok, reason = is_distillation_candidate(calls)
        assert ok is True

    def test_reason_contains_step_count_when_candidate(self):
        from distiller import is_distillation_candidate

        calls = ["a", "b", "c", "d", "e", "f"]
        ok, reason = is_distillation_candidate(calls)
        assert ok is True
        assert "6" in reason

    def test_no_memupalace_parameter(self):
        """Verify the function signature accepts exactly one argument (CR-03 fix)."""
        import inspect
        from distiller import is_distillation_candidate

        sig = inspect.signature(is_distillation_candidate)
        assert len(sig.parameters) == 1, (
            f"is_distillation_candidate should have 1 param, got {list(sig.parameters)}"
        )

    def test_does_not_require_memupalace(self):
        """No memupalace search fn — always reachable with enough steps."""
        from distiller import is_distillation_candidate

        # Calling with only tool_calls (no second arg) must not raise
        calls = ["a", "b", "c", "d"]
        ok, reason = is_distillation_candidate(calls)
        assert ok is True


class TestEnqueuePending:
    def test_enqueue_creates_jsonl_entry(self, tmp_path, monkeypatch):
        pending_file = tmp_path / "pending_distillations.jsonl"
        import distiller
        monkeypatch.setattr(distiller, "PENDING_FILE", pending_file)
        from distiller import enqueue_pending

        enqueue_pending("summarise meeting notes", "cloud_ok")
        lines = pending_file.read_text(encoding="utf-8").strip().split("\n")
        assert len(lines) == 1
        entry = json.loads(lines[0])
        assert entry["status"] == "pending"
        assert entry["privacy_tier"] == "cloud_ok"
        assert "summarise" in entry["prompt"]

    def test_enqueue_appends_multiple_entries(self, tmp_path, monkeypatch):
        pending_file = tmp_path / "pending_distillations.jsonl"
        import distiller
        monkeypatch.setattr(distiller, "PENDING_FILE", pending_file)
        from distiller import enqueue_pending

        enqueue_pending("prompt1", "cloud_ok")
        enqueue_pending("prompt2", "local_only")
        lines = [ln for ln in pending_file.read_text(encoding="utf-8").strip().split("\n") if ln]
        assert len(lines) == 2

    def test_enqueue_entry_has_timestamp(self, tmp_path, monkeypatch):
        pending_file = tmp_path / "pending_distillations.jsonl"
        import distiller
        monkeypatch.setattr(distiller, "PENDING_FILE", pending_file)
        from distiller import enqueue_pending

        enqueue_pending("test prompt", "local_only")
        entry = json.loads(pending_file.read_text(encoding="utf-8").strip())
        assert "timestamp" in entry
        assert entry["timestamp"]  # non-empty

    def test_enqueue_creates_parent_dir(self, tmp_path, monkeypatch):
        pending_file = tmp_path / "nested" / "dir" / "pending_distillations.jsonl"
        import distiller
        monkeypatch.setattr(distiller, "PENDING_FILE", pending_file)
        from distiller import enqueue_pending

        enqueue_pending("test", "cloud_ok")
        assert pending_file.exists()
