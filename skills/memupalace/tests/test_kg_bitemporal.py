"""Tests for KnowledgeGraph bitemporal features (D-15/MUPL-03, D-03)."""

from __future__ import annotations

import sqlite3
import tempfile
import os

import pytest

from skills.memupalace.knowledge_graph import KnowledgeGraph


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _tmp_kg() -> tuple[KnowledgeGraph, str]:
    """Create a KG in a temp dir; caller must close manually."""
    tmpdir = tempfile.mkdtemp()
    db_path = os.path.join(tmpdir, "kg_bitemporal.db")
    return KnowledgeGraph(db_path), db_path


# ---------------------------------------------------------------------------
# Schema tests
# ---------------------------------------------------------------------------


def test_schema_has_valid_from_column(tmp_sqlite_path: str) -> None:
    """entities table must have valid_from column after init."""
    kg = KnowledgeGraph(tmp_sqlite_path)
    kg.close()

    conn = sqlite3.connect(tmp_sqlite_path)
    cols = {row[1] for row in conn.execute("PRAGMA table_info(entities)").fetchall()}
    conn.close()

    assert "valid_from" in cols


def test_schema_has_valid_to_column(tmp_sqlite_path: str) -> None:
    """entities table must have valid_to column after init."""
    kg = KnowledgeGraph(tmp_sqlite_path)
    kg.close()

    conn = sqlite3.connect(tmp_sqlite_path)
    cols = {row[1] for row in conn.execute("PRAGMA table_info(entities)").fetchall()}
    conn.close()

    assert "valid_to" in cols


def test_relations_schema_has_bitemporal_columns(tmp_sqlite_path: str) -> None:
    """relations table must have valid_from and valid_to columns."""
    kg = KnowledgeGraph(tmp_sqlite_path)
    kg.close()

    conn = sqlite3.connect(tmp_sqlite_path)
    cols = {row[1] for row in conn.execute("PRAGMA table_info(relations)").fetchall()}
    conn.close()

    assert "valid_from" in cols
    assert "valid_to" in cols


# ---------------------------------------------------------------------------
# Entity validity tests
# ---------------------------------------------------------------------------


def test_new_entity_has_null_valid_to(tmp_sqlite_path: str) -> None:
    """Newly created entity must have valid_to = NULL (still active)."""
    kg = KnowledgeGraph(tmp_sqlite_path)
    entity_id = kg.upsert_entity("Alice", "person")
    kg.close()

    conn = sqlite3.connect(tmp_sqlite_path)
    row = conn.execute(
        "SELECT valid_to FROM entities WHERE id = ?", (entity_id,)
    ).fetchone()
    conn.close()

    assert row is not None
    assert row[0] is None  # valid_to must be NULL


def test_new_entity_has_valid_from_set(tmp_sqlite_path: str) -> None:
    """Newly created entity must have valid_from set (not NULL, not empty)."""
    kg = KnowledgeGraph(tmp_sqlite_path)
    entity_id = kg.upsert_entity("Bob", "person")
    kg.close()

    conn = sqlite3.connect(tmp_sqlite_path)
    row = conn.execute(
        "SELECT valid_from FROM entities WHERE id = ?", (entity_id,)
    ).fetchone()
    conn.close()

    assert row is not None
    assert row[0] is not None and row[0] != ""


def test_explicit_valid_from_stored(tmp_sqlite_path: str) -> None:
    """upsert_entity accepts explicit valid_from and stores it."""
    kg = KnowledgeGraph(tmp_sqlite_path)
    vfrom = "2026-01-01T00:00:00+00:00"
    entity_id = kg.upsert_entity("Charlie", "person", valid_from=vfrom)
    kg.close()

    conn = sqlite3.connect(tmp_sqlite_path)
    row = conn.execute(
        "SELECT valid_from FROM entities WHERE id = ?", (entity_id,)
    ).fetchone()
    conn.close()

    assert row[0] == vfrom


# ---------------------------------------------------------------------------
# invalidate() tests
# ---------------------------------------------------------------------------


def test_invalidate_sets_valid_to(tmp_sqlite_path: str) -> None:
    """invalidate(entity_id) must set valid_to to a non-NULL ISO timestamp."""
    kg = KnowledgeGraph(tmp_sqlite_path)
    entity_id = kg.upsert_entity("Alice", "person")
    kg.invalidate(entity_id)
    kg.close()

    conn = sqlite3.connect(tmp_sqlite_path)
    row = conn.execute(
        "SELECT valid_to FROM entities WHERE id = ?", (entity_id,)
    ).fetchone()
    conn.close()

    assert row is not None
    assert row[0] is not None  # valid_to must be set after invalidation


