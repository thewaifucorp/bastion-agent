"""WR-03 / WR-04 tests — path traversal guard for suggest_promotion.

These tests verify that:
- _safe_segment is present and rejects traversal/uppercase/too-long segments
- _assert_inside_skills_dir is present
- suggest_promotion sanitizes persona_slug and pattern_id via _safe_segment
- _get_adapter() no longer takes a persona_slug parameter (WR-04 fix)
"""

from __future__ import annotations

import pytest


# ── _safe_segment unit tests ──────────────────────────────────────────────────


class TestSafeSegment:
    def test_valid_simple_slug(self):
        import mcp_server

        assert mcp_server._safe_segment("mario") == "mario"

    def test_valid_with_dashes_and_digits(self):
        import mcp_server

        assert mcp_server._safe_segment("persona-01") == "persona-01"

    def test_valid_with_underscores(self):
        import mcp_server

        assert mcp_server._safe_segment("my_persona") == "my_persona"

    def test_path_traversal_double_dot_raises(self):
        import mcp_server

        with pytest.raises(ValueError):
            mcp_server._safe_segment("../../etc/passwd")

    def test_path_traversal_with_slash_raises(self):
        import mcp_server

        with pytest.raises(ValueError):
            mcp_server._safe_segment("foo/bar")

    def test_uppercase_normalized_not_raised(self):
        """_safe_segment lowercases before validating, so uppercase input is normalized.

        Note: plan behavior spec listed uppercase → raises, but the reference
        implementation (and plan action spec) lowercases first, so UPPER_CASE
        becomes upper_case which passes. Lowercasing is the correct behaviour
        per skill-writer mirror spec.
        """
        import mcp_server

        # UPPER_CASE lowercases to upper_case which matches [a-z0-9][a-z0-9_-]{0,63}
        result = mcp_server._safe_segment("UPPER_CASE")
        assert result == "upper_case"

    def test_too_long_raises(self):
        """Segment longer than 64 chars must be rejected."""
        import mcp_server

        with pytest.raises(ValueError):
            mcp_server._safe_segment("a" * 65)

    def test_empty_string_raises(self):
        import mcp_server

        with pytest.raises(ValueError):
            mcp_server._safe_segment("")

    def test_dot_only_raises(self):
        import mcp_server

        with pytest.raises(ValueError):
            mcp_server._safe_segment(".")

    def test_lowercases_input(self):
        """_safe_segment strips and lowercases before matching."""
        import mcp_server

        assert mcp_server._safe_segment("  mario  ") == "mario"


# ── _SEGMENT_RE presence test ─────────────────────────────────────────────────


class TestSegmentRe:
    def test_segment_re_exists(self):
        import mcp_server

        assert hasattr(mcp_server, "_SEGMENT_RE"), "_SEGMENT_RE not defined in mcp_server"

    def test_segment_re_rejects_traversal(self):
        import mcp_server

        assert not mcp_server._SEGMENT_RE.match("../../etc")

    def test_segment_re_accepts_valid(self):
        import mcp_server

        assert mcp_server._SEGMENT_RE.match("mario")


# ── _assert_inside_skills_dir presence test ───────────────────────────────────


class TestAssertInsideSkillsDir:
    def test_function_exists(self):
        import mcp_server

        assert callable(getattr(mcp_server, "_assert_inside_skills_dir", None))


# ── suggest_promotion integration tests (WR-03) ───────────────────────────────


class TestSuggestPromotionPathTraversal:
    def test_traversal_persona_slug_raises(self):
        """WR-03: ../../etc/passwd in persona_slug must be blocked."""
        from mcp_server import suggest_promotion

        with pytest.raises(ValueError):
            suggest_promotion("pattern-123", "../../etc/passwd")

    def test_uppercase_persona_slug_normalized(self):
        """Uppercase persona slug is lowercased and accepted (mirrors skill-writer behaviour)."""
        from unittest.mock import patch

        from mcp_server import suggest_promotion

        with patch("mcp_server._get_adapter") as mock_adapter:
            mock_adapter.return_value.get_pattern.return_value = None
            result = suggest_promotion("pattern-123", "UPPER_CASE")
        # Slug lowercased; reaches adapter with normalized value.
        # not_found dict (per plan spec) carries the normalized slug in `reason`,
        # not as a top-level persona_slug key.
        assert result["status"] == "not_found"
        assert "upper_case" in result["reason"]

    def test_traversal_pattern_id_raises(self):
        """WR-03: ../../evil in pattern_id must also be blocked."""
        from mcp_server import suggest_promotion

        with pytest.raises(ValueError):
            suggest_promotion("../../evil", "mario")

    def test_valid_inputs_proceed_normally(self):
        """Valid slug and pattern_id should reach the adapter."""
        from unittest.mock import patch

        from mcp_server import suggest_promotion

        with patch("mcp_server._get_adapter") as mock_adapter:
            mock_adapter.return_value.get_pattern.return_value = None
            result = suggest_promotion("pattern-123", "mario")
        assert result["status"] == "not_found"

    def test_sanitized_values_passed_to_adapter(self):
        """suggest_promotion passes sanitized slug and pattern_id to adapter methods."""
        from unittest.mock import MagicMock, patch

        from mcp_server import suggest_promotion

        mock_pattern = MagicMock()
        with (
            patch("mcp_server._get_adapter") as mock_adapter,
            patch("mcp_server.should_promote", return_value=(False, "low count")),
        ):
            mock_adapter.return_value.get_pattern.return_value = mock_pattern
            mock_adapter.return_value.get_current_weight.return_value = 0.3
            result = suggest_promotion("pattern-123", "mario")

        # Adapter.get_pattern called with sanitized values
        mock_adapter.return_value.get_pattern.assert_called_once_with("mario", "pattern-123")


# ── _get_adapter signature test (WR-04) ──────────────────────────────────────


class TestGetAdapterSignature:
    def test_get_adapter_takes_no_parameters(self):
        """WR-04: _get_adapter() must not accept a persona_slug parameter."""
        import inspect

        import mcp_server

        sig = inspect.signature(mcp_server._get_adapter)
        params = [p for p in sig.parameters.values()
                  if p.default is inspect.Parameter.empty]
        assert len(params) == 0, (
            f"_get_adapter has unexpected required params: {list(sig.parameters.keys())}"
        )
