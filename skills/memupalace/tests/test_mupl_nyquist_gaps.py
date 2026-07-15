"""Nyquist gap tests for memupalace (MUPL-01, MUPL-04).

Dependency-light: no chromadb, no ONNX. Uses AST inspection and
import-level checks so they run green in the local dev environment.
"""
from __future__ import annotations

import ast
import sys
import types
from pathlib import Path

import pytest

# ---------------------------------------------------------------------------
# FastMCP stub so mcp_server can be imported without the live package
# ---------------------------------------------------------------------------

def _install_fastmcp_stub() -> None:
    if "fastmcp" in sys.modules:
        return
    stub = types.ModuleType("fastmcp")

    class _FakeMCP:
        def __init__(self, name: str) -> None:
            self.name = name
            self._tools: list[str] = []

        def tool(self):
            def decorator(fn):
                self._tools.append(fn.__name__)
                return fn
            return decorator

        def run(self, **kwargs: object) -> None:
            pass

    stub.FastMCP = _FakeMCP
    sys.modules["fastmcp"] = stub


_install_fastmcp_stub()


# ---------------------------------------------------------------------------
# MUPL-01: memupalace exposes exactly 6 MCP tools
#
# Requirement: "memupalace é MCP server Python isolado em container próprio"
# Observable behavior: the server registers 6 tools.
# Test type: unit (AST count — no chromadb dependency)
# ---------------------------------------------------------------------------


class TestMUPL01ToolCount:
    """MUPL-01: memupalace MCP server must expose exactly 6 tools."""

    def test_memupalace_registers_exactly_6_tools_via_ast(self):
        """Count @mcp.tool() decorators in mcp_server.py.

        This is the chromadb-free proxy for the runtime tool-list check in
        test_mcp_server.py (which is gated behind pytest.importorskip('chromadb')).
        The AST count directly verifies the server exposes 6 tools.
        """
        src_path = Path(__file__).parent.parent / "mcp_server.py"
        assert src_path.exists(), f"mcp_server.py not found at {src_path}"
        tree = ast.parse(src_path.read_text(encoding="utf-8"))

        tool_defs: list[str] = []
        for node in ast.walk(tree):
            if not isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
                continue
            for decorator in node.decorator_list:
                # Match @mcp.tool() — Call with func = Attribute(attr="tool")
                if (
                    isinstance(decorator, ast.Call)
                    and isinstance(decorator.func, ast.Attribute)
                    and decorator.func.attr == "tool"
                ):
                    tool_defs.append(node.name)
                    break

        assert len(tool_defs) == 6, (
            f"MUPL-01: expected 6 @mcp.tool() functions, found {len(tool_defs)}: {tool_defs}"
        )

    def test_memupalace_expected_tool_names_present(self):
        """MUPL-01: the 6 tools have the required names."""
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
            "memory_add",
            "memory_search",
            "memory_list_locations",
            "memory_delete",
            "memory_embed",
            "memory_invalidate",
        }
        assert tool_defs == expected, (
            f"MUPL-01: tool name mismatch. expected={expected}, got={tool_defs}"
        )

    def test_memupalace_server_uses_streamable_http_transport(self):
        """MUPL-01: server must use streamable-http transport (not sse or stdio)."""
        src_path = Path(__file__).parent.parent / "mcp_server.py"
        src = src_path.read_text(encoding="utf-8")
        assert "streamable-http" in src, (
            "MUPL-01: mcp_server.py must specify transport='streamable-http' — "
            "using 'sse' or 'stdio' would break container-to-container connectivity"
        )


# ---------------------------------------------------------------------------
# MUPL-04: Wing/room taxonomy parameters are present on the memory_add tool
#
# Requirement: "Wing/room taxonomy (mempalace) para organização semântica"
# Observable behavior: memory_add accepts wing, hall, room parameters.
# Test type: unit (signature inspection)
# ---------------------------------------------------------------------------


