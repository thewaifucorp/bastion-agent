"""Unit tests for the Memupalace factory and facade."""

from __future__ import annotations

import pytest

from skills.memupalace.factory import _create_memupalace_with_embedder
from skills.memupalace.models import MemupalaceSettings


def _make_memupalace(mock_embedder, tmp_chroma_path, tmp_sqlite_path):
    """Helper: build a Memupalace with mock embedder and temp paths."""
    settings = MemupalaceSettings(
        chroma_path=tmp_chroma_path,
        sqlite_path=tmp_sqlite_path,
        onnx_model_path="models/embedder.onnx",  # not used with mock
    )
    return _create_memupalace_with_embedder(settings, mock_embedder)


# ---------------------------------------------------------------------------
# test_add_creates_memory
# ---------------------------------------------------------------------------


def test_add_creates_memory(mock_embedder, tmp_chroma_path, tmp_sqlite_path):
    mp = _make_memupalace(mock_embedder, tmp_chroma_path, tmp_sqlite_path)
    result = mp.add("Python is a great language", wing="tech")
    assert result.operation == "created"
    assert result.id  # non-empty UUID


# ---------------------------------------------------------------------------
# test_add_duplicate_reinforces
# ---------------------------------------------------------------------------


def test_add_duplicate_reinforces(mock_embedder, tmp_chroma_path, tmp_sqlite_path):
    """Adding the same content twice should reinforce on the second call."""
    settings = MemupalaceSettings(
        chroma_path=tmp_chroma_path,
        sqlite_path=tmp_sqlite_path,
        duplicate_threshold=0.5,  # low threshold so mock embedder triggers duplicate
    )
    mp = _create_memupalace_with_embedder(settings, mock_embedder)

    first = mp.add("I love coffee in the morning", wing="personal")
    second = mp.add("I love coffee in the morning", wing="personal")

    assert first.operation == "created"
    assert second.operation == "reinforced"
    assert second.id == first.id


# ---------------------------------------------------------------------------
# test_search_returns_results
# ---------------------------------------------------------------------------


def test_search_returns_results(mock_embedder, tmp_chroma_path, tmp_sqlite_path):
    mp = _make_memupalace(mock_embedder, tmp_chroma_path, tmp_sqlite_path)
    mp.add("The sky is blue", wing="facts")
    results = mp.search("sky color", wing="facts")
    assert len(results) >= 1
    assert any("sky" in r.content.lower() for r in results)


# ---------------------------------------------------------------------------
# test_delete_removes_memory
# ---------------------------------------------------------------------------


def test_delete_removes_memory(mock_embedder, tmp_chroma_path, tmp_sqlite_path):
    mp = _make_memupalace(mock_embedder, tmp_chroma_path, tmp_sqlite_path)
    result = mp.add("Temporary memory to delete", wing="temp")
    memory_id = result.id

    mp.delete(memory_id)

    # After deletion, searching should not return the deleted memory
    results = mp.search("temporary memory", wing="temp")
    assert all(r.id != memory_id for r in results)

    # Direct get should raise KeyError
    with pytest.raises(KeyError):
        mp._store.get(memory_id)


# ---------------------------------------------------------------------------
# test_search_respects_min_score
# ---------------------------------------------------------------------------


def test_search_respects_min_score(mock_embedder, tmp_chroma_path, tmp_sqlite_path):
    """min_score=1.1 is impossible — should return empty results."""
    mp = _make_memupalace(mock_embedder, tmp_chroma_path, tmp_sqlite_path)
    mp.add("Some memory content", wing="test")
    results = mp.search("some memory", wing="test", min_score=1.1)
    assert results == []


# ---------------------------------------------------------------------------
# test_add_validates_empty_content
# ---------------------------------------------------------------------------


def test_add_validates_empty_content(mock_embedder, tmp_chroma_path, tmp_sqlite_path):
    mp = _make_memupalace(mock_embedder, tmp_chroma_path, tmp_sqlite_path)
    with pytest.raises(ValueError, match="empty"):
        mp.add("   ", wing="test")


# ---------------------------------------------------------------------------
# test_add_validates_invalid_slug
# ---------------------------------------------------------------------------


def test_add_validates_invalid_slug(mock_embedder, tmp_chroma_path, tmp_sqlite_path):
    mp = _make_memupalace(mock_embedder, tmp_chroma_path, tmp_sqlite_path)
    with pytest.raises(ValueError, match="invalid characters"):
        mp.add("valid content", wing="invalid wing!")


# ---------------------------------------------------------------------------
# test_list_locations
# ---------------------------------------------------------------------------


def test_list_locations(mock_embedder, tmp_chroma_path, tmp_sqlite_path):
    mp = _make_memupalace(mock_embedder, tmp_chroma_path, tmp_sqlite_path)
    mp.add("Memory in alpha", wing="work", hall="projects")
    mp.add("Memory in beta", wing="work", hall="meetings")

    locations = mp.list_locations(wing="work")
    assert "projects" in locations
    assert "meetings" in locations
