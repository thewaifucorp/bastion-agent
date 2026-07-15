"""TemporalPatternDetector — Layer 0 detector for temporal interaction patterns."""

from __future__ import annotations

import logging
from datetime import datetime, timezone

from event_bus import EventBus
from models import DetectionEvent
from protocols import LifeLogProtocol, PersonaConfig
from settings import ProactiveSettings

logger = logging.getLogger(__name__)

_MIN_RECORDS = 10
_MAX_EVENTS = 3


class TemporalPatternDetector:
    def __init__(
        self,
        life_log: LifeLogProtocol,
        bus: EventBus,
        settings: ProactiveSettings,
    ) -> None:
        self._life_log = life_log
        self._bus = bus
        self._settings = settings

    async def run(self, personas: list[PersonaConfig]) -> None:
        """
        Run SQL query to find temporal patterns (day_of_week, hour_bucket).
        If total records < 10: log and return without emitting.
        Emit at most 3 DetectionEvent[temporal_pattern], prioritizing highest count.
        """
        now = datetime.now(tz=timezone.utc)
        persona_slugs = [p.slug for p in personas if p.current_weight >= 0.1]

        if not persona_slugs:
            return

        try:
            rows = await self._life_log.query_temporal_patterns(
                personas=persona_slugs,
                min_occurrences=self._settings.pattern_min_occurrences,
            )
        except Exception:
            logger.warning("TemporalPatternDetector: query failed", exc_info=True)
            return

        total = sum(r.get("cnt", 0) for r in rows)
        if total < _MIN_RECORDS:
            logger.info(
                "TemporalPatternDetector: insufficient data (%d records) — skipping", total
            )
            return

        # Prioritize by highest count, emit at most _MAX_EVENTS
        rows_sorted = sorted(rows, key=lambda r: r.get("cnt", 0), reverse=True)
        emitted = 0

        for row in rows_sorted:
            if emitted >= _MAX_EVENTS:
                break
            if row.get("cnt", 0) < self._settings.pattern_min_occurrences:
                continue

            event = DetectionEvent(
                type="temporal_pattern",
                persona=row.get("persona", "system"),
                payload={
                    "day_of_week": row.get("day_of_week"),
                    "hour_bucket": row.get("hour_bucket"),
                    "cnt": row.get("cnt"),
                },
                timestamp=now,
            )
            if self._bus.emit(event):
                emitted += 1
                logger.info(
                    "TemporalPatternDetector: emitted pattern %s/%s (cnt=%d)",
                    row.get("day_of_week"),
                    row.get("hour_bucket"),
                    row.get("cnt"),
                )
