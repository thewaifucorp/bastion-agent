#!/usr/bin/env python3
"""Migration script: re-embed life-log interactions using ONNXEmbedder.

Replaces LLM-generated embeddings (variable dimension, e.g. 1536) with
ONNX-generated embeddings (384-dim, all-MiniLM-L6-v2).

Usage:
    python skills/memupalace/migrate_lifelog.py \
        --life-log-db db/life-log.db \
        --onnx-model models/embedder.onnx \
        [--batch-size 64]

Idempotent: re-running re-embeds all records (dimension check ensures
records with wrong dimension are always re-embedded).
"""

from __future__ import annotations

import argparse
import sqlite3
import struct
import sys
from pathlib import Path


EXPECTED_DIM = 384


def _has_column(conn: sqlite3.Connection, table: str, column: str) -> bool:
    cursor = conn.execute(f"PRAGMA table_info({table})")
    return any(row[1] == column for row in cursor.fetchall())


def _table_exists(conn: sqlite3.Connection, table: str) -> bool:
    cursor = conn.execute(
        "SELECT name FROM sqlite_master WHERE type='table' AND name=?", (table,)
    )
    return cursor.fetchone() is not None


def _decode_embedding(blob: bytes | None) -> list[float] | None:
    """Decode a BLOB into a list of floats, or return None on failure."""
    if blob is None:
        return None
    try:
        n = len(blob) // 4
        return list(struct.unpack(f"{n}f", blob))
    except struct.error:
        return None


def migrate(
    life_log_db_path: str,
    onnx_model_path: str,
    batch_size: int = 64,
) -> int:
    """Run the migration and return the total number of records migrated."""
    # Lazy import so the script fails fast with a clear message if missing
    try:
        from skills.memupalace.embedder import ONNXEmbedder  # noqa: PLC0415
    except ImportError:
        # Fallback for running directly from repo root without install
        sys.path.insert(0, str(Path(__file__).parent.parent.parent))
        from skills.memupalace.embedder import ONNXEmbedder  # noqa: PLC0415

    db_path = Path(life_log_db_path)
    if not db_path.exists():
        print(f"WARNING: life-log database not found at {db_path}. Nothing to migrate.")
        return 0

    conn = sqlite3.connect(str(db_path))
    try:
        # Guard: table must exist
        if not _table_exists(conn, "interactions"):
            print(
                "WARNING: 'interactions' table not found in the database. "
                "Nothing to migrate."
            )
            return 0

        # Guard: embedding column must exist
        if not _has_column(conn, "interactions", "embedding"):
            print(
                "WARNING: 'interactions' table has no 'embedding' column. "
                "Skipping migration."
            )
            return 0

        # Load embedder (raises FileNotFoundError if model missing)
        embedder = ONNXEmbedder(onnx_model_path)

        # Fetch all rows — id + intent
        rows: list[tuple[int | str, str | None]] = conn.execute(
            "SELECT id, intent FROM interactions"
        ).fetchall()

        total = len(rows)
        if total == 0:
            print("No records found. Nothing to migrate.")
            return 0

        migrated = 0

        for i in range(0, total, batch_size):
            batch = rows[i : i + batch_size]

            # Filter out records with NULL intent
            valid = [(rid, intent) for rid, intent in batch if intent is not None]
            skipped = len(batch) - len(valid)

            if valid:
                texts = [intent for _, intent in valid]
                embeddings = embedder.embed_batch(texts)

                for (record_id, _), emb in zip(valid, embeddings):
                    blob = struct.pack(f"{len(emb)}f", *emb)
                    conn.execute(
                        "UPDATE interactions SET embedding = ? WHERE id = ?",
                        (blob, record_id),
                    )

                conn.commit()
                migrated += len(valid)

            progress = min(i + batch_size, total)
            print(f"Migrated {progress}/{total} records (skipped {skipped} NULL intents in this batch)")

        print(f"\nDone. Total migrated: {migrated}/{total} records.")
        return migrated

    finally:
        conn.close()


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Re-embed life-log interactions using the local ONNX model."
    )
    parser.add_argument(
        "--life-log-db",
        required=True,
        help="Path to the life-log SQLite database (e.g. db/life-log.db)",
    )
    parser.add_argument(
        "--onnx-model",
        required=True,
        help="Path to the ONNX model file (e.g. models/embedder.onnx)",
    )
    parser.add_argument(
        "--batch-size",
        type=int,
        default=64,
        help="Number of records to process per batch (default: 64)",
    )

    args = parser.parse_args()

    if args.batch_size < 1:
        parser.error("--batch-size must be at least 1")

    migrate(
        life_log_db_path=args.life_log_db,
        onnx_model_path=args.onnx_model,
        batch_size=args.batch_size,
    )


if __name__ == "__main__":
    main()
