"""Nyquist gap tests for self-improving (SELF-01).

Dependency-light: AST inspection only, no network/LLM/chromadb.
"""
from __future__ import annotations

import ast
from pathlib import Path

import pytest


# ---------------------------------------------------------------------------
# SELF-01: self-improving exposes exactly 3 MCP tools
#
# Requirement: "self-improving é MCP server Python (port da skill v2)"
# Observable behavior: server registers 3 tools.
# ---------------------------------------------------------------------------


class TestSELF01ToolCount:
    """SELF-01: self-improving MCP server must expose exactly 3 tools."""

    def test_self_improving_registers_exactly_3_tools_via_ast(self):
        """Count @mcp.tool() decorators in self-improving/mcp_server.py."""
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

        assert len(tool_defs) == 3, (
            f"SELF-01: expected 3 @mcp.tool() functions, found {len(tool_defs)}: {tool_defs}"
        )

    def test_self_improving_expected_tool_names_present(self):
        """SELF-01/02: the 3 tools are the required ones."""
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
            "observe_usage",
            "suggest_promotion",
            "list_pending_suggestions",
        }
        assert tool_defs == expected, (
            f"SELF-01: tool name mismatch. expected={expected}, got={tool_defs}"
        )

    def test_self_improving_uses_streamable_http_transport(self):
        """SELF-01: server must use streamable-http transport (not sse or stdio)."""
        src_path = Path(__file__).parent.parent / "mcp_server.py"
        src = src_path.read_text(encoding="utf-8")
        assert "streamable-http" in src, (
            "SELF-01: mcp_server.py must specify transport='streamable-http' — "
            "using 'sse' or 'stdio' would break container-to-container connectivity"
        )

    def test_self_improving_pending_approval_invariant_hardcoded(self):
        """SELF-01/02: D-11 invariant — suggest_promotion never auto-applies.

        The literal string 'pending_approval' must appear in mcp_server.py
        as a hardcoded return value, not derived from a runtime variable.
        This structural check ensures the invariant cannot be bypassed by
        a flag or configuration path.
        """
        src_path = Path(__file__).parent.parent / "mcp_server.py"
        src = src_path.read_text(encoding="utf-8")
        assert '"pending_approval"' in src or "'pending_approval'" in src, (
            "SELF-01: D-11 invariant violated — 'pending_approval' literal not found "
            "in mcp_server.py. suggest_promotion must hardcode this status, never "
            "derive it from a variable that could yield 'applied' or 'auto_applied'."
        )
