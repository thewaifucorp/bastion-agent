"""Knowledge Graph backed by SQLite for the memupalace skill."""

from __future__ import annotations

import sqlite3
import uuid
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path


@dataclass
class Entity:
    id: str
    name: str
    type: str
    first_seen_at: str  # ISO 8601
    valid_from: str  # ISO 8601 — when this version became valid
    valid_to: str | None  # ISO 8601 — NULL means still valid


@dataclass
class Relation:
    id: str
    source_id: str
    target_id: str
    relation_type: str
    observed_at: str  # ISO 8601
    memory_id: str
    valid_from: str  # ISO 8601
    valid_to: str | None  # NULL = still valid


_SCHEMA_SQL_V2 = """
CREATE TABLE IF NOT EXISTS entities (
    id            TEXT PRIMARY KEY,
    name          TEXT NOT NULL,
    type          TEXT NOT NULL,
    first_seen_at TEXT NOT NULL,
    valid_from    TEXT NOT NULL,    -- ISO 8601, when this version became valid
    valid_to      TEXT              -- ISO 8601, NULL = still valid
);

CREATE TABLE IF NOT EXISTS relations (
    id            TEXT PRIMARY KEY,
    source_id     TEXT NOT NULL REFERENCES entities(id),
    target_id     TEXT NOT NULL REFERENCES entities(id),
    relation_type TEXT NOT NULL,
    observed_at   TEXT NOT NULL,
    memory_id     TEXT NOT NULL,
    valid_from    TEXT NOT NULL,    -- ISO 8601
    valid_to      TEXT              -- NULL = still valid
);

CREATE INDEX IF NOT EXISTS idx_entities_name    ON entities(name);
CREATE INDEX IF NOT EXISTS idx_entities_valid   ON entities(valid_to);
CREATE INDEX IF NOT EXISTS idx_relations_source ON relations(source_id);
CREATE INDEX IF NOT EXISTS idx_relations_target ON relations(target_id);
CREATE INDEX IF NOT EXISTS idx_relations_memory ON relations(memory_id);
CREATE INDEX IF NOT EXISTS idx_relations_valid  ON relations(valid_to);
"""


def _now_iso() -> str:
    return datetime.now(tz=timezone.utc).isoformat()


