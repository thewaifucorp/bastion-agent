"""Tests for versioning.py (D-07/SKWR-04)."""
from __future__ import annotations
import time
from datetime import UTC, datetime, timedelta

import pytest


@pytest.fixture()
def tmp_skill(tmp_path):
    skill_path = tmp_path / "SKILL.md"
    skill_path.write_text("<name>test-skill</name>\n<description>v1</description>", encoding="utf-8")
    return skill_path


class TestSnapshot:
    def test_snapshot_creates_versioned_file(self, tmp_skill):
        from versioning import list_snapshots, snapshot

        snapshot(tmp_skill)
        time.sleep(0.05)  # allow ThreadPoolExecutor to complete
        snaps = list_snapshots(tmp_skill)
        assert len(snaps) == 1
        assert snaps[0].name.startswith("SKILL.md.")

    def test_snapshot_preserves_content(self, tmp_skill):
        from versioning import list_snapshots, snapshot

        snapshot(tmp_skill)
        time.sleep(0.05)
        snaps = list_snapshots(tmp_skill)
        assert snaps[0].read_text(encoding="utf-8") == tmp_skill.read_text(encoding="utf-8")

    def test_snapshot_nonexistent_skill_does_not_raise(self, tmp_path):
        from versioning import list_snapshots, snapshot

        ghost = tmp_path / "ghost" / "SKILL.md"
        # Must not raise even though parent dir doesn't exist
        snapshot(ghost)
        time.sleep(0.05)
        assert list_snapshots(ghost) == []

    def test_multiple_snapshots_are_sorted(self, tmp_path):
        from versioning import VERSIONS_DIR, SNAPSHOT_PREFIX, list_snapshots

        # Create two snapshots manually with distinct timestamps (avoids 1-second resolution gap)
        skill_path = tmp_path / "SKILL.md"
        skill_path.write_text("v1", encoding="utf-8")
        versions_dir = tmp_path / VERSIONS_DIR
        versions_dir.mkdir()
        snap1 = versions_dir / f"{SNAPSHOT_PREFIX}20260601T100000Z"
        snap2 = versions_dir / f"{SNAPSHOT_PREFIX}20260601T110000Z"
        snap1.write_text("v1", encoding="utf-8")
        snap2.write_text("v2", encoding="utf-8")

        snaps = list_snapshots(skill_path)
        assert len(snaps) == 2
        # Sorted oldest first
        assert snaps[0].name < snaps[1].name


class TestRollback:
    def test_rollback_to_date_hint_ontem(self, tmp_skill):
        from versioning import rollback_to_date

        # Simulate an old snapshot by writing a file directly
        versions_dir = tmp_skill.parent / ".versions"
        versions_dir.mkdir(exist_ok=True)
        yesterday_dt = datetime.now(UTC) - timedelta(days=1)
        yesterday = yesterday_dt.strftime("%Y%m%dT%H%M%SZ")
        snap = versions_dir / f"SKILL.md.{yesterday}"
        snap.write_text("<name>test-skill</name>\n<description>v0-yesterday</description>", encoding="utf-8")

        result = rollback_to_date(tmp_skill, "ontem")
        assert result is not None
        assert result.startswith("SKILL.md.")
        assert "v0-yesterday" in tmp_skill.read_text(encoding="utf-8")

    def test_rollback_to_iso_date(self, tmp_skill):
        from versioning import rollback_to_date

        versions_dir = tmp_skill.parent / ".versions"
        versions_dir.mkdir(exist_ok=True)
        # Place snapshot on 2026-06-01
        snap_ts = "20260601T120000Z"
        snap = versions_dir / f"SKILL.md.{snap_ts}"
        snap.write_text("version-from-2026-06-01", encoding="utf-8")

        result = rollback_to_date(tmp_skill, "2026-06-01")
        assert result is not None
        assert "version-from-2026-06-01" in tmp_skill.read_text(encoding="utf-8")

    def test_rollback_no_snapshot_returns_none(self, tmp_path):
        from versioning import rollback_to_date

        skill = tmp_path / "SKILL.md"
        skill.write_text("content", encoding="utf-8")
        result = rollback_to_date(skill, "ontem")
        assert result is None

    def test_rollback_bad_date_hint_returns_none(self, tmp_skill):
        from versioning import rollback_to_date

        result = rollback_to_date(tmp_skill, "isso não é uma data")
        assert result is None
