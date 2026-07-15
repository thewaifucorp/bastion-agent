"""Tests for MemoryStore (ChromaDB).

Unit tests (5.1) and property-based tests (5.2, 5.7).
ChromaDB is optional — tests are skipped if not installed.
"""

from __future__ import annotations

import pytest

chromadb = pytest.importorskip("chromadb")

from hypothesis import HealthCheck, given, settings
from hypothesis import strategies as st

from skills.memupalace.store import MemoryStore


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _store(tmp_chroma_path: str) -> MemoryStore:
    return MemoryStore(chroma_path=tmp_chroma_path)


# ---------------------------------------------------------------------------
# 5.1 Unit tests
# ---------------------------------------------------------------------------


def test_add_and_get_round_trip(tmp_chroma_path: str, mock_embedder) -> None:
    """Add a memory, get it by ID, verify content matches."""
    store = _store(tmp_chroma_path)
    content = "The capital of France is Paris."
    emb = mock_embedder.embed(content)
    mid = store.add(content, emb, wing="geo", hall=None, room=None)

    mem = store.get(mid)
    assert mem.id == mid
    assert mem.content == content
    assert mem.wing == "geo"
    assert mem.hall is None
    assert mem.room is None
    assert mem.reinforcement_count == 0


def test_delete_removes_memory(tmp_chroma_path: str, mock_embedder) -> None:
    """Add then delete; subsequent get raises KeyError."""
    store = _store(tmp_chroma_path)
    emb = mock_embedder.embed("hello world")
    mid = store.add("hello world", emb, wing="test", hall=None, room=None)

    store.delete(mid)
    with pytest.raises(KeyError):
        store.get(mid)


def test_delete_nonexistent_raises_key_error(tmp_chroma_path: str) -> None:
    """Deleting a fake ID raises KeyError."""
    store = _store(tmp_chroma_path)
    with pytest.raises(KeyError):
        store.delete("00000000-0000-0000-0000-000000000000")


def test_location_filter(tmp_chroma_path: str, mock_embedder) -> None:
    """Memories in different wings are isolated by vector_search wing filter."""
    store = _store(tmp_chroma_path)

    content_work = "Work meeting notes"
    content_personal = "Personal diary entry"
    emb_work = mock_embedder.embed(content_work)
    emb_personal = mock_embedder.embed(content_personal)

    store.add(content_work, emb_work, wing="work", hall=None, room=None)
    store.add(content_personal, emb_personal, wing="personal", hall=None, room=None)

    # Search in "work" wing using the work embedding
    results = store.vector_search(emb_work, wing="work", hall=None, room=None, n_results=10)
    wings_found = {mem.wing for mem, _ in results}
    assert wings_found == {"work"}, f"Expected only 'work', got {wings_found}"

    # Search in "personal" wing
    results_p = store.vector_search(emb_personal, wing="personal", hall=None, room=None, n_results=10)
    wings_found_p = {mem.wing for mem, _ in results_p}
    assert wings_found_p == {"personal"}, f"Expected only 'personal', got {wings_found_p}"


# ---------------------------------------------------------------------------
# 5.2 Property 1: Verbatim Storage Round-Trip
# Validates: Requirements 1.1
# ---------------------------------------------------------------------------


@given(content=st.text(min_size=1).filter(lambda s: s.strip()))
@settings(max_examples=20, suppress_health_check=[HealthCheck.function_scoped_fixture], deadline=None)
def test_verbatim_storage_round_trip(tmp_path, mock_embedder, content: str) -> None:
    """
    Property 1: Verbatim Storage Round-Trip
    Validates: Requirements 1.1

    For any non-empty text, add it, get it back — content must be identical.
    """
    import tempfile
    with tempfile.TemporaryDirectory() as d:
        store = MemoryStore(chroma_path=d + "/chroma")
        emb = mock_embedder.embed(content)
        mid = store.add(content, emb, wing="test", hall=None, room=None)
        mem = store.get(mid)
        assert mem.content == content


# ---------------------------------------------------------------------------
# 5.7 Property 8: Duplicate Check Idempotence
# Validates: Requirements 5.2, 5.3
# ---------------------------------------------------------------------------


@given(n=st.integers(min_value=1, max_value=5))
@settings(max_examples=20, suppress_health_check=[HealthCheck.function_scoped_fixture], deadline=None)
def test_duplicate_check_idempotence(tmp_path, mock_embedder, n: int) -> None:
    """
    Property 8: Duplicate Check Idempotence
    Validates: Requirements 5.2, 5.3

    Adding the same content N times results in exactly 1 entry with
    reinforcement_count = N - 1.
    """
    import tempfile
    with tempfile.TemporaryDirectory() as d:
        store = MemoryStore(chroma_path=d + "/chroma")
        content = "Idempotent memory content"
        emb = mock_embedder.embed(content)
        threshold = 0.95

        first_id: str | None = None
        for i in range(n):
            duplicate = store.check_duplicate(emb, wing="test", threshold=threshold)
            if duplicate is None:
                mid = store.add(content, emb, wing="test", hall=None, room=None)
                if first_id is None:
                    first_id = mid
            else:
                store.reinforce(duplicate.id)
                if first_id is None:
                    first_id = duplicate.id

        assert first_id is not None
        final = store.get(first_id)
        assert final.reinforcement_count == n - 1