def test_invalidate_makes_entity_invisible_in_upsert(tmp_sqlite_path: str) -> None:
    """After invalidation, upsert_entity with same name creates a new entity."""
    kg = KnowledgeGraph(tmp_sqlite_path)
    id1 = kg.upsert_entity("Alice", "person")
    kg.invalidate(id1)
    id2 = kg.upsert_entity("Alice", "person")
    kg.close()

    # A new entity must be created since the old one is invalidated
    assert id1 != id2


def test_invalidate_cascades_to_relations(tmp_sqlite_path: str) -> None:
    """Invalidating an entity must also invalidate its active relations."""
    kg = KnowledgeGraph(tmp_sqlite_path)
    src = kg.upsert_entity("Alice", "person")
    tgt = kg.upsert_entity("Project", "project")
    kg.add_relation(src, tgt, "works_on", "mem-001")

    kg.invalidate(src)

    # get_relations filters valid_to IS NULL — must return empty
    relations = kg.get_relations(src)
    kg.close()

    assert len(relations) == 0


def test_invalidate_idempotent(tmp_sqlite_path: str) -> None:
    """Calling invalidate twice on the same entity must not raise."""
    kg = KnowledgeGraph(tmp_sqlite_path)
    entity_id = kg.upsert_entity("Alice", "person")
    kg.invalidate(entity_id)
    kg.invalidate(entity_id)  # should not raise or error
    kg.close()


# ---------------------------------------------------------------------------
# invalidate_by_memory() tests
# ---------------------------------------------------------------------------


def test_invalidate_by_memory_returns_entity_ids(tmp_sqlite_path: str) -> None:
    """invalidate_by_memory returns list of entity_ids invalidated."""
    kg = KnowledgeGraph(tmp_sqlite_path)
    src = kg.upsert_entity("Alice", "person")
    tgt = kg.upsert_entity("Project", "project")
    memory_id = "mem-abc"
    kg.add_relation(src, tgt, "works_on", memory_id)

    invalidated = kg.invalidate_by_memory(memory_id)
    kg.close()

    assert src in invalidated


def test_invalidate_by_memory_empty_for_unknown_memory(tmp_sqlite_path: str) -> None:
    """invalidate_by_memory returns empty list if memory_id has no relations."""
    kg = KnowledgeGraph(tmp_sqlite_path)
    kg.upsert_entity("Alice", "person")

    result = kg.invalidate_by_memory("nonexistent-memory")
    kg.close()

    assert result == []


def test_invalidate_by_memory_makes_entities_invisible(tmp_sqlite_path: str) -> None:
    """After invalidate_by_memory, entities linked to that memory are inactive."""
    kg = KnowledgeGraph(tmp_sqlite_path)
    src = kg.upsert_entity("ConceptA", "concept")
    tgt = kg.upsert_entity("ConceptB", "concept")
    memory_id = "mem-xyz"
    kg.add_relation(src, tgt, "related_to", memory_id)

    kg.invalidate_by_memory(memory_id)

    # get_entities filters active only — must be empty
    entities = kg.get_entities(memory_id)
    kg.close()

    assert len(entities) == 0


# ---------------------------------------------------------------------------
# Active-only filter tests
# ---------------------------------------------------------------------------


def test_get_entities_filters_invalid(tmp_sqlite_path: str) -> None:
    """get_entities must not return invalidated entities."""
    kg = KnowledgeGraph(tmp_sqlite_path)
    src = kg.upsert_entity("Alice", "person")
    tgt = kg.upsert_entity("Project", "project")
    memory_id = "mem-filter"
    kg.add_relation(src, tgt, "works_on", memory_id)

    # Invalidate the source entity
    kg.invalidate(src)
    entities = kg.get_entities(memory_id)
    kg.close()

    entity_ids = {e.id for e in entities}
    assert src not in entity_ids


def test_get_relations_filters_invalid(tmp_sqlite_path: str) -> None:
    """get_relations must not return invalidated relations."""
    kg = KnowledgeGraph(tmp_sqlite_path)
    src = kg.upsert_entity("Alice", "person")
    tgt = kg.upsert_entity("Project", "project")
    kg.add_relation(src, tgt, "works_on", "mem-001")

    # Invalidate src — cascades to relations
    kg.invalidate(src)
    relations = kg.get_relations(tgt)
    kg.close()

    assert len(relations) == 0