class KnowledgeGraph:
    """Persistent knowledge graph stored in a dedicated SQLite file.

    The file is created automatically on first use, along with the full schema.
    Supports bitemporal model: valid_from / valid_to columns on entities and
    relations (D-15/MUPL-03). All reads filter valid_to IS NULL by default.

    This database is intentionally separate from ``db/life-log.db`` (Requirement 11.1).
    """

    def __init__(self, sqlite_path: str) -> None:
        path = Path(sqlite_path)
        path.parent.mkdir(parents=True, exist_ok=True)
        self._conn = sqlite3.connect(str(path), check_same_thread=False)
        self._conn.execute("PRAGMA foreign_keys = ON")
        self._apply_schema()
        self._conn.commit()

    def _apply_schema(self) -> None:
        """Apply V2 schema; migrate pre-existing dbs missing bitemporal columns.

        Checks for existing tables BEFORE running executescript so we can detect
        old (V1) databases that need the valid_from/valid_to migration. Fresh
        databases skip the PRAGMA check entirely for performance.
        """
        # Check if entities table exists before applying V2 schema
        existing_tables = {
            row[0]
            for row in self._conn.execute(
                "SELECT name FROM sqlite_master WHERE type='table'"
            ).fetchall()
        }
        needs_migration = "entities" in existing_tables  # only old dbs need migration

        self._conn.executescript(_SCHEMA_SQL_V2)

        if needs_migration:
            # Check for missing valid_from column (old V1 schema)
            cols = {
                row[1]
                for row in self._conn.execute("PRAGMA table_info(entities)").fetchall()
            }
            if "valid_from" not in cols:
                migration_stmts = [
                    "ALTER TABLE entities ADD COLUMN valid_from TEXT NOT NULL DEFAULT '1970-01-01T00:00:00+00:00'",
                    "ALTER TABLE entities ADD COLUMN valid_to TEXT",
                    "ALTER TABLE relations ADD COLUMN valid_from TEXT NOT NULL DEFAULT '1970-01-01T00:00:00+00:00'",
                    "ALTER TABLE relations ADD COLUMN valid_to TEXT",
                ]
                for stmt in migration_stmts:
                    try:
                        self._conn.execute(stmt)
                    except sqlite3.OperationalError:
                        pass  # Column already exists
        self._conn.commit()

    # ------------------------------------------------------------------
    # Entities
    # ------------------------------------------------------------------

    def upsert_entity(
        self,
        name: str,
        entity_type: str,
        valid_from: str | None = None,
    ) -> str:
        """Return the entity_id for *name*.

        Creates a new entity if one with that name does not yet exist and is
        currently active (valid_to IS NULL); otherwise returns the existing id.
        Accepts optional valid_from ISO timestamp (defaults to now).
        """
        row = self._conn.execute(
            "SELECT id FROM entities WHERE name = ? AND valid_to IS NULL", (name,)
        ).fetchone()
        if row is not None:
            return row[0]

        entity_id = str(uuid.uuid4())
        now = _now_iso()
        vfrom = valid_from if valid_from is not None else now
        self._conn.execute(
            "INSERT INTO entities (id, name, type, first_seen_at, valid_from, valid_to) "
            "VALUES (?, ?, ?, ?, ?, NULL)",
            (entity_id, name, entity_type, now, vfrom),
        )
        self._conn.commit()
        return entity_id

    def get_entities(self, memory_id: str) -> list[Entity]:
        """Return all active entities linked to *memory_id* via active relations."""
        rows = self._conn.execute(
            """
            SELECT DISTINCT e.id, e.name, e.type, e.first_seen_at, e.valid_from, e.valid_to
            FROM entities e
            JOIN relations r ON (r.source_id = e.id OR r.target_id = e.id)
            WHERE r.memory_id = ? AND r.valid_to IS NULL AND e.valid_to IS NULL
            """,
            (memory_id,),
        ).fetchall()
        return [
            Entity(
                id=r[0],
                name=r[1],
                type=r[2],
                first_seen_at=r[3],
                valid_from=r[4],
                valid_to=r[5],
            )
            for r in rows
        ]

    # ------------------------------------------------------------------
    # Relations
    # ------------------------------------------------------------------

    def add_relation(
        self,
        source_id: str,
        target_id: str,
        relation_type: str,
        memory_id: str,
        valid_from: str | None = None,
    ) -> None:
        """Persist a directed relation between two entities."""
        relation_id = str(uuid.uuid4())
        now = _now_iso()
        vfrom = valid_from if valid_from is not None else now
        self._conn.execute(
            """
            INSERT INTO relations
                (id, source_id, target_id, relation_type, observed_at, memory_id,
                 valid_from, valid_to)
            VALUES (?, ?, ?, ?, ?, ?, ?, NULL)
            """,
            (relation_id, source_id, target_id, relation_type, now, memory_id, vfrom),
        )
        self._conn.commit()

    def get_relations(self, entity_id: str) -> list[Relation]:
        """Return all active relations where *entity_id* is source or target."""
        rows = self._conn.execute(
            """
            SELECT id, source_id, target_id, relation_type, observed_at, memory_id,
                   valid_from, valid_to
            FROM relations
            WHERE (source_id = ? OR target_id = ?) AND valid_to IS NULL
            """,
            (entity_id, entity_id),
        ).fetchall()
        return [
            Relation(
                id=r[0],
                source_id=r[1],
                target_id=r[2],
                relation_type=r[3],
                observed_at=r[4],
                memory_id=r[5],
                valid_from=r[6],
                valid_to=r[7],
            )
            for r in rows
        ]

    # ------------------------------------------------------------------
    # Bitemporal revocation (D-03/D-15)
    # ------------------------------------------------------------------

    def invalidate(self, entity_id: str) -> None:
        """Mark entity valid_to = now (bitemporal revocation, D-03).

        Also invalidates all active relations where entity is source or target.
        """
        now = _now_iso()
        self._conn.execute(
            "UPDATE entities SET valid_to = ? WHERE id = ? AND valid_to IS NULL",
            (now, entity_id),
        )
        self._conn.execute(
            "UPDATE relations SET valid_to = ? "
            "WHERE (source_id = ? OR target_id = ?) AND valid_to IS NULL",
            (now, entity_id, entity_id),
        )
        self._conn.commit()

    def invalidate_by_memory(self, memory_id: str) -> list[str]:
        """Invalidate all entities (source and target) associated with a memory_id.

        Returns list of entity_ids that were invalidated.
        """
        rows = self._conn.execute(
            "SELECT DISTINCT source_id, target_id FROM relations "
            "WHERE memory_id = ? AND valid_to IS NULL",
            (memory_id,),
        ).fetchall()
        entity_ids: list[str] = []
        for r in rows:
            if r[0] not in entity_ids:
                entity_ids.append(r[0])
            if r[1] not in entity_ids:
                entity_ids.append(r[1])
        for eid in entity_ids:
            self.invalidate(eid)
        return entity_ids

    # ------------------------------------------------------------------
    # Lifecycle
    # ------------------------------------------------------------------

    def close(self) -> None:
        self._conn.close()
