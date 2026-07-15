"""Tests for memupalace fastmcp tool functions — unit tests (9.1) and property P17 (9.3).

Tests call fastmcp tool functions directly by injecting a pre-built Memupalace
instance via the module-level _mp global, avoiding real ONNX model loading.
ChromaDB is optional — tests are skipped if not installed.
"""

from __future__ import annotations

import math

import pytest

# Skip entire module if chromadb is not installed
pytest.importorskip("chromadb")

from hypothesis import given, settings
from hypothesis import strategies as st

import skills.memupalace.mcp_server as _srv
from skills.memupalace.factory import MemupalaceSettings, _create_memupalace_with_embedder
from skills.memupalace.mcp_server import (
    memory_add,
    memory_delete,
    memory_embed,
    memory_invalidate,
    memory_list_locations,
    memory_search,
)


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _inject_mp(tmp_chroma_path: str, tmp_sqlite_path: str, mock_embedder) -> None:
    """Wire mock Memupalace into the mcp_server singleton (_mp)."""
    cfg = MemupalaceSettings(
        chroma_path=tmp_chroma_path,
        sqlite_path=tmp_sqlite_path,
        onnx_model_path="models/embedder.onnx",  # not used — mock embedder
    )
    mp = _create_memupalace_with_embedder(cfg, mock_embedder)
    _srv._mp = mp


# ---------------------------------------------------------------------------
# 9.1 Unit tests
# ---------------------------------------------------------------------------


def test_mcp_server_exposes_6_tools(tmp_chroma_path, tmp_sqlite_path, mock_embedder):
    """fastmcp server must expose exactly 6 tools."""
    import asyncio

    _inject_mp(tmp_chroma_path, tmp_sqlite_path, mock_embedder)
    tools = asyncio.run(_srv.mcp.list_tools())
    tool_names = {t.name for t in tools}
    assert tool_names == {
        "memory_add",
        "memory_search",
        "memory_list_locations",
        "memory_delete",
        "memory_embed",
        "memory_invalidate",
    }


def test_memory_add_valid(tmp_chroma_path, tmp_sqlite_path, mock_embedder):
    """memory_add with valid params returns a dict with 'id' and 'operation'."""
    _inject_mp(tmp_chroma_path, tmp_sqlite_path, mock_embedder)
    result = memory_add(content="Hello world", wing="test")
    assert "id" in result
    assert "operation" in result
    assert result["operation"] in ("created", "reinforced")
    assert isinstance(result["id"], str) and result["id"]


def test_memory_add_empty_content_raises(tmp_chroma_path, tmp_sqlite_path, mock_embedder):
    """memory_add with empty content must raise ValueError."""
    _inject_mp(tmp_chroma_path, tmp_sqlite_path, mock_embedder)
    with pytest.raises(ValueError, match="non-empty"):
        memory_add(content="", wing="test")


def test_memory_embed_empty_text_raises(tmp_chroma_path, tmp_sqlite_path, mock_embedder):
    """memory_embed with empty string must raise ValueError."""
    _inject_mp(tmp_chroma_path, tmp_sqlite_path, mock_embedder)
    with pytest.raises(ValueError, match="non-empty"):
        memory_embed(text="")


def test_memory_embed_whitespace_raises(tmp_chroma_path, tmp_sqlite_path, mock_embedder):
    """memory_embed with whitespace-only string must raise ValueError."""
    _inject_mp(tmp_chroma_path, tmp_sqlite_path, mock_embedder)
    with pytest.raises(ValueError, match="non-empty"):
        memory_embed(text="   ")


def test_memory_delete_nonexistent_raises(tmp_chroma_path, tmp_sqlite_path, mock_embedder):
    """memory_delete with a fake ID must raise KeyError."""
    _inject_mp(tmp_chroma_path, tmp_sqlite_path, mock_embedder)
    with pytest.raises(KeyError):
        memory_delete(memory_id="00000000-0000-0000-0000-000000000000")


def test_memory_search_returns_list(tmp_chroma_path, tmp_sqlite_path, mock_embedder):
    """After adding a memory, searching for it returns a list."""
    _inject_mp(tmp_chroma_path, tmp_sqlite_path, mock_embedder)
    memory_add(content="Python is great for data science", wing="tech")
    results = memory_search(query="Python data science", wing="tech")
    assert isinstance(results, list)
    assert len(results) >= 1
    first = results[0]
    assert "id" in first
    assert "content" in first
    assert "score" in first


