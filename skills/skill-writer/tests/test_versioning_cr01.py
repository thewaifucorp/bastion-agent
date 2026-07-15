"""Test for CR-01: snapshot() must capture content on caller thread.

The bug: snapshot() currently reads file content inside _write() on the
background thread. If main thread writes new content between snapshot()
returning and the background thread executing, the snapshot captures
post-edit content instead of pre-edit content — corrupting rollback history.

Fix: content must be read synchronously on the caller thread before
_executor.submit(), so the background closure only writes bytes it received.
"""
from __future__ import annotations

import time
import pathlib as _pl

import pytest


class TestSnapshotCallerThreadRead:
    """CR-01: content captured on caller thread, not background thread."""

    def test_snapshot_captures_pre_edit_content_despite_race(self, tmp_path):
        """Core CR-01 regression: snapshot must preserve pre-edit bytes.

        Steps:
          1. Write 'original content' to SKILL.md
          2. Call snapshot() — should capture 'original content'
          3. Immediately overwrite file with 'edited content' on main thread
          4. Wait for background thread
          5. Verify snapshot contains 'original content', not 'edited content'
        """
        from versioning import snapshot, list_snapshots

        p = tmp_path / "SKILL.md"
        p.write_text("original content", encoding="utf-8")

        snapshot(p)

        # Simulate main-thread write racing with background thread
        p.write_text("edited content", encoding="utf-8")

        time.sleep(0.3)

        snaps = list_snapshots(p)
        assert len(snaps) == 1, f"expected 1 snapshot, got {len(snaps)}"
        snap_content = snaps[0].read_text(encoding="utf-8")
        assert snap_content == "original content", (
            f"CR-01: snapshot has wrong content: {snap_content!r} "
            f"(expected 'original content' — content was read on background thread after main-thread overwrite)"
        )

    def test_snapshot_nonexistent_path_is_silent(self, tmp_path):
        """Non-existent skill path: snapshot() returns silently, no thread task."""
        from versioning import snapshot, list_snapshots

        ghost = tmp_path / "no-such-dir" / "SKILL.md"
        snapshot(ghost)  # must not raise
        time.sleep(0.1)
        assert list_snapshots(ghost) == []

    def test_read_text_not_inside_write_closure(self):
        """Structural: verify read_text is called on caller thread (source inspection)."""
        import ast

        src = (_pl.Path(__file__).parent.parent / "versioning.py").read_text(encoding="utf-8")
        tree = ast.parse(src)

        # Find snapshot() function
        snapshot_func = None
        for node in ast.walk(tree):
            if isinstance(node, ast.FunctionDef) and node.name == "snapshot":
                snapshot_func = node
                break

        assert snapshot_func is not None, "snapshot() function not found in versioning.py"

        # Find _write() inner function inside snapshot()
        write_func = None
        for node in ast.walk(snapshot_func):
            if isinstance(node, ast.FunctionDef) and node.name == "_write":
                write_func = node
                break

        assert write_func is not None, "_write() inner function not found inside snapshot()"

        # Collect all read_text calls inside _write()
        write_read_text_calls = []
        for node in ast.walk(write_func):
            if (
                isinstance(node, ast.Call)
                and isinstance(node.func, ast.Attribute)
                and node.func.attr == "read_text"
            ):
                write_read_text_calls.append(node)

        assert len(write_read_text_calls) == 0, (
            f"CR-01: read_text() found inside _write() closure — "
            f"content must be read on caller thread before _executor.submit(), "
            f"found {len(write_read_text_calls)} call(s) inside _write()"
        )
