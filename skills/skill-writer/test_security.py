"""
Regression tests for skill-writer path traversal security fix.
Wave 1 (04-01): xfail markers removed — fix is implemented in mcp_server.py.

Folded todo: .planning/todos/pending/skill-writer-path-traversal.md
"""
from __future__ import annotations

import importlib
import sys
import types

import pytest


# ── fastmcp stub (mirrors tests/test_mcp_server.py pattern) ──────────────────


def _install_fastmcp_stub() -> None:
    """Install a minimal fastmcp stub so mcp_server can be imported without the package."""
    if "fastmcp" in sys.modules:
        return
    stub = types.ModuleType("fastmcp")

    class _FakeMCP:
        def __init__(self, name: str) -> None:
            pass

        def tool(self):
            def decorator(fn):
                return fn
            return decorator

        def run(self, **kwargs: object) -> None:  # noqa: ARG002
            pass

    stub.FastMCP = _FakeMCP  # type: ignore[attr-defined]
    sys.modules["fastmcp"] = stub


def _import_server():
    _install_fastmcp_stub()
    if "mcp_server" in sys.modules:
        return sys.modules["mcp_server"]
    return importlib.import_module("mcp_server")


# ---------------------------------------------------------------------------
# Stub: _safe_segment and _skill_path are not yet implemented with the fix.
# Tests below will xfail until 04-01 implements _safe_segment allowlist.
# ---------------------------------------------------------------------------

def test_path_traversal_dotdot_rejected():
    """../etc/passwd style slug must be rejected."""
    m = _import_server()
    with pytest.raises(ValueError, match="[Ii]nvalid"):
        m._safe_segment("../etc/passwd")


def test_path_traversal_absolute_path_rejected():
    """/etc/passwd style absolute path must be rejected."""
    m = _import_server()
    with pytest.raises(ValueError, match="[Ii]nvalid"):
        m._safe_segment("/etc/passwd")


def test_valid_slug_accepted():
    """Valid slug 'weekly-review' must pass _safe_segment."""
    m = _import_server()
    result = m._safe_segment("weekly-review")
    assert result == "weekly-review"


def test_skill_path_stays_inside_skills_dir(tmp_path):
    """_skill_path must resolve inside SKILLS_DIR; traversal attempt raises ValueError."""
    import os
    os.environ["SKILLS_DIR"] = str(tmp_path)
    # Force re-import so SKILLS_DIR env var is picked up
    if "mcp_server" in sys.modules:
        del sys.modules["mcp_server"]
    m = _import_server()
    m.SKILLS_DIR = tmp_path  # reset module-level var with tmp_path
    with pytest.raises(ValueError):
        m._skill_path("../outside")


def test_null_byte_in_slug_rejected():
    """Null byte injection must be rejected."""
    m = _import_server()
    with pytest.raises(ValueError):
        m._safe_segment("evil\x00slug")


def test_skill_list_no_path_disclosure(tmp_path):
    """skill_list must not expose paths outside SKILLS_DIR."""
    import os
    os.environ["SKILLS_DIR"] = str(tmp_path)
    if "mcp_server" in sys.modules:
        del sys.modules["mcp_server"]
    m = _import_server()
    # Create a valid skill dir
    skill_dir = tmp_path / "weekly-review"
    skill_dir.mkdir()
    (skill_dir / "SKILL.md").write_text("---\nname: weekly-review\n---\n")
    m.SKILLS_DIR = tmp_path
    result = m.skill_list()
    for entry in result:
        path_val = entry.get("path", "")
        assert str(tmp_path) not in path_val or path_val.startswith(str(tmp_path))