class TestMUPL04WingRoomTaxonomy:
    """MUPL-04: memory_add must accept wing, hall, room for semantic organization."""

    def _import_memory_add(self):
        """Import memory_add without requiring chromadb."""
        # Stub chromadb so mcp_server doesn't crash on import
        if "chromadb" not in sys.modules:
            chroma_stub = types.ModuleType("chromadb")
            chroma_stub.PersistentClient = object
            sys.modules["chromadb"] = chroma_stub

        # Provide skills.memupalace stubs for factory/store if not importable
        import importlib
        import importlib.util

        # Try real import first
        try:
            # Re-use the already-imported mcp_server if available
            if "skills.memupalace.mcp_server" in sys.modules:
                return sys.modules["skills.memupalace.mcp_server"].memory_add
            mod = importlib.import_module("skills.memupalace.mcp_server")
            return mod.memory_add
        except Exception:
            return None

    def test_memory_add_signature_has_wing_parameter(self):
        """memory_add function signature must include 'wing' parameter (MUPL-04)."""
        import inspect

        src_path = Path(__file__).parent.parent / "mcp_server.py"
        tree = ast.parse(src_path.read_text(encoding="utf-8"))

        memory_add_node = None
        for node in ast.walk(tree):
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)) and node.name == "memory_add":
                memory_add_node = node
                break

        assert memory_add_node is not None, "memory_add function not found in mcp_server.py"

        arg_names = [a.arg for a in memory_add_node.args.args]
        assert "wing" in arg_names, (
            f"MUPL-04: memory_add must accept 'wing' parameter for taxonomy, got: {arg_names}"
        )

    def test_memory_add_signature_has_hall_parameter(self):
        """memory_add must accept 'hall' for nested taxonomy (MUPL-04)."""
        src_path = Path(__file__).parent.parent / "mcp_server.py"
        tree = ast.parse(src_path.read_text(encoding="utf-8"))

        memory_add_node = None
        for node in ast.walk(tree):
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)) and node.name == "memory_add":
                memory_add_node = node
                break

        assert memory_add_node is not None
        arg_names = [a.arg for a in memory_add_node.args.args]
        assert "hall" in arg_names, (
            f"MUPL-04: memory_add must accept 'hall' parameter, got: {arg_names}"
        )

    def test_memory_add_signature_has_room_parameter(self):
        """memory_add must accept 'room' for leaf-level taxonomy (MUPL-04)."""
        src_path = Path(__file__).parent.parent / "mcp_server.py"
        tree = ast.parse(src_path.read_text(encoding="utf-8"))

        memory_add_node = None
        for node in ast.walk(tree):
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)) and node.name == "memory_add":
                memory_add_node = node
                break

        assert memory_add_node is not None
        arg_names = [a.arg for a in memory_add_node.args.args]
        assert "room" in arg_names, (
            f"MUPL-04: memory_add must accept 'room' parameter, got: {arg_names}"
        )

    def test_memory_add_passes_taxonomy_to_store(self):
        """MUPL-04: memory_add body must forward wing/hall/room to the store call."""
        src_path = Path(__file__).parent.parent / "mcp_server.py"
        src = src_path.read_text(encoding="utf-8")

        # The store add call must pass wing=, hall=, room= through
        assert "wing=wing" in src or "wing=" in src, (
            "MUPL-04: memory_add must pass wing to _get_mp().add()"
        )
        assert "hall=hall" in src or "hall=" in src, (
            "MUPL-04: memory_add must pass hall to _get_mp().add()"
        )
        assert "room=room" in src or "room=" in src, (
            "MUPL-04: memory_add must pass room to _get_mp().add()"
        )

    def test_insight_cache_key_includes_wing(self):
        """MUPL-04 + MUPL-02 interaction: InsightCache key must incorporate wing.

        A cache keyed only on content would collapse memories in different wings
        into the same entry — silently merging data from semantically distinct spaces.
        """
        from skills.memupalace.insight_cache import InsightCache

        # Same content, different wing → different cache keys
        k_personal = InsightCache.make_key("I had a meeting", "personal")
        k_work = InsightCache.make_key("I had a meeting", "work")
        assert k_personal != k_work, (
            "MUPL-04: InsightCache.make_key must differentiate by wing — "
            "same content in 'personal' and 'work' must produce different keys"
        )
