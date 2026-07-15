"""InactivityDetector — Layer 0 detector for inactive personas."""

from __future__ import annotations

import logging
from datetime import timezone

from event_bus import EventBus
from models import DetectionEvent
from protocols import LifeLogProtocol, PersonaConfig
from settings import ProactiveSettings

logger = logging.getLogger(__name__)


class InactivityDetector:
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
        For each persona with current_weight >= 0.1:
        - Query life-log summary for inactivity_days
        - If inactive: emit DetectionEvent[inactivity]
        Personas with weight < 0.1 are silently ignored.
        """
        from datetime import datetime

        now = datetime.now(tz=timezone.utc)

        for persona in personas:
            if persona.current_weight < 0.1:
                continue

            try:
                summary = await self._life_log.get_persona_summary(
                    persona.slug, days=self._settings.inactivity_days
                )
            except Exception:
                logger.warning(
                    "InactivityDetector: failed to query life-log for %r",
                    persona.slug,
                    exc_info=True,
                )
                continue

            last_interaction = summary.get("last_interaction")
            records = summary.get("records", [])

            if records:
                # Has recent interactions — not inactive
                continue

            event = DetectionEvent(
                type="inactivity",
                persona=persona.slug,
                payload={
                    "days_inactive": self._settings.inactivity_days,
                    "last_interaction": (
                        last_interaction.isoformat() if last_interaction else None
                    ),
                },
                timestamp=now,
            )
            accepted = self._bus.emit(event)
            if accepted:
                logger.info(
                    "InactivityDetector: emitted event for persona %r", persona.slug
                )
