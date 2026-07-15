"""IntentTracker — Layer 0 tracker that persists user intents to memupalace."""

from __future__ import annotations

import json
import logging
import os
import tempfile
from datetime import datetime, timezone
from pathlib import Path

from protocols import MemupalaceProtocol
from settings import ProactiveSettings

logger = logging.getLogger(__name__)


class IntentTracker:
    def __init__(
        self,
        memupalace: MemupalaceProtocol | None,
        settings: ProactiveSettings,
    ) -> None:
        self._memupalace = memupalace
        self._settings = settings
        self._queue_path = Path(settings.intent_queue_path)

    async def track(self, intent: str, persona: str, timestamp: datetime) -> None:
        """
        Persist intent to memupalace (wing=proactive/intent, hall=persona, room=day_of_week).
        If memupalace unavailable: enqueue to intent-queue.json.
        Operation is non-blocking — does not propagate exceptions.
        """
        if timestamp.tzinfo is None:
            timestamp = timestamp.replace(tzinfo=timezone.utc)

        metadata = {
            "persona": persona,
            "timestamp": timestamp.isoformat(),
            "day_of_week": timestamp.strftime("%A"),
            "hour_of_day": timestamp.hour,
        }

        if self._memupalace is not None:
            try:
                await self._memupalace.add(
                    content=intent,
                    wing="proactive/intent",
                    hall=persona,
                    room=timestamp.strftime("%A"),
                    metadata=metadata,
                )
                return
            except Exception:
                logger.warning(
                    "IntentTracker: memupalace unavailable — queuing intent", exc_info=True
                )

        # Enqueue locally
        self._enqueue({"intent": intent, "persona": persona, **metadata})

    async def flush_queue(self) -> None:
        """Attempt to persist queued intents to memupalace when it becomes available."""
        if self._memupalace is None:
            return
        if not self._queue_path.exists():
            return

        try:
            with open(self._queue_path) as f:
                queue: list[dict] = json.load(f)
        except Exception:
            logger.warning("IntentTracker: failed to read intent queue", exc_info=True)
            return

        remaining = []
        for item in queue:
            try:
                ts = datetime.fromisoformat(item["timestamp"])
                await self._memupalace.add(
                    content=item["intent"],
                    wing="proactive/intent",
                    hall=item["persona"],
                    room=item.get("day_of_week", "Unknown"),
                    metadata={k: v for k, v in item.items() if k != "intent"},
                )
            except Exception:
                logger.warning("IntentTracker: failed to flush item — keeping in queue")
                remaining.append(item)

        self._write_queue(remaining)

    def _enqueue(self, item: dict) -> None:
        self._queue_path.parent.mkdir(parents=True, exist_ok=True)
        queue: list[dict] = []
        if self._queue_path.exists():
            try:
                with open(self._queue_path) as f:
                    queue = json.load(f)
            except Exception:
                queue = []
        queue.append(item)
        self._write_queue(queue)

    def _write_queue(self, queue: list[dict]) -> None:
        self._queue_path.parent.mkdir(parents=True, exist_ok=True)
        fd, tmp = tempfile.mkstemp(dir=self._queue_path.parent, suffix=".tmp")
        try:
            with os.fdopen(fd, "w") as f:
                json.dump(queue, f)
            os.replace(tmp, self._queue_path)
        except Exception:
            try:
                os.unlink(tmp)
            except OSError:
                pass
            logger.warning("IntentTracker: failed to write intent queue", exc_info=True)
