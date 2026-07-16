"""Nyquist gap tests for skill-writer (SKWR-01, SKWR-03, SKWR-04).

Dependency-light: no network, no LLM, no chromadb.
"""
from __future__ import annotations

import ast
import sys
import time
import types
from pathlib import Path

import pytest

# ---------------------------------------------------------------------------
# FastMCP stub (mirrors test_mcp_server.py pattern)
# ---------------------------------------------------------------------------


def _install_fastmcp_stub() -> None:
    if "fastmcp" in sys.modules:
        return
    stub = types.ModuleType("fastmcp")

    class _FakeMCP:
        def __init__(self, name: str) -> None:
            self.name = name

        def tool(self):
            def decorator(fn):
                return fn
            return decorator

        def run(self, **kwargs: object) -> None:
            pass

    stub.FastMCP = _FakeMCP
    sys.modules["fastmcp"] = stub


_install_fastmcp_stub()


_SKILL_WRITER_DIR = Path(__file__).parent.parent
_MCP_SERVER_PATH = _SKILL_WRITER_DIR / "mcp_server.py"
# Unique module name to avoid collision with self-improving's mcp_server
_MODULE_KEY = "skill_writer_mcp_server_nyquist"


def _import_server():
    """Import skill-writer's mcp_server by absolute path.

    Uses importlib.util.spec_from_file_location so the module is always
    loaded from skills/skill-writer/mcp_server.py, regardless of sys.path
    ordering (avoids collision with skills/self-improving/mcp_server.py).
    """
    import importlib.util

    if _MODULE_KEY in sys.modules:
        return sys.modules[_MODULE_KEY]

    # Ensure skill-writer dir is on sys.path so internal imports (versioning,
    # distiller) resolve correctly from within the module.
    sw_dir = str(_SKILL_WRITER_DIR)
    if sw_dir not in sys.path:
        sys.path.insert(0, sw_dir)

    spec = importlib.util.spec_from_file_location(_MODULE_KEY, str(_MCP_SERVER_PATH))
    mod = importlib.util.module_from_spec(spec)
    sys.modules[_MODULE_KEY] = mod
    spec.loader.exec_module(mod)
    return mod


# ---------------------------------------------------------------------------
# SKWR-01: skill-writer exposes exactly 5 MCP tools
#
# Requirement: "skill-writer é MCP server Python isolado"
# Observable behavior: server registers 5 tools.
# ---------------------------------------------------------------------------


class TestSKWR01ToolCount:
    """SKWR-01: skill-writer MCP server must expose exactly 5 tools."""

    def test_skill_writer_registers_exactly_5_tools_via_ast(self):
        """Count @mcp.tool() decorators in skill-writer/mcp_server.py."""
        src_path = Path(__file__).parent.parent / "mcp_server.py"
        assert src_path.exists(), f"mcp_server.py not found at {src_path}"
        tree = ast.parse(src_path.read_text(encoding="utf-8"))

        tool_defs: list[str] = []
        for node in ast.walk(tree):
            if not isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
                continue
            for decorator in node.decorator_list:
                if (
                    isinstance(decorator, ast.Call)
                    and isinstance(decorator.func, ast.Attribute)
                    and decorator.func.attr == "tool"
                ):
                    tool_defs.append(node.name)
                    break

        assert len(tool_defs) == 5, (
            f"SKWR-01: expected 5 @mcp.tool() functions, found {len(tool_defs)}: {tool_defs}"
        )

    def test_skill_writer_expected_tool_names_present(self):
        """SKWR-01/02/03/04/06: the 5 tools are the required ones."""
        src_path = Path(__file__).parent.parent / "mcp_server.py"
        tree = ast.parse(src_path.read_text(encoding="utf-8"))

        tool_defs: set[str] = set()
        for node in ast.walk(tree):
            if not isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
                continue
            for decorator in node.decorator_list:
                if (
                    isinstance(decorator, ast.Call)
                    and isinstance(decorator.func, ast.Attribute)
                    and decorator.func.attr == "tool"
                ):
                    tool_defs.add(node.name)
                    break

        expected = {
            "skill_create",
            "skill_edit",
            "skill_rollback",
            "skill_distill_candidate",
            "skill_list",
        }
        assert tool_defs == expected, (
            f"SKWR-01: tool name mismatch. expected={expected}, got={tool_defs}"
        )

    def test_skill_writer_uses_streamable_http_transport(self):
        """SKWR-01: server must use streamable-http transport."""
        src_path = Path(__file__).parent.parent / "mcp_server.py"
        src = src_path.read_text(encoding="utf-8")
        assert "streamable-http" in src, (
            "SKWR-01: mcp_server.py must specify transport='streamable-http'"
        )


