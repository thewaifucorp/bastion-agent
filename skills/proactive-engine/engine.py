"""ProactiveEngine — orchestrates the proactive-cycle."""

from __future__ import annotations

import json
import logging
import os
import tempfile
from datetime import datetime, timedelta, timezone
from pathlib import Path

from event_bus import EventBus
from layer0.cve import CVEDetector
from layer0.inactivity import InactivityDetector
from layer0.intent_tracker import IntentTracker
from layer0.staleness import MemoryStalenessDetector
from layer0.temporal import TemporalPatternDetector
from layer1.suggestion_generator import SuggestionGenerator
from layer1.weekly_synthesizer import WeeklySynthesizer
from models import DetectionEvent
from protocols import ClawHubClient, LifeLogProtocol, MemupalaceProtocol, PersonaConfig
from settings import ProactiveSettings

logger = logging.getLogger(__name__)


class ProactiveEngine:
    def __init__(
        self,
        settings: ProactiveSettings,
        life_log: LifeLogProtocol,
        memupalace: MemupalaceProtocol | None,
        clawhub: ClawHubClient,
        personas: list[PersonaConfig] | None = None,
        installed_skills: list[str] | None = None,
    ) -> None:
        self._settings = settings
        self._life_log = life_log
        self._memupalace = memupalace
        self._clawhub = clawhub
        self._personas = personas or []
        self._installed_skills = installed_skills or []
        self._bus = EventBus(settings)

    async def run_cycle(self) -> None:
        """
        Mandatory proactive-cycle sequence (each step in independent try/except):
        1. Layer 0: Inactivity, Staleness, Temporal, IntentTracker.flush_queue()
        2. SuggestionGenerator.run(bus.consume(), personas)
        3. EventBus.flush()
        4. Update heartbeat-state.json
        """
        # Step 1 — Layer 0 detectors
        try:
            detector = InactivityDetector(self._life_log, self._bus, self._settings)
            await detector.run(self._personas)
        except Exception:
            logger.warning("run_cycle: InactivityDetector failed", exc_info=True)

        try:
            staleness = MemoryStalenessDetector(self._memupalace, self._bus, self._settings)
            await staleness.run()
        except Exception:
            logger.warning("run_cycle: MemoryStalenessDetector failed", exc_info=True)

        try:
            temporal = TemporalPatternDetector(self._life_log, self._bus, self._settings)
            await temporal.run(self._personas)
        except Exception:
            logger.warning("run_cycle: TemporalPatternDetector failed", exc_info=True)

        try:
            intent_tracker = IntentTracker(self._memupalace, self._settings)
            await intent_tracker.flush_queue()
        except Exception:
            logger.warning("run_cycle: IntentTracker.flush_queue failed", exc_info=True)

        # Step 2 — SuggestionGenerator
        try:
            generator = SuggestionGenerator(self._life_log, self._memupalace, self._settings)
            events = self._bus.consume()
            await generator.run(events, self._personas)
        except Exception:
            logger.warning("run_cycle: SuggestionGenerator failed", exc_info=True)

        # Step 3 — Flush
        try:
            self._bus.flush()
        except Exception:
            logger.warning("run_cycle: EventBus.flush failed", exc_info=True)

        # Step 4 — Update heartbeat state
        try:
            self._update_heartbeat_state("proactive-cycle")
        except Exception:
            logger.warning("run_cycle: heartbeat state update failed", exc_info=True)

    async def run_cve_check(self) -> None:
        """CVEDetector.run() + immediate EventBus.flush()."""
        try:
            detector = CVEDetector(self._clawhub, self._bus, self._settings)
            await detector.run(self._installed_skills)
        except Exception:
            logger.warning("run_cve_check: CVEDetector failed", exc_info=True)

        try:
            self._bus.flush()
        except Exception:
            logger.warning("run_cve_check: EventBus.flush failed", exc_info=True)

        try:
            self._update_heartbeat_state("proactive-cve-check")
        except Exception:
            logger.warning("run_cve_check: heartbeat state update failed", exc_info=True)

    async def run_weekly(self) -> None:
        """WeeklySynthesizer.run() with events from the last 7 days."""
        cutoff = datetime.now(tz=timezone.utc) - timedelta(days=7)
        events_last_7 = [e for e in self._bus._events if e.timestamp >= cutoff]

        try:
            synthesizer = WeeklySynthesizer(self._life_log, self._memupalace, self._settings)
            await synthesizer.run(events_last_7)
        except Exception:
            logger.warning("run_weekly: WeeklySynthesizer failed", exc_info=True)

        try:
            self._update_heartbeat_state("proactive-weekly")
        except Exception:
            logger.warning("run_weekly: heartbeat state update failed", exc_info=True)

    def _update_heartbeat_state(self, task: str) -> None:
        path = Path(self._settings.heartbeat_state_path)
        path.parent.mkdir(parents=True, exist_ok=True)

        state: dict = {}
        if path.exists():
            try:
                with open(path) as f:
                    state = json.load(f)
            except Exception:
                state = {}

        state[task] = {"last_run": datetime.now(tz=timezone.utc).isoformat()}

        fd, tmp = tempfile.mkstemp(dir=path.parent, suffix=".tmp")
        try:
            with os.fdopen(fd, "w") as f:
                json.dump(state, f)
            os.replace(tmp, path)
        except Exception:
            try:
                os.unlink(tmp)
            except OSError:
                pass
            raise
