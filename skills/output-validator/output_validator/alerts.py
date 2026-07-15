"""
Monitoring alerts for the Output Validator.

Scans validation metrics and emits structured log alerts for:
- Skills with recent success rate below the drift threshold (< 90%)
- Skills where schema generation failed (no schema.json and no Output Example)
- Spikes in validation errors (recent error rate > 2x the overall error rate)

Integrates with the existing Bastion logging system via Python's standard
logging module. Alerts are emitted as WARNING-level structured JSON entries
so they are picked up by any log aggregator watching the Bastion process.

Usage (standalone scan, e.g. from HEARTBEAT)::

    from output_validator.alerts import run_alert_scan
    run_alert_scan()

Usage (programmatic)::

    from output_validator.alerts import AlertScanner
    scanner = AlertScanner()
    alerts = scanner.scan()
    for alert in alerts:
        print(alert["message"])
"""

import json
import logging
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Dict, List, Optional

from .metrics_tracker import MetricsTracker, DRIFT_THRESHOLD, MIN_SAMPLES_FOR_DRIFT

logger = logging.getLogger(__name__)

_METRICS_FILE = Path("config/logs/validation-metrics.json")
_SKILLS_DIR = Path("skills")

# A spike is flagged when recent error rate is more than 2x the overall error rate
_SPIKE_MULTIPLIER = 2.0


@dataclass
class Alert:
    """A single monitoring alert."""

    level: str          # "warning" | "error"
    kind: str           # "drift" | "schema_missing" | "error_spike" | "schema_gen_failed"
    skill: str
    message: str
    details: Dict[str, Any] = field(default_factory=dict)

    def to_log_dict(self) -> Dict[str, Any]:
        return {
            "event": f"alert_{self.kind}",
            "level": self.level,
            "skill": self.skill,
            "message": self.message,
            **self.details,
        }


