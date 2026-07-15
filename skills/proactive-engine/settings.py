"""Configuration for the proactive-engine skill."""

from __future__ import annotations

import os
from dataclasses import dataclass


@dataclass
class ProactiveSettings:
    llm_model: str = "google/gemini-flash-1.5"
    inactivity_days: int = 3
    staleness_days: int = 14
    pattern_min_occurrences: int = 3
    lifelog_window: int = 50
    dedup_window_hours: int = 6
    enabled: bool = True
    pending_events_path: str = "db/proactive-engine/pending-events.json"
    intent_queue_path: str = "db/proactive-engine/intent-queue.json"
    heartbeat_state_path: str = "db/proactive-engine/heartbeat-state.json"

    @classmethod
    def from_env(cls) -> "ProactiveSettings":
        """Read and validate settings from PROACTIVE_* environment variables."""
        enabled_raw = os.environ.get("PROACTIVE_ENABLED", "true").lower()
        enabled = enabled_raw not in ("false", "0", "no")

        def _int(key: str, default: int) -> int:
            raw = os.environ.get(key)
            if raw is None:
                return default
            try:
                val = int(raw)
            except (ValueError, TypeError):
                raise ValueError(
                    f"{key} must be an integer, got {raw!r}"
                )
            return val

        settings = cls(
            llm_model=os.environ.get("PROACTIVE_LLM_MODEL", cls.llm_model),
            inactivity_days=_int("PROACTIVE_INACTIVITY_DAYS", cls.inactivity_days),
            staleness_days=_int("PROACTIVE_STALENESS_DAYS", cls.staleness_days),
            pattern_min_occurrences=_int(
                "PROACTIVE_PATTERN_MIN_OCCURRENCES", cls.pattern_min_occurrences
            ),
            lifelog_window=_int("PROACTIVE_LIFELOG_WINDOW", cls.lifelog_window),
            dedup_window_hours=_int("PROACTIVE_DEDUP_WINDOW_HOURS", cls.dedup_window_hours),
            enabled=enabled,
            pending_events_path=os.environ.get(
                "PROACTIVE_PENDING_EVENTS_PATH", cls.pending_events_path
            ),
            intent_queue_path=os.environ.get(
                "PROACTIVE_INTENT_QUEUE_PATH", cls.intent_queue_path
            ),
            heartbeat_state_path=os.environ.get(
                "PROACTIVE_HEARTBEAT_STATE_PATH", cls.heartbeat_state_path
            ),
        )
        settings.validate()
        return settings

    def validate(self) -> None:
        """Validate all numeric fields. Raises ValueError with field name and valid range."""
        numeric_fields = {
            "inactivity_days": (1, None),
            "staleness_days": (1, None),
            "pattern_min_occurrences": (1, None),
            "lifelog_window": (1, None),
            "dedup_window_hours": (1, None),
        }
        for field, (min_val, max_val) in numeric_fields.items():
            val = getattr(self, field)
            if not isinstance(val, int) or val <= 0:
                range_desc = f">= {min_val}" if max_val is None else f"{min_val}..{max_val}"
                raise ValueError(
                    f"ProactiveSettings.{field} must be a positive integer ({range_desc}), got {val!r}"
                )
