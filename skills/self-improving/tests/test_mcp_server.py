"""Tests for self-improving MCP server (SELF-01/SELF-02)."""

from __future__ import annotations

import json
from pathlib import Path
from unittest.mock import MagicMock, patch

import pytest


class TestSuggestPromotion:
    def test_pattern_not_found_returns_not_found(self):
        from mcp_server import suggest_promotion

        with patch("mcp_server._get_adapter") as mock_adapter:
            mock_adapter.return_value.get_pattern.return_value = None
            result = suggest_promotion("nonexistent", "mario")
        assert result["status"] == "not_found"
        assert result["eligible"] is False

    def test_suggest_always_pending_approval_never_applies(self):
        """D-11 invariant: status must be pending_approval when eligible."""
        from mcp_server import suggest_promotion

        mock_pattern = MagicMock()
        with (
            patch("mcp_server._get_adapter") as mock_adapter,
            patch("mcp_server.should_promote", return_value=(True, "Eligible")),
            patch("mcp_server._save_suggestion"),
        ):
            mock_adapter.return_value.get_pattern.return_value = mock_pattern
            mock_adapter.return_value.get_current_weight.return_value = 0.5
            result = suggest_promotion("pattern-123", "mario")
        assert result["status"] == "pending_approval"
        assert result["eligible"] is True

    def test_ineligible_pattern_still_returns_pending_approval(self):
        """D-11 invariant applies even when not eligible (status field present)."""
        from mcp_server import suggest_promotion

        mock_pattern = MagicMock()
        with (
            patch("mcp_server._get_adapter") as mock_adapter,
            patch("mcp_server.should_promote", return_value=(False, "Not enough occurrences")),
        ):
            mock_adapter.return_value.get_pattern.return_value = mock_pattern
            mock_adapter.return_value.get_current_weight.return_value = 0.5
            result = suggest_promotion("pattern-123", "mario")
        assert result["status"] == "pending_approval"
        assert result["eligible"] is False

    def test_empty_pattern_id_raises(self):
        from mcp_server import suggest_promotion

        with pytest.raises(ValueError, match="pattern_id"):
            suggest_promotion("", "mario")

    def test_empty_persona_slug_raises(self):
        from mcp_server import suggest_promotion

        with pytest.raises(ValueError, match="persona_slug"):
            suggest_promotion("pattern-123", "")


class TestListPendingSuggestions:
    def test_no_file_returns_empty_list(self, tmp_path, monkeypatch):
        import mcp_server

        monkeypatch.setattr(mcp_server, "SUGGESTIONS_FILE", tmp_path / "suggestions.jsonl")
        result = mcp_server.list_pending_suggestions()
        assert result == []

    def test_returns_only_pending_approval(self, tmp_path, monkeypatch):
        import mcp_server

        sf = tmp_path / "suggestions.jsonl"
        sf.write_text(
            json.dumps({"status": "pending_approval", "pattern_id": "p1"})
            + "\n"
            + json.dumps({"status": "applied", "pattern_id": "p2"})
            + "\n"
        )
        monkeypatch.setattr(mcp_server, "SUGGESTIONS_FILE", sf)
        result = mcp_server.list_pending_suggestions()
        assert len(result) == 1
        assert result[0]["pattern_id"] == "p1"

    def test_skips_malformed_json_lines(self, tmp_path, monkeypatch):
        import mcp_server

        sf = tmp_path / "suggestions.jsonl"
        sf.write_text(
            "not-json\n"
            + json.dumps({"status": "pending_approval", "pattern_id": "p3"})
            + "\n"
        )
        monkeypatch.setattr(mcp_server, "SUGGESTIONS_FILE", sf)
        result = mcp_server.list_pending_suggestions()
        assert len(result) == 1
        assert result[0]["pattern_id"] == "p3"


class TestObserveUsage:
    def test_observe_usage_returns_observed_true(self):
        import asyncio

        from mcp_server import observe_usage

        with patch("mcp_server._add_to_memupalace") as mock_add:
            mock_add.return_value = None

            async def run():
                return await observe_usage("my-skill", "mario", True, "ctx")

            result = asyncio.get_event_loop().run_until_complete(run())

        assert result["observed"] is True
        assert result["skill_name"] == "my-skill"
        assert result["persona_slug"] == "mario"

    def test_context_summary_truncated_to_200_chars(self):
        """T-03-05-02: context_summary injection mitigated by 200-char truncation."""
        import asyncio

        from mcp_server import observe_usage

        long_summary = "x" * 300
        captured = []

        async def mock_add(content, wing="skill-usage"):
            captured.append(content)

        async def run():
            with patch("mcp_server._add_to_memupalace", side_effect=mock_add):
                return await observe_usage("my-skill", "mario", True, long_summary)

        asyncio.get_event_loop().run_until_complete(run())
        assert len(captured) == 1
        # The summary segment should be truncated to 200 chars
        assert "x" * 201 not in captured[0]
        assert "x" * 200 in captured[0]

    def test_empty_skill_name_raises(self):
        import asyncio

        from mcp_server import observe_usage

        async def run():
            return await observe_usage("", "mario", True)

        with pytest.raises(ValueError, match="skill_name"):
            asyncio.get_event_loop().run_until_complete(run())

    def test_empty_persona_slug_raises(self):
        import asyncio

        from mcp_server import observe_usage

        async def run():
            return await observe_usage("skill", "", True)

        with pytest.raises(ValueError, match="persona_slug"):
            asyncio.get_event_loop().run_until_complete(run())