class AlertScanner:
    """
    Scans validation metrics and skill directories for alert conditions.

    Args:
        metrics_file: Path to the validation metrics JSON file.
        skills_dir: Root directory containing skill sub-directories.
        drift_threshold: Success-rate threshold for drift alerts (default 0.90).
    """

    def __init__(
        self,
        metrics_file: Path = _METRICS_FILE,
        skills_dir: Path = _SKILLS_DIR,
        drift_threshold: float = DRIFT_THRESHOLD,
    ) -> None:
        self.metrics_file = Path(metrics_file)
        self.skills_dir = Path(skills_dir)
        self.drift_threshold = drift_threshold
        self._tracker = MetricsTracker(metrics_file, drift_threshold=drift_threshold)

    # ------------------------------------------------------------------
    # Public API
    # ------------------------------------------------------------------

    def scan(self) -> List[Alert]:
        """
        Run all alert checks and return a list of triggered alerts.

        Checks performed:
        1. Drift: recent success rate < threshold (min 20 samples)
        2. Error spike: recent error rate > 2x overall error rate
        3. Schema missing: skill directory exists but has no schema.json
           and no ## Output Example in SKILL.md
        4. Schema generation failed: schema.json absent despite SKILL.md example

        Returns:
            List of :class:`Alert` objects (may be empty).
        """
        alerts: List[Alert] = []

        all_stats = self._tracker.get_stats()
        for skill_name, stats in all_stats.items():
            alerts.extend(self._check_drift(skill_name, stats))
            alerts.extend(self._check_error_spike(skill_name, stats))

        alerts.extend(self._check_schema_missing())

        # Emit all alerts to the log
        for alert in alerts:
            if alert.level == "error":
                logger.error(json.dumps(alert.to_log_dict()))
            else:
                logger.warning(json.dumps(alert.to_log_dict()))

        return alerts

    # ------------------------------------------------------------------
    # Private checks
    # ------------------------------------------------------------------

    def _check_drift(self, skill_name: str, stats: Dict[str, Any]) -> List[Alert]:
        """Alert when recent success rate drops below the threshold."""
        recent_rate: float = stats.get("recent_success_rate", 1.0)
        window_size: int = stats.get("recent_window_size", 0)

        if window_size < MIN_SAMPLES_FOR_DRIFT:
            return []

        if recent_rate < self.drift_threshold:
            last_error = stats.get("last_error") or {}
            last_msg = (last_error.get("errors") or ["(unknown)"])[0]
            return [Alert(
                level="warning",
                kind="drift",
                skill=skill_name,
                message=(
                    f"⚠️ Drift de validação em '{skill_name}': "
                    f"taxa de sucesso = {recent_rate:.1%} "
                    f"(últimas {window_size} execuções). "
                    f"Último erro: {last_msg}"
                ),
                details={
                    "recent_success_rate": recent_rate,
                    "window_size": window_size,
                    "threshold": self.drift_threshold,
                    "last_error": last_msg,
                },
            )]
        return []

    def _check_error_spike(self, skill_name: str, stats: Dict[str, Any]) -> List[Alert]:
        """Alert when recent error rate is more than 2x the overall error rate."""
        total: int = stats.get("total_validations", 0)
        overall_rate: float = stats.get("overall_success_rate", 1.0)
        recent_rate: float = stats.get("recent_success_rate", 1.0)
        window_size: int = stats.get("recent_window_size", 0)

        if total < MIN_SAMPLES_FOR_DRIFT or window_size < MIN_SAMPLES_FOR_DRIFT:
            return []

        overall_error_rate = 1.0 - overall_rate
        recent_error_rate = 1.0 - recent_rate

        if overall_error_rate > 0 and recent_error_rate >= overall_error_rate * _SPIKE_MULTIPLIER:
            return [Alert(
                level="warning",
                kind="error_spike",
                skill=skill_name,
                message=(
                    f"⚠️ Spike de erros em '{skill_name}': "
                    f"taxa de erro recente = {recent_error_rate:.1%} "
                    f"(vs. {overall_error_rate:.1%} histórico)"
                ),
                details={
                    "recent_error_rate": recent_error_rate,
                    "overall_error_rate": overall_error_rate,
                    "spike_multiplier": _SPIKE_MULTIPLIER,
                },
            )]
        return []

    def _check_schema_missing(self) -> List[Alert]:
        """Alert for skill directories that have no schema and no Output Example."""
        alerts: List[Alert] = []

        if not self.skills_dir.exists():
            return alerts

        for skill_dir in sorted(self.skills_dir.iterdir()):
            if not skill_dir.is_dir():
                continue
            skill_name = skill_dir.name

            schema_path = skill_dir / "schema.json"
            skill_md_path = skill_dir / "SKILL.md"

            if schema_path.exists():
                continue  # schema present — all good

            # No schema.json — check if SKILL.md has an Output Example
            has_example = False
            if skill_md_path.exists():
                try:
                    content = skill_md_path.read_text(encoding="utf-8")
                    import re
                    has_example = bool(
                        re.search(r'##\s+Output\s+Example', content, re.IGNORECASE)
                    )
                except OSError:
                    pass

            if not has_example:
                alerts.append(Alert(
                    level="warning",
                    kind="schema_missing",
                    skill=skill_name,
                    message=(
                        f"⚠️ Skill '{skill_name}' sem schema de validação configurado. "
                        f"Adicione ## Output Example ao SKILL.md."
                    ),
                    details={"schema_path": str(schema_path)},
                ))
            else:
                # Has example but schema.json not generated yet
                alerts.append(Alert(
                    level="warning",
                    kind="schema_gen_failed",
                    skill=skill_name,
                    message=(
                        f"⚠️ Skill '{skill_name}' tem ## Output Example mas schema.json "
                        f"não foi gerado. Execute: "
                        f"python -m output_validator regenerate {skill_name}"
                    ),
                    details={"schema_path": str(schema_path)},
                ))

        return alerts


def run_alert_scan(
    metrics_file: Path = _METRICS_FILE,
    skills_dir: Path = _SKILLS_DIR,
) -> List[Alert]:
    """
    Convenience function: run a full alert scan and return triggered alerts.

    Intended to be called from HEARTBEAT tasks or monitoring scripts.

    Returns:
        List of :class:`Alert` objects.
    """
    scanner = AlertScanner(metrics_file=metrics_file, skills_dir=skills_dir)
    return scanner.scan()
