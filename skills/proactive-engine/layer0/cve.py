"""CVEDetector — Layer 0 detector for CVE alerts on installed skills."""

from __future__ import annotations

import logging
from datetime import datetime, timedelta, timezone

from event_bus import EventBus
from models import DetectionEvent
from protocols import ClawHubClient
from settings import ProactiveSettings

logger = logging.getLogger(__name__)

_DEDUP_HOURS = 24


class CVEDetector:
    def __init__(
        self,
        clawhub: ClawHubClient,
        bus: EventBus,
        settings: ProactiveSettings,
    ) -> None:
        self._clawhub = clawhub
        self._bus = bus
        self._settings = settings

    async def run(self, installed_skills: list[str]) -> None:
        """
        Check CVEs for each installed skill via ClawHub API.
        Consolidates all CVEs into a single DetectionEvent[cve] per cycle.
        Uses 24h dedup window (overrides the default 6h).
        If ClawHub unavailable: log warning, no event emitted.
        """
        if not installed_skills:
            return

        now = datetime.now(tz=timezone.utc)

        try:
            batch_cves = await self._clawhub.get_batch_cves(installed_skills)
        except Exception:
            logger.warning("CVEDetector: ClawHub unavailable — skipping CVE check", exc_info=True)
            return

        all_cves: list[dict] = []
        for skill_name, cves in batch_cves.items():
            for cve in cves:
                all_cves.append({"skill": skill_name, **cve})

        if not all_cves:
            return

        # Check 24h dedup manually before emitting
        window = timedelta(hours=_DEDUP_HOURS)
        duplicate = any(
            not e.processed
            and e.type == "cve"
            and (now - e.timestamp) < window
            for e in self._bus._events
        )
        if duplicate:
            logger.debug("CVEDetector: CVE event deduplicated (within 24h window)")
            return

        event = DetectionEvent(
            type="cve",
            persona="system",
            payload={"cves": all_cves, "skill_count": len(batch_cves)},
            timestamp=now,
        )
        self._bus._events.append(event)
        logger.warning(
            "CVEDetector: emitted CVE event with %d CVEs across %d skills",
            len(all_cves),
            len(batch_cves),
        )
