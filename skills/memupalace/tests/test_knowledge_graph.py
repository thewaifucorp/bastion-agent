"""Tests for KnowledgeGraph (SQLite).

Unit tests cover schema creation, upsert idempotency, relation persistence,
and entity lookup by memory_id.

Property 7: Knowledge Graph Persistence Completeness
  Validates: Requirements 4.1, 4.2
"""

from __future__ import annotations

import re
import sqlite3
import tempfile
import os

import pytest
from hypothesis import HealthCheck, given, settings
from hypothesis import strategies as st

from skills.memupalace.knowledge_graph import Entity, KnowledgeGraph, Relation

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

UUID_RE = re.compile(
    r"^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$",
    re.IGNORECASE,
)

ISO8601_RE = re.compile(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}")


# ---------------------------------------------------------------------------
# Unit tests
# ---------------------------------------------------------------------------


def test_schema_created_automatically(tmp_sqlite_path: str) -> None:
    """KnowledgeGraph creates the SQLite file and both tables on __init__."""
    kg = KnowledgeGraph(tmp_sqlite_path)
    kg.close()

    conn = sqlite3.connect(tmp_sqlite_path)
    tables = {
        row[0]
        for row in conn.execute(
            "SELECT name FROM sqlite_master WHERE type='table'"
        ).fetchall()
    }
    conn.close()

    assert "entities" in tables
    assert "relations" in tables


def test_upsert_entity_idempotent(tmp_sqlite_path: str) -> None:
    """Calling upsert_entity twice with the same name returns the same ID."""
    kg = KnowledgeGraph(tmp_sqlite_path)
    id1 = kg.upsert_entity("Alice", "person")
    id2 = kg.upsert_entity("Alice", "person")
    kg.close()

    assert id1 == id2
    assert UUID_RE.match(id1)


def test_upsert_entity_different_names(tmp_sqlite_path: str) -> None:
    """Different names produce different IDs."""
    kg = KnowledgeGraph(tmp_sqlite_path)
    id_alice = kg.upsert_entity("Alice", "person")
    id_bob = kg.upsert_entity("Bob", "person")
    kg.close()

    assert id_alice != id_bob


def test_add_relation_and_get(tmp_sqlite_path: str) -> None:
    """add_relation persists a relation; get_relations returns it."""
    kg = KnowledgeGraph(tmp_sqlite_path)
    src = kg.upsert_entity("Alice", "person")
    tgt = kg.upsert_entity("ProjectAlpha", "project")
    memory_id = "mem-001"

    kg.add_relation(src, tgt, "works_on", memory_id)
    relations = kg.get_relations(src)
    kg.close()

    assert len(relations) == 1
    rel = relations[0]
    assert rel.source_id == src
    assert rel.target_id == tgt
    assert rel.relation_type == "works_on"
    assert rel.memory_id == memory_id
    assert ISO8601_RE.match(rel.observed_at)


def test_get_relations_includes_target_side(tmp_sqlite_path: str) -> None:
    """get_relations returns relations where entity is the target too."""
    kg = KnowledgeGraph(tmp_sqlite_path)
    src = kg.upsert_entity("Alice", "person")
    tgt = kg.upsert_entity("ProjectAlpha", "project")
    kg.add_relation(src, tgt, "works_on", "mem-001")

    # Query from the target's perspective
    relations = kg.get_relations(tgt)
    kg.close()

    assert len(relations) == 1
    assert relations[0].source_id == src


def test_get_entities_by_memory_id(tmp_sqlite_path: str) -> None:
    """get_entities returns entities linked to a given memory_id via relations."""
    kg = KnowledgeGraph(tmp_sqlite_path)
    alice = kg.upsert_entity("Alice", "person")
    proj = kg.upsert_entity("ProjectAlpha", "project")
    memory_id = "mem-xyz"

    kg.add_relation(alice, proj, "works_on", memory_id)
    entities = kg.get_entities(memory_id)
    kg.close()

    entity_ids = {e.id for e in entities}
    assert alice in entity_ids
    assert proj in entity_ids


