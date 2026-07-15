"""SuggestionGenerator — Layer 1 generator that produces proactive suggestions via LLM."""

from __future__ import annotations

import json
import logging
import os
from datetime import datetime, timezone

import httpx

from models import DetectionEvent, EventType, ProactiveSuggestion
from protocols import InteractionRecord, LifeLogProtocol, MemupalaceProtocol, PersonaConfig
from settings import ProactiveSettings

logger = logging.getLogger(__name__)

_OPENROUTER_URL = "https://openrouter.ai/api/v1/chat/completions"


class SuggestionGenerator:
    def __init__(
        self,
        life_log: LifeLogProtocol,
        memupalace: MemupalaceProtocol | None,
        settings: ProactiveSettings,
    ) -> None:
        self._life_log = life_log
        self._memupalace = memupalace
        self._settings = settings
        self._last_cycle_record_count: int = 0

    async def run(
        self,
        events: list[DetectionEvent],
        personas: list[PersonaConfig],
    ) -> list[ProactiveSuggestion]:
        """
        If no events AND no new life-log records since last cycle: skip LLM call.
        Collect last lifelog_window records per active persona (weight >= 0.1).
        Build prompt (privacy-safe: no verbatim user content).
        Single LLM call. On failure: use _fallback_templates(events).
        Persist each suggestion to memupalace.
        """
        now = datetime.now(tz=timezone.utc)
        active_personas = [p for p in personas if p.current_weight >= 0.1]

        # Collect log records
        log_records: list[InteractionRecord] = []
        for persona in active_personas:
            try:
                summary = await self._life_log.get_persona_summary(
                    persona.slug, days=self._settings.lifelog_window
                )
                for rec in summary.get("records", []):
                    log_records.append(
                        InteractionRecord(
                            intent=rec.get("intent", ""),
                            tools=rec.get("tools", []),
                            timestamp=rec.get("timestamp", now),
                        )
                    )
            except Exception:
                logger.warning(
                    "SuggestionGenerator: failed to fetch records for %r", persona.slug, exc_info=True
                )

        new_records = len(log_records)

        # Skip if no events and no new records
        if not events and new_records == self._last_cycle_record_count:
            logger.info("SuggestionGenerator: no events and no new records — skipping LLM call")
            self._last_cycle_record_count = new_records
            return []

        self._last_cycle_record_count = new_records

        prompt = self._build_prompt(events, log_records, {}, "en")

        suggestions: list[ProactiveSuggestion] = []
        try:
            suggestions = await self._call_llm(prompt, events, now)
        except Exception:
            logger.warning("SuggestionGenerator: LLM call failed — using fallback", exc_info=True)
            suggestions = self._fallback_templates(events)

        # Persist to memupalace
        if self._memupalace is not None:
            for s in suggestions:
                try:
                    await self._memupalace.add(
                        content=s.text,
                        wing="proactive/suggestions",
                        hall=s.persona,
                        room=now.strftime("%Y-%m-%d"),
                        metadata={"suggestion_id": s.id, "event_type": s.event_type},
                    )
                except Exception:
                    logger.warning("SuggestionGenerator: failed to persist suggestion", exc_info=True)

        return suggestions

    def _build_prompt(
        self,
        events: list[DetectionEvent],
        log_records: list[InteractionRecord],
        persona_souls: dict[str, str],
        language: str,
    ) -> str:
        """
        Build prompt with only non-sensitive fields from logs.
        Never includes verbatim user message content.
        """
        safe_logs = [
            {
                "intent": rec.intent,
                "tools": rec.tools,
                "timestamp": rec.timestamp.isoformat(),
                "day_of_week": rec.timestamp.strftime("%A"),
                "hour_of_day": rec.timestamp.hour,
            }
            for rec in log_records
        ]

        consolidated: dict[str, list] = {}
        for event in events:
            consolidated.setdefault(event.type, []).append(event.payload)

        return json.dumps(
            {
                "language": language,
                "events": consolidated,
                "recent_activity": safe_logs[-50:],  # limit context size
            },
            ensure_ascii=False,
            default=str,
        )

    def _fallback_templates(self, events: list[DetectionEvent]) -> list[ProactiveSuggestion]:
        """Generate plain-text suggestions by event type when LLM fails."""
        now = datetime.now(tz=timezone.utc)
        templates: dict[str, str] = {
            "inactivity": "It's been a while since you engaged with {persona}. Consider checking in.",
            "memory_staleness": "Some memories in wing '{wing}' haven't been revisited recently.",
            "cve": "Security alert: CVEs detected in installed skills. Review and update.",
            "temporal_pattern": "You tend to be active on {day_of_week} around hour {hour_bucket}.",
        }

        suggestions = []
        for event in events:
            template = templates.get(event.type, f"Proactive alert: {event.type}")
            text = template.format(
                persona=event.persona,
                wing=event.payload.get("wing", ""),
                day_of_week=event.payload.get("day_of_week", ""),
                hour_bucket=event.payload.get("hour_bucket", ""),
            )
            suggestions.append(
                ProactiveSuggestion(
                    event_id=event.id,
                    text=text,
                    event_type=event.type,
                    persona=event.persona,
                    timestamp=now,
                    model_used="fallback",
                    is_fallback=True,
                )
            )
        return suggestions

    async def _call_llm(
        self,
        prompt: str,
        events: list[DetectionEvent],
        now: datetime,
    ) -> list[ProactiveSuggestion]:
        api_key = os.environ.get("OPENROUTER_API_KEY", "")
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
                                "You are a proactive AI assistant. Based on the provided events "
                                "and recent activity, generate concise, actionable suggestions. "
                                "Respond with a JSON array of objects with fields: "
                                '{"persona": str, "text": str, "event_type": str|null}'
                            ),
                        },
                        {"role": "user", "content": prompt},
                    ],
                },
            )
            resp.raise_for_status()
            data = resp.json()
            content = data["choices"][0]["message"]["content"]
            raw = json.loads(content)

        suggestions = []
        event_map = {e.id: e for e in events}
        for item in raw if isinstance(raw, list) else []:
            suggestions.append(
                ProactiveSuggestion(
                    event_id=None,
                    text=item.get("text", ""),
                    event_type=item.get("event_type"),
                    persona=item.get("persona", "system"),
                    timestamp=now,
                    model_used=self._settings.llm_model,
                    is_fallback=False,
                )
            )
        return suggestions