def test_memory_search_applies_sanitizer(tmp_chroma_path, tmp_sqlite_path, mock_embedder):
    """memory_search must pass through query_sanitizer (D-14)."""
    _inject_mp(tmp_chroma_path, tmp_sqlite_path, mock_embedder)
    # Short clean query — passthrough expected, must not raise
    results = memory_search(query="simple query", wing=None)
    assert isinstance(results, list)


def test_memory_embed_returns_vector(tmp_chroma_path, tmp_sqlite_path, mock_embedder):
    """memory_embed with valid text returns a list of floats."""
    _inject_mp(tmp_chroma_path, tmp_sqlite_path, mock_embedder)
    vec = memory_embed(text="hello")
    assert isinstance(vec, list)
    assert len(vec) == 384
    assert all(isinstance(v, float) for v in vec)


def test_memory_list_locations_returns_dict(tmp_chroma_path, tmp_sqlite_path, mock_embedder):
    """memory_list_locations returns a dict with 'wings' key."""
    _inject_mp(tmp_chroma_path, tmp_sqlite_path, mock_embedder)
    memory_add(content="Some fact", wing="personal")
    result = memory_list_locations()
    assert isinstance(result, dict)
    assert "wings" in result
    assert "personal" in result["wings"]


def test_memory_delete_valid(tmp_chroma_path, tmp_sqlite_path, mock_embedder):
    """memory_delete with a valid ID returns confirmation dict."""
    _inject_mp(tmp_chroma_path, tmp_sqlite_path, mock_embedder)
    add_result = memory_add(content="To be deleted", wing="temp")
    memory_id = add_result["id"]
    result = memory_delete(memory_id=memory_id)
    assert result == {"deleted": memory_id}


def test_memory_invalidate_unknown_belief(tmp_chroma_path, tmp_sqlite_path, mock_embedder):
    """memory_invalidate with unknown rust_belief_id returns None chroma_id."""
    _inject_mp(tmp_chroma_path, tmp_sqlite_path, mock_embedder)
    result = memory_invalidate(rust_belief_id="nonexistent-belief-id")
    assert result["rust_belief_id"] == "nonexistent-belief-id"
    assert result["invalidated_chroma_id"] is None


def test_memory_invalidate_empty_raises(tmp_chroma_path, tmp_sqlite_path, mock_embedder):
    """memory_invalidate with empty string must raise ValueError."""
    _inject_mp(tmp_chroma_path, tmp_sqlite_path, mock_embedder)
    with pytest.raises(ValueError, match="non-empty"):
        memory_invalidate(rust_belief_id="")


# ---------------------------------------------------------------------------
# 9.3 Property 17: Embedding Service Non-Zero Output
# Validates: Requirements 3.5, 13.1
# ---------------------------------------------------------------------------


def _make_stub_embedder():
    """Build a standalone stub embedder (no pytest fixture) for property tests."""
    import math as _math
    from unittest.mock import MagicMock

    embedder = MagicMock()

    def _embed(text: str) -> list[float]:
        seed = hash(text) % (2**31)
        vec = [_math.sin(seed + i) * 0.1 + 0.01 for i in range(384)]
        norm = _math.sqrt(sum(x * x for x in vec))
        return [x / norm for x in vec]

    embedder.embed.side_effect = _embed
    embedder.embed_batch.side_effect = lambda texts: [_embed(t) for t in texts]
    return embedder


@given(text=st.text(min_size=1).filter(lambda s: s.strip()))
@settings(max_examples=100, deadline=None)
def test_property_17_memory_embed_nonzero_output(text: str) -> None:
    """Property 17: For any non-empty text, memory_embed returns a vector with L2 norm > 0.

    Validates: Requirements 3.5, 13.1
    """
    import os
    import tempfile

    stub = _make_stub_embedder()

    with tempfile.TemporaryDirectory() as tmpdir:
        chroma_dir = os.path.join(tmpdir, "chroma")
        sqlite_path = os.path.join(tmpdir, "knowledge.db")

        cfg = MemupalaceSettings(
            chroma_path=chroma_dir,
            sqlite_path=sqlite_path,
            onnx_model_path="models/embedder.onnx",
        )
        mp = _create_memupalace_with_embedder(cfg, stub)
        _srv._mp = mp

        vec = memory_embed(text=text)

    assert isinstance(vec, list), "Result must be a list"
    assert len(vec) > 0, "Embedding must be non-empty"

    norm = math.sqrt(sum(v * v for v in vec))
    assert norm > 0.0, f"L2 norm must be > 0 for text={text!r}, got norm={norm}"
