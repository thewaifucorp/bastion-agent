"""Unit tests for Output Validator alerts."""

import json
from pathlib import Path

import pytest

from output_validator.alerts import Alert, AlertScanner, run_alert_scan, MIN_SAMPLES_FOR_DRIFT


class TestAlert:
    def test_to_log_dict(self):
        alert = Alert(
            level="warning",
            kind="test_kind",
            skill="test-skill",
            message="test message",
            details={"extra": 123}
        )
        expected = {
            "event": "alert_test_kind",
            "level": "warning",
            "skill": "test-skill",
            "message": "test message",
            "extra": 123,
        }
        assert alert.to_log_dict() == expected


class TestAlertScannerDrift:
    def test_no_alert_when_window_too_small(self):
        scanner = AlertScanner(drift_threshold=0.90)
        stats = {"recent_success_rate": 0.5, "recent_window_size": MIN_SAMPLES_FOR_DRIFT - 1}
        alerts = scanner._check_drift("skill-a", stats)
        assert not alerts

    def test_no_alert_when_rate_above_threshold(self):
        scanner = AlertScanner(drift_threshold=0.90)
        stats = {"recent_success_rate": 0.95, "recent_window_size": MIN_SAMPLES_FOR_DRIFT}
        alerts = scanner._check_drift("skill-a", stats)
        assert not alerts

    def test_alert_when_rate_below_threshold(self):
        scanner = AlertScanner(drift_threshold=0.90)
        stats = {
            "recent_success_rate": 0.85,
            "recent_window_size": MIN_SAMPLES_FOR_DRIFT,
            "last_error": {"errors": ["test error"]}
        }
        alerts = scanner._check_drift("skill-a", stats)
        assert len(alerts) == 1
        alert = alerts[0]
        assert alert.level == "warning"
        assert alert.kind == "drift"
        assert alert.skill == "skill-a"
        assert "0.85" in alert.message or "85.0%" in alert.message
        assert "test error" in alert.message


class TestAlertScannerErrorSpike:
    def test_no_alert_when_samples_too_small(self):
        scanner = AlertScanner()
        stats = {
            "total_validations": MIN_SAMPLES_FOR_DRIFT - 1,
            "recent_window_size": MIN_SAMPLES_FOR_DRIFT,
            "overall_success_rate": 0.9,
            "recent_success_rate": 0.5,
        }
        assert not scanner._check_error_spike("skill-b", stats)

        stats["total_validations"] = MIN_SAMPLES_FOR_DRIFT
        stats["recent_window_size"] = MIN_SAMPLES_FOR_DRIFT - 1
        assert not scanner._check_error_spike("skill-b", stats)

    def test_no_alert_when_spike_below_multiplier(self):
        scanner = AlertScanner()
        stats = {
            "total_validations": MIN_SAMPLES_FOR_DRIFT,
            "recent_window_size": MIN_SAMPLES_FOR_DRIFT,
            "overall_success_rate": 0.9, # error rate 0.1
            "recent_success_rate": 0.85, # error rate 0.15 (< 0.2)
        }
        assert not scanner._check_error_spike("skill-b", stats)

    def test_alert_when_spike_exceeds_multiplier(self):
        scanner = AlertScanner()
        stats = {
            "total_validations": MIN_SAMPLES_FOR_DRIFT,
            "recent_window_size": MIN_SAMPLES_FOR_DRIFT,
            "overall_success_rate": 0.9, # error rate 0.1
            "recent_success_rate": 0.7, # error rate 0.3 (>= 0.2)
        }
        alerts = scanner._check_error_spike("skill-b", stats)
        assert len(alerts) == 1
        alert = alerts[0]
        assert alert.level == "warning"
        assert alert.kind == "error_spike"
        assert alert.skill == "skill-b"
        assert "30.0%" in alert.message
        assert "10.0%" in alert.message


class TestAlertScannerSchemaMissing:
    def test_skills_dir_does_not_exist(self, tmp_path):
        scanner = AlertScanner(skills_dir=tmp_path / "nonexistent")
        assert not scanner._check_schema_missing()

    def test_schema_present_no_alert(self, tmp_path):
        skill_dir = tmp_path / "skill-ok"
        skill_dir.mkdir()
        (skill_dir / "schema.json").write_text("{}")

        scanner = AlertScanner(skills_dir=tmp_path)
        assert not scanner._check_schema_missing()

    def test_schema_missing_no_example(self, tmp_path):
        skill_dir = tmp_path / "skill-miss"
        skill_dir.mkdir()
        (skill_dir / "SKILL.md").write_text("# Title\nSome content.")

        scanner = AlertScanner(skills_dir=tmp_path)
        alerts = scanner._check_schema_missing()
        assert len(alerts) == 1
        assert alerts[0].kind == "schema_missing"
        assert alerts[0].skill == "skill-miss"

    def test_schema_missing_has_example(self, tmp_path):
        skill_dir = tmp_path / "skill-gen-fail"
        skill_dir.mkdir()
        (skill_dir / "SKILL.md").write_text("# Title\n## Output Example\n```json\n{}\n```")

        scanner = AlertScanner(skills_dir=tmp_path)
        alerts = scanner._check_schema_missing()
        assert len(alerts) == 1
        assert alerts[0].kind == "schema_gen_failed"
        assert alerts[0].skill == "skill-gen-fail"


class TestAlertScannerScan:
    def test_scan_aggregates_and_logs(self, monkeypatch, caplog):
        scanner = AlertScanner()

        # Mock get_stats to trigger one drift and one spike alert
        def mock_get_stats():
            return {
                "skill-drift": {
                    "recent_success_rate": 0.5,
                    "recent_window_size": MIN_SAMPLES_FOR_DRIFT,
                },
                "skill-spike": {
                    "total_validations": MIN_SAMPLES_FOR_DRIFT,
                    "recent_window_size": MIN_SAMPLES_FOR_DRIFT,
                    "overall_success_rate": 0.99,
                    "recent_success_rate": 0.95,
                }
            }
        monkeypatch.setattr(scanner._tracker, "get_stats", mock_get_stats)

        # Mock _check_schema_missing to return one alert
        monkeypatch.setattr(scanner, "_check_schema_missing", lambda: [
            Alert(level="error", kind="schema_missing", skill="skill-miss", message="err")
        ])

        with caplog.at_level("WARNING"):
            alerts = scanner.scan()

        assert len(alerts) == 3
        kinds = [a.kind for a in alerts]
        assert "drift" in kinds
        assert "error_spike" in kinds
        assert "schema_missing" in kinds

        # Verify logging
        log_events = [json.loads(r.message)["event"] for r in caplog.records]
        assert "alert_drift" in log_events
        assert "alert_error_spike" in log_events
        assert "alert_schema_missing" in log_events


def test_run_alert_scan(monkeypatch):
    called = False

    def mock_scan(self):
        nonlocal called
        called = True
        return [Alert(level="warning", kind="test", skill="test", message="test")]

    monkeypatch.setattr(AlertScanner, "scan", mock_scan)

    alerts = run_alert_scan()
    assert called
    assert len(alerts) == 1
