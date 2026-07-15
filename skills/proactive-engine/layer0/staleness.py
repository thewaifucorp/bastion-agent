"""MemoryStalenessDetector — Layer 0 detector for stale memories."""

from __future__ import annotations

import logging
from datetime import datetime, timezone

from event_bus import EventBus
from models import DetectionEvent
from protocols import MemupalaceProtocol
from settings import ProactiveSettings

logger = logging.getLogger(__name__)

_IGNORED_WING = "proactive/intent"
_DEDUP_HOURS = 24


class MemoryStalenessDetector:
    def __init__(
        self,
        memupalace: MemupalaceProtocol | None,
        bus: EventBus,
        settings: ProactiveSettings,
    ) -> None:
        self._memupalace = memupalace
        self._bus = bus
        self._settings = settings

    async def run(self) -> None:
        """
        If memupalace is None: log warning, return without emitting.
        Fetch stale memories (last_reinforced_at < now - staleness_days).
        Ignore 'proactive/intent' wing.
        Group by wing, emit at most 1 DetectionEvent per wing per 24h.
        """
        if self._memupalace is None:
            logger.warning("MemoryStalenessDetector: memupalace unavailable — skipping")
            return

        now = datetime.now(tz=timezone.utc)

        try:
            stale = await self._memupalace.get_stale(
                before_days=self._settings.staleness_days,
                exclude_wing=_IGNORED_WING,
            )
        except Exception:
            logger.warning(
                "MemoryStalenessDetector: failed to fetch stale memories", exc_info=True
            )
            return

        # Group by wing
        by_wing: dict[str, list] = {}
        for memory in stale:
            wing = getattr(memory, "wing", None) or (
                memory.get("wing") if isinstance(memory, dict) else None
            )
            if wing is None or wing == _IGNORED_WING:
                continue
            by_wing.setdefault(wing, []).append(memory)

        from datetime import timedelta

        for wing, memories in by_wing.items():
            event = DetectionEvent(
                type="memory_staleness",
                persona="system",
                payload={
                    "wing": wing,
                    "stale_count": len(memories),
                    "staleness_days": self._settings.staleness_days,
                },
                timestamp=now,
            )
            # Override dedup window to 24h for staleness events
            from event_bus import EventBus as _EB  # already imported but used for clarity

            # Temporarily check 24h manually before emitting
            window = timedelta(hours=_DEDUP_HOURS)
            duplicate = any(
                not e.processed
                and e.type == "memory_staleness"
                and e.payload.get("wing") == wing
                and (now - e.timestamp) < window
                for e in self._bus._events
            )
            if not duplicate:
                self._bus._events.append(event)
                logger.info(
                    "MemoryStalenessDetector: emitted event for wing %r (%d stale)",
                    wing,
                    len(memories),
                )
