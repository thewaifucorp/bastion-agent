"""WeeklySynthesizer — Layer 1 generator for weekly summaries."""

from __future__ import annotations

import json
import logging
import os
from datetime import datetime, timezone

import httpx

from models import DetectionEvent
from protocols import LifeLogProtocol, MemupalaceProtocol
from settings import ProactiveSettings

logger = logging.getLogger(__name__)

_OPENROUTER_URL = "https://openrouter.ai/api/v1/chat/completions"
_MAX_NEXT_ACTIONS = 3


class WeeklySynthesizer:
    def __init__(
        self,
        life_log: LifeLogProtocol,
        memupalace: MemupalaceProtocol | None,
        settings: ProactiveSettings,
    ) -> None:
        self._life_log = life_log
        self._memupalace = memupalace
        self._settings = settings

    async def run(self, events_last_7_days: list[DetectionEvent]) -> str:
        """
        If no events in last 7 days: return minimal summary without LLM call.
        Otherwise: synthesize via LLM.
        Persist to memupalace (wing=proactive/weekly).
        Returns summary text.
        Includes 'next actions' section with at most 3 items.
        """
        now = datetime.now(tz=timezone.utc)

        if not events_last_7_days:
            summary = (
                "Weekly summary: No proactive events detected in the last 7 days. "
                "All systems nominal.\n\n"
                "## Next suggested actions\n"
                "- Continue current routines.\n"
            )
            await self._persist(summary, now)
            return summary

        summary = await self._synthesize_with_llm(events_last_7_days, now)
        await self._persist(summary, now)
        return summary

    async def _synthesize_with_llm(
        self, events: list[DetectionEvent], now: datetime
    ) -> str:
        event_data = [
            {"type": e.type, "persona": e.persona, "payload": e.payload, "timestamp": e.timestamp.isoformat()}
            for e in events
        ]
        prompt = json.dumps({"events_last_7_days": event_data}, default=str)

        api_key = os.environ.get("OPENROUTER_API_KEY", "")
        try:
            async with httpx.AsyncClient(timeout=30) as client:
                resp = await client.post(
                    _OPENROUTER_URL,
                    headers={"Authorization": f"Bearer {api_key}"},
                    json={
                        "model": self._settings.llm_model,
                        "messages": [
                            {
                                "role": "system",
                                "content": (
                                    "You are a weekly summarizer. Based on the proactive events "
                                    "from the last 7 days, produce a concise Markdown summary. "
                                    f"Include a '## Next suggested actions' section with at most "
                                    f"{_MAX_NEXT_ACTIONS} bullet points."
                                ),
                            },
                            {"role": "user", "content": prompt},
                        ],
                    },
                )
                resp.raise_for_status()
                return resp.json()["choices"][0]["message"]["content"]
        except Exception:
            logger.warning("WeeklySynthesizer: LLM call failed — using fallback summary", exc_info=True)
            lines = ["# Weekly Summary (fallback)\n"]
            from collections import Counter
            counts = Counter(e.type for e in events)
            for etype, cnt in counts.items():
                lines.append(f"- {etype}: {cnt} event(s)")
            lines.append("\n## Next suggested actions")
            for i, e in enumerate(events[:_MAX_NEXT_ACTIONS], 1):
                lines.append(f"- Review {e.type} alert for {e.persona}.")
            return "\n".join(lines)

    async def _persist(self, summary: str, now: datetime) -> None:
        if self._memupalace is None:
            return
        try:
            await self._memupalace.add(
                content=summary,
                wing="proactive/weekly",
                hall="summaries",
                room=now.strftime("%Y-%m-%d"),
            )
        except Exception:
            logger.warning("WeeklySynthesizer: failed to persist summary", exc_info=True)
