"""
Bastion Output Validator
========================

Automatic output validation for Bastion skills.

Generates JSON Schema Draft 7 from ``## Output Example`` sections in SKILL.md
files and validates LLM outputs against those schemas at runtime.

Usage::

    from skills.output_validator import validate_skill_output

    result = validate_skill_output("life-log", output)
    if not result.is_valid:
        logger.error("Validation failed: %s", result.errors)

Metrics tracking is enabled by default. Disable with ``track_metrics=False``::

    result = validate_skill_output("life-log", output, track_metrics=False)
"""

import json
import logging
from pathlib import Path
from typing import Any

from .alerts import Alert, run_alert_scan
from .auto_validator import AutoValidator, ValidationResult
from .metrics_tracker import MetricsTracker

logger = logging.getLogger(__name__)

_SKILLS_DIR = Path("skills")
_METRICS_FILE = Path("config/logs/validation-metrics.json")

# Singleton instances (lazy-initialised)
_validator: AutoValidator | None = None
_tracker: MetricsTracker | None = None


def _get_validator() -> AutoValidator:
    global _validator
    if _validator is None:
        _validator = AutoValidator(_SKILLS_DIR)
    return _validator


def _get_tracker() -> MetricsTracker:
    global _tracker
    if _tracker is None:
        _tracker = MetricsTracker(_METRICS_FILE)
    return _tracker


def validate_skill_output(
    skill_name: str,
    output: Any,
    track_metrics: bool = True,
) -> ValidationResult:
    """
    Validate a skill's LLM output.

    Generates a JSON Schema automatically from the skill's SKILL.md if none
    exists yet. Returns a valid result with a warning when the skill has no
    ``## Output Example`` defined.

    Args:
        skill_name: Skill directory name (e.g. ``"life-log"``).
        output: LLM output — dict, list, or JSON string.
        track_metrics: If True (default), record the result in the metrics
            tracker. Set to False to skip tracking.

    Returns:
        :class:`ValidationResult` with ``is_valid``, ``errors``, ``warnings``,
        ``schema_generated``, and ``schema_path``.

    Example::

        >>> result = validate_skill_output("life-log", {"entry": "..."})
        >>> if not result.is_valid:
        ...     print(f"Errors: {result.errors}")
    """
    validator = _get_validator()
    result = validator.validate_skill_output(skill_name, output)

    # Log schema generation
    if result.schema_generated:
        logger.info(
            json.dumps({
                "event": "schema_generated",
                "skill": skill_name,
                "schema_path": str(result.schema_path),
            })
        )

    # Log validation failures
    if not result.is_valid:
        logger.warning(
            json.dumps({
                "event": "validation_failed",
                "skill": skill_name,
                "errors": result.errors,
            })
        )

    # Log drift warnings (forwarded from metrics tracker)
    if result.warnings:
        for w in result.warnings:
            logger.info(
                json.dumps({
                    "event": "validation_warning",
                    "skill": skill_name,
                    "warning": w,
                })
            )

    # Track metrics (only when a schema was actually used)
    if track_metrics and result.schema_path and not result.schema_generated:
        tracker = _get_tracker()
        tracker.record_validation(skill_name, result.is_valid, result.errors)

    return result


__all__ = ["Alert", "AutoValidator", "MetricsTracker", "ValidationResult", "run_alert_scan", "validate_skill_output"]
