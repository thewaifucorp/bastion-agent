"""
Metrics Tracker — tracks validation success rates per skill and detects drift.

Persists metrics to a JSON file. Uses a sliding window (deque) to compute
recent success rates and warns when the rate drops below a configurable threshold.
"""

import json
import logging
from collections import deque
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Optional

logger = logging.getLogger(__name__)

DEFAULT_WINDOW_SIZE = 100
DRIFT_THRESHOLD = 0.90          # 90 %
MIN_SAMPLES_FOR_DRIFT = 20      # minimum window entries before drift check


class MetricsTracker:
    """
    Tracks validation metrics per skill with drift detection.

    Metrics are persisted to a JSON file so they survive process restarts.
    A sliding window of the last ``window_size`` results is used to compute
    the recent success rate.

    Args:
        metrics_file: Path to the JSON file used for persistence.
        window_size: Size of the sliding window (default 100).
        drift_threshold: Success-rate threshold below which drift is reported
            (default 0.90 = 90 %).

    Example::

        tracker = MetricsTracker(Path("config/logs/validation-metrics.json"))
        tracker.record_validation("life-log", is_valid=True, errors=[])
        stats = tracker.get_stats("life-log")
    """

    def __init__(
        self,
        metrics_file: Path,
        window_size: int = DEFAULT_WINDOW_SIZE,
        drift_threshold: float = DRIFT_THRESHOLD,
    ) -> None:
        self.metrics_file = Path(metrics_file)
        self.window_size = window_size
        self.drift_threshold = drift_threshold
        self.metrics: dict[str, Any] = self._load_metrics()

    # ------------------------------------------------------------------
    # Public API
    # ------------------------------------------------------------------

    def record_validation(
        self,
        skill_name: str,
        is_valid: bool,
        errors: list[str],
    ) -> None:
        """
        Record a validation result for a skill.

        Increments counters, updates the sliding window, persists to disk,
        and checks for drift.

        Args:
            skill_name: Skill directory name (e.g. ``"life-log"``).
            is_valid: Whether the validation passed.
            errors: List of validation error messages (empty if valid).
        """
        if skill_name not in self.metrics:
            self.metrics[skill_name] = {
                "total": 0,
                "valid": 0,
                "recent": [],
                "last_error": None,
                "last_updated": None,
            }

        m = self.metrics[skill_name]
        m["total"] += 1
        if is_valid:
            m["valid"] += 1

        # Maintain sliding window as a plain list (JSON-serialisable)
        recent: list[bool] = m["recent"]
        recent.append(is_valid)
        if len(recent) > self.window_size:
            recent.pop(0)

        m["last_updated"] = datetime.now(tz=timezone.utc).isoformat()

        if not is_valid:
            m["last_error"] = {
                "timestamp": datetime.now(tz=timezone.utc).isoformat(),
                "errors": errors,
            }

        self._save_metrics()
        self._check_drift(skill_name)

    def get_stats(self, skill_name: Optional[str] = None) -> dict[str, Any]:
        """
        Return validation statistics.

        Args:
            skill_name: If provided, return stats for that skill only.
                        Otherwise return stats for all tracked skills.

        Returns:
            A dict of formatted stats (or a dict of dicts for all skills).
        """
        if skill_name:
            if skill_name not in self.metrics:
                return {}
            return self._format_skill_stats(skill_name)

        return {
            name: self._format_skill_stats(name)
            for name in self.metrics
        }

    # ------------------------------------------------------------------
    # Private helpers
    # ------------------------------------------------------------------

    def _load_metrics(self) -> dict[str, Any]:
        """Load metrics from the JSON file, or return an empty dict."""
        if not self.metrics_file.exists():
            logger.debug("Metrics file not found, starting fresh: %s", self.metrics_file)
            return {}
        try:
            data = json.loads(self.metrics_file.read_text(encoding="utf-8"))
            logger.debug("Loaded metrics from %s", self.metrics_file)
            return data
        except (OSError, json.JSONDecodeError) as exc:
            logger.error("Cannot load metrics from %s: %s", self.metrics_file, exc)
            return {}

    def _save_metrics(self) -> None:
        """Persist metrics to the JSON file."""
        try:
            self.metrics_file.parent.mkdir(parents=True, exist_ok=True)
            self.metrics_file.write_text(
                json.dumps(self.metrics, indent=2, ensure_ascii=False),
                encoding="utf-8",
            )
        except OSError as exc:
            logger.error("Cannot save metrics to %s: %s", self.metrics_file, exc)

    def _check_drift(self, skill_name: str) -> None:
        """Log a warning if the recent success rate is below the threshold."""
        recent: list[bool] = self.metrics[skill_name]["recent"]
        if len(recent) < MIN_SAMPLES_FOR_DRIFT:
            return

        success_rate = sum(recent) / len(recent)
        if success_rate < self.drift_threshold:
            logger.warning(
                "⚠️  Drift detected in '%s': success rate = %.1f%% "
                "(last %d executions)",
                skill_name,
                success_rate * 100,
                len(recent),
            )

    def _format_skill_stats(self, skill_name: str) -> dict[str, Any]:
        """Format statistics for a single skill."""
        m = self.metrics[skill_name]
        recent: list[bool] = m["recent"]
        total: int = m["total"]
        valid: int = m["valid"]

        overall_rate = valid / total if total > 0 else 0.0
        recent_rate = sum(recent) / len(recent) if recent else 0.0

        return {
            "skill": skill_name,
            "total_validations": total,
            "total_valid": valid,
            "overall_success_rate": overall_rate,
            "recent_success_rate": recent_rate,
            "recent_window_size": len(recent),
            "last_error": m.get("last_error"),
            "last_updated": m.get("last_updated"),
        }