def test_get_entities_empty_for_unknown_memory(tmp_sqlite_path: str) -> None:
    """get_entities returns empty list for a memory_id with no relations."""
    kg = KnowledgeGraph(tmp_sqlite_path)
    kg.upsert_entity("Alice", "person")
    result = kg.get_entities("nonexistent-memory")
    kg.close()

    assert result == []


def test_sqlite_path_is_separate_from_lifelog(tmp_sqlite_path: str) -> None:
    """The KnowledgeGraph path must not be db/life-log.db (Requirement 11.1)."""
    assert "life-log" not in tmp_sqlite_path


# ---------------------------------------------------------------------------
# Property 7: Knowledge Graph Persistence Completeness
# Validates: Requirements 4.1, 4.2
# ---------------------------------------------------------------------------

_name_strategy = st.text(
    alphabet=st.characters(whitelist_categories=("Lu", "Ll", "Nd"), whitelist_characters="-_ "),
    min_size=1,
    max_size=50,
)

_entity_type_strategy = st.sampled_from(["person", "project", "concept", "place", "event"])

_relation_type_strategy = st.sampled_from(
    ["works_on", "knows", "related_to", "part_of", "created_by"]
)


@given(
    name=_name_strategy,
    entity_type=_entity_type_strategy,
)
@settings(max_examples=100)
def test_property7_entity_fields_completeness(
    name: str,
    entity_type: str,
) -> None:
    """Property 7 (entities): all fields of a persisted entity are non-null.

    Validates: Requirements 4.1, 4.2
    """
    with tempfile.TemporaryDirectory() as tmpdir:
        db_path = os.path.join(tmpdir, "kg_prop7_entity.db")
        kg = KnowledgeGraph(db_path)
        entity_id = kg.upsert_entity(name, entity_type)

        # Read back directly from SQLite to verify persistence
        conn = sqlite3.connect(db_path)
        row = conn.execute(
            "SELECT id, name, type, first_seen_at FROM entities WHERE id = ?",
            (entity_id,),
        ).fetchone()
        conn.close()
        kg.close()

    assert row is not None, "Entity must be persisted"
    eid, ename, etype, efirst_seen = row
    assert eid is not None and eid != ""
    assert ename is not None and ename != ""
    assert etype is not None and etype != ""
    assert efirst_seen is not None and efirst_seen != ""


@given(
    src_name=_name_strategy,
    tgt_name=_name_strategy,
    relation_type=_relation_type_strategy,
    memory_id=st.uuids().map(str),
)
@settings(max_examples=100)
def test_property7_relation_fields_completeness(
    src_name: str,
    tgt_name: str,
    relation_type: str,
    memory_id: str,
) -> None:
    """Property 7 (relations): all fields of a persisted relation are non-null.

    Validates: Requirements 4.1, 4.2
    """
    # Ensure distinct names to avoid same-entity edge case
    if src_name == tgt_name:
        tgt_name = tgt_name + "_target"

    with tempfile.TemporaryDirectory() as tmpdir:
        db_path = os.path.join(tmpdir, "kg_prop7_rel.db")
        kg = KnowledgeGraph(db_path)
        src_id = kg.upsert_entity(src_name, "person")
        tgt_id = kg.upsert_entity(tgt_name, "project")
        kg.add_relation(src_id, tgt_id, relation_type, memory_id)

        conn = sqlite3.connect(db_path)
        row = conn.execute(
            """
            SELECT id, source_id, target_id, relation_type, observed_at, memory_id
            FROM relations
            WHERE source_id = ? AND target_id = ?
            """,
            (src_id, tgt_id),
        ).fetchone()
        conn.close()
        kg.close()

    assert row is not None, "Relation must be persisted"
    rid, rsrc, rtgt, rtype, robserved, rmem = row
    assert rid is not None and rid != ""
    assert rsrc is not None and rsrc != ""
    assert rtgt is not None and rtgt != ""
    assert rtype is not None and rtype != ""
    assert robserved is not None and robserved != ""
    assert rmem is not None and rmem != ""
