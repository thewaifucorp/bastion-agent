"""Regression for WR-02: rollback_to_date must snapshot current state first.

The bug: rollback_to_date() overwrote SKILL.md with the chosen snapshot WITHOUT
first snapshotting the current (pre-rollback) content — so the state you rolled
away from was permanently lost and the rollback was irreversible. skill_create
and skill_edit both snapshot before writing; rollback must too.
"""
from __future__ import annotations

import time

VERSIONS_DIR = ".versions"
SNAPSHOT_PREFIX = "SKILL.md."


class TestRollbackSnapshotsCurrentFirst:
    def test_rollback_preserves_pre_rollback_state(self, tmp_path):
        from versioning import list_snapshots, rollback_to_date, snapshot

        p = tmp_path / "SKILL.md"

        # 1. An older version exists as a snapshot (the rollback target).
        p.write_text("old version", encoding="utf-8")
        snapshot(p)
        time.sleep(0.3)

        # 2. Current state is newer content with no snapshot yet.
        p.write_text("current version", encoding="utf-8")
        snaps_before = len(list_snapshots(p))
        assert snaps_before == 1

        # 3. Roll back to today (matches the only snapshot's date).
        from datetime import UTC, datetime

        today = datetime.now(UTC).strftime("%Y-%m-%d")
        restored = rollback_to_date(p, today)
        assert restored is not None
        time.sleep(0.3)

        # 4. File now holds the rolled-back content...
        assert p.read_text(encoding="utf-8") == "old version"

        # 5. ...AND a fresh snapshot of "current version" was taken before the
        # overwrite, so the rollback is reversible (no data loss).
        snaps_after = list_snapshots(p)
        assert len(snaps_after) == snaps_before + 1, (
            "WR-02: rollback did not snapshot the pre-rollback state"
        )
        contents = {s.read_text(encoding="utf-8") for s in snaps_after}
        assert "current version" in contents, (
            "WR-02: pre-rollback 'current version' was lost — not snapshotted"
        )