# ---------------------------------------------------------------------------
# SKWR-03: skill_edit edits an existing skill via conversation
#
# Requirement: "Edita skill existente conforme conversa"
# Observable behavior: skill_edit snapshots, calls gateway, writes new content,
#   returns skill_reloaded=True.
# ---------------------------------------------------------------------------


class TestSKWR03SkillEdit:
    """SKWR-03: skill_edit must snapshot → call gateway → write → signal reload."""

    @pytest.mark.asyncio
    async def test_skill_edit_snapshots_before_writing(self, tmp_path):
        """skill_edit must snapshot the existing skill before any modification."""
        from unittest.mock import AsyncMock, patch

        mod = _import_server()
        original_skills_dir = mod.SKILLS_DIR
        mod.SKILLS_DIR = tmp_path

        skill_dir = tmp_path / "my-skill"
        skill_dir.mkdir()
        skill_path = skill_dir / "SKILL.md"
        skill_path.write_text("original content", encoding="utf-8")

        snapshots_taken: list[Path] = []

        original_snapshot = None
        import versioning as _versioning
        original_snapshot = _versioning.snapshot

        def mock_snapshot(p: Path) -> None:
            snapshots_taken.append(p)
            original_snapshot(p)

        with (
            patch.object(mod, "snapshot", side_effect=mock_snapshot),
            patch.object(mod, "_build_pattern_context", new=AsyncMock(return_value="")),
            patch.object(mod, "_call_gateway", new=AsyncMock(return_value="updated content")),
        ):
            await mod.skill_edit(
                name="my-skill",
                edit_instructions="make it shorter",
            )

        mod.SKILLS_DIR = original_skills_dir

        assert len(snapshots_taken) >= 1, (
            "SKWR-03: skill_edit must call snapshot() before writing — "
            "versioning invariant violated (snapshot count=0)"
        )

    @pytest.mark.asyncio
    async def test_skill_edit_writes_gateway_response_to_file(self, tmp_path):
        """skill_edit must write the gateway-returned content to SKILL.md."""
        from unittest.mock import AsyncMock, patch

        mod = _import_server()
        original_skills_dir = mod.SKILLS_DIR
        mod.SKILLS_DIR = tmp_path

        skill_dir = tmp_path / "my-skill"
        skill_dir.mkdir()
        skill_path = skill_dir / "SKILL.md"
        skill_path.write_text("old content", encoding="utf-8")

        new_content = "<name>my-skill</name>\n<description>updated</description>"

        with (
            patch.object(mod, "snapshot"),
            patch.object(mod, "_build_pattern_context", new=AsyncMock(return_value="")),
            patch.object(mod, "_call_gateway", new=AsyncMock(return_value=new_content)),
        ):
            await mod.skill_edit(
                name="my-skill",
                edit_instructions="add description",
            )

        mod.SKILLS_DIR = original_skills_dir

        assert skill_path.read_text(encoding="utf-8") == new_content, (
            "SKWR-03: skill_edit must write the gateway response to SKILL.md"
        )

    @pytest.mark.asyncio
    async def test_skill_edit_returns_skill_reloaded_true(self, tmp_path):
        """skill_edit must return skill_reloaded=True so core SkillsLoader rescans (D-06)."""
        from unittest.mock import AsyncMock, patch

        mod = _import_server()
        original_skills_dir = mod.SKILLS_DIR
        mod.SKILLS_DIR = tmp_path

        skill_dir = tmp_path / "my-skill"
        skill_dir.mkdir()
        (skill_dir / "SKILL.md").write_text("content", encoding="utf-8")

        with (
            patch.object(mod, "snapshot"),
            patch.object(mod, "_build_pattern_context", new=AsyncMock(return_value="")),
            patch.object(mod, "_call_gateway", new=AsyncMock(return_value="new content")),
        ):
            result = await mod.skill_edit(name="my-skill", edit_instructions="improve")

        mod.SKILLS_DIR = original_skills_dir

        assert result["skill_reloaded"] is True, (
            "SKWR-03: skill_edit must return skill_reloaded=True — "
            "without this signal the Rust core never rescans the updated skill (D-06)"
        )

    @pytest.mark.asyncio
    async def test_skill_edit_queued_when_gateway_unavailable(self, tmp_path):
        """SKWR-03: when gateway returns None, edit returns queued status."""
        from unittest.mock import AsyncMock, patch

        mod = _import_server()
        original_skills_dir = mod.SKILLS_DIR
        mod.SKILLS_DIR = tmp_path

        skill_dir = tmp_path / "my-skill"
        skill_dir.mkdir()
        (skill_dir / "SKILL.md").write_text("content", encoding="utf-8")

        with (
            patch.object(mod, "snapshot"),
            patch.object(mod, "_build_pattern_context", new=AsyncMock(return_value="")),
            patch.object(mod, "_call_gateway", new=AsyncMock(return_value=None)),
        ):
            result = await mod.skill_edit(name="my-skill", edit_instructions="improve")

        mod.SKILLS_DIR = original_skills_dir

        assert result["skill_reloaded"] is False
        assert result["status"] == "queued", (
            "SKWR-03: when gateway unavailable, skill_edit must return status='queued'"
        )

    @pytest.mark.asyncio
    async def test_skill_edit_nonexistent_skill_raises(self, tmp_path):
        """SKWR-03: editing a non-existent skill must raise ValueError."""
        mod = _import_server()
        original_skills_dir = mod.SKILLS_DIR
        mod.SKILLS_DIR = tmp_path

        with pytest.raises(ValueError):
            await mod.skill_edit(name="no-such-skill", edit_instructions="fix it")

        mod.SKILLS_DIR = original_skills_dir

    @pytest.mark.asyncio
    async def test_skill_edit_includes_existing_content_in_prompt(self, tmp_path):
        """SKWR-03: the gateway prompt must contain the current SKILL.md content.

        Without this, the LLM has no context for what to edit — the edit
        would be a creation from scratch, not a conversation-driven edit.
        """
        from unittest.mock import AsyncMock, patch

        mod = _import_server()
        original_skills_dir = mod.SKILLS_DIR
        mod.SKILLS_DIR = tmp_path

        skill_dir = tmp_path / "edit-skill"
        skill_dir.mkdir()
        existing_content = "<name>edit-skill</name>\n<description>original desc</description>"
        (skill_dir / "SKILL.md").write_text(existing_content, encoding="utf-8")

        captured_prompts: list[str] = []

        async def mock_call_gateway(prompt: str, context_tier: str = "cloud_ok") -> str:
            captured_prompts.append(prompt)
            return "updated content"

        with (
            patch.object(mod, "snapshot"),
            patch.object(mod, "_build_pattern_context", new=AsyncMock(return_value="")),
            patch.object(mod, "_call_gateway", new=mock_call_gateway),
        ):
            await mod.skill_edit(name="edit-skill", edit_instructions="make it shorter")

        mod.SKILLS_DIR = original_skills_dir

        assert len(captured_prompts) == 1
        assert existing_content in captured_prompts[0], (
            "SKWR-03: skill_edit must include the current SKILL.md content in the gateway prompt — "
            "the LLM needs the existing content to apply a conversational edit"
        )


