"""EventBus — in-memory event queue with deduplication and atomic persistence."""

from __future__ import annotations

import json
import logging
import os
import tempfile
from datetime import timedelta, timezone
from pathlib import Path

from models import DetectionEvent
from settings import ProactiveSettings

logger = logging.getLogger(__name__)


class EventBus:
    def __init__(self, settings: ProactiveSettings) -> None:
        self._settings = settings
        self._events: list[DetectionEvent] = []
        self.load()

    def emit(self, event: DetectionEvent) -> bool:
        """
        Add event to bus. Returns False (discarded) if a duplicate exists within
        dedup_window_hours for the same (type, persona). Returns True if accepted.
        """
        window = timedelta(hours=self._settings.dedup_window_hours)
        for existing in self._events:
            if (
                not existing.processed
                and existing.type == event.type
                and existing.persona == event.persona
                and (event.timestamp - existing.timestamp) < window
            ):
                logger.debug(
                    "EventBus: dedup drop %s/%s (within %dh window)",
                    event.type,
                    event.persona,
                    self._settings.dedup_window_hours,
                )
                return False
        self._events.append(event)
        return True

    def consume(self) -> list[DetectionEvent]:
        """
        Return all unprocessed events sorted by timestamp asc, marking all as
        processed=True atomically before returning.
        """
        unprocessed = [e for e in self._events if not e.processed]
        unprocessed.sort(key=lambda e: e.timestamp)
        for e in unprocessed:
            e.processed = True
        return unprocessed

    def flush(self) -> None:
        """Persist current state to pending-events.json atomically."""
        path = Path(self._settings.pending_events_path)
        path.parent.mkdir(parents=True, exist_ok=True)
        data = [e.model_dump(mode="json") for e in self._events]
        fd, tmp_path = tempfile.mkstemp(dir=path.parent, suffix=".tmp")
        try:
            with os.fdopen(fd, "w") as f:
                json.dump(data, f)
            os.replace(tmp_path, path)
        except Exception:
            try:
                os.unlink(tmp_path)
            except OSError:
                pass
            raise

    def load(self) -> None:
        """Load events from pending-events.json on startup."""
        path = Path(self._settings.pending_events_path)
        if not path.exists():
            return
        try:
            with open(path) as f:
                data = json.load(f)
            self._events = [DetectionEvent.model_validate(item) for item in data]
            logger.debug("EventBus: loaded %d events from %s", len(self._events), path)
        except Exception:
            logger.warning(
                "EventBus: failed to load %s — starting with empty bus", path, exc_info=True
            )
            self._events = []