# ---------------------------------------------------------------------------
# SKWR-04: rollback is deterministic (no timing dependency)
#
# Requirement: "Versiona skills geradas (histórico, rollback)"
# Observable behavior: rollback_to_date restores file from a known snapshot,
#   content is correct, no race condition with timestamp collision.
# ---------------------------------------------------------------------------


class TestSKWR04RollbackDeterministic:
    """SKWR-04: rollback_to_date must be deterministic regardless of timestamp collision."""

    def test_rollback_restores_correct_content_with_explicit_snapshot(self, tmp_path):
        """SKWR-04: rollback restores the correct snapshot content.

        Uses a pre-placed snapshot with a known timestamp to avoid any
        timing dependency. The snapshot file is created directly in .versions/
        rather than via snapshot() to eliminate the async race.
        """
        from versioning import SNAPSHOT_PREFIX, rollback_to_date

        p = tmp_path / "SKILL.md"
        p.write_text("current content", encoding="utf-8")

        # Place a snapshot with a known past timestamp directly (no async)
        versions_dir = tmp_path / ".versions"
        versions_dir.mkdir()
        snap_ts = "20260601T120000Z"
        snap_file = versions_dir / f"{SNAPSHOT_PREFIX}{snap_ts}"
        snap_file.write_text("restored content", encoding="utf-8")

        restored = rollback_to_date(p, "2026-06-01")

        assert restored is not None, (
            "SKWR-04: rollback_to_date returned None — expected to find snapshot for 2026-06-01"
        )
        # Wait for the async snapshot of "current content" to complete
        time.sleep(0.2)

        final_content = p.read_text(encoding="utf-8")
        assert final_content == "restored content", (
            f"SKWR-04: rollback restored wrong content: {final_content!r}, "
            "expected 'restored content'"
        )

    def test_rollback_unknown_date_returns_none(self, tmp_path):
        """SKWR-04: rollback with unrecognised date_hint returns None gracefully."""
        from versioning import rollback_to_date

        p = tmp_path / "SKILL.md"
        p.write_text("content", encoding="utf-8")

        result = rollback_to_date(p, "not-a-date-at-all")
        assert result is None, (
            "SKWR-04: rollback_to_date must return None for unrecognised date_hint"
        )

    def test_rollback_no_snapshots_returns_none(self, tmp_path):
        """SKWR-04: rollback with no snapshots returns None."""
        from versioning import rollback_to_date

        p = tmp_path / "SKILL.md"
        p.write_text("content", encoding="utf-8")
        # No .versions/ dir created

        result = rollback_to_date(p, "2026-06-01")
        assert result is None

    def test_rollback_preserves_pre_rollback_state_no_timestamp_collision(self, tmp_path):
        """SKWR-04: rollback snapshots current state; deterministic via distinct timestamps.

        Unlike test_versioning_rollback.py (which has a timing race when both
        snapshot and rollback happen within the same second), this test places the
        initial snapshot with a past timestamp so the rollback snapshot gets a
        DIFFERENT (current) timestamp — no collision possible.
        """
        from versioning import rollback_to_date, list_snapshots, SNAPSHOT_PREFIX

        p = tmp_path / "SKILL.md"
        p.write_text("current content to preserve", encoding="utf-8")

        # Place a past snapshot to be the rollback target
        versions_dir = tmp_path / ".versions"
        versions_dir.mkdir()
        # Use a fixed past timestamp well before "today"
        snap_ts = "20250101T000000Z"
        snap_file = versions_dir / f"{SNAPSHOT_PREFIX}{snap_ts}"
        snap_file.write_text("old content", encoding="utf-8")

        snaps_before = len(list_snapshots(p))
        assert snaps_before == 1

        restored = rollback_to_date(p, "2025-01-01")
        assert restored is not None

        # Wait for the async snapshot of "current content to preserve"
        time.sleep(0.2)

        snaps_after = list_snapshots(p)
        # rollback_to_date should have taken a snapshot of the pre-rollback state
        assert len(snaps_after) == snaps_before + 1, (
            "SKWR-04: rollback_to_date must snapshot current state before overwriting — "
            "pre-rollback state would be permanently lost"
        )

        all_contents = {s.read_text(encoding="utf-8") for s in snaps_after}
        assert "current content to preserve" in all_contents, (
            "SKWR-04: pre-rollback content 'current content to preserve' must be "
            "preserved in a snapshot — not lost"
        )
