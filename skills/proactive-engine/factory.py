"""
Factory for creating a ProactiveEngine with concrete adapters.

Bridges the proactive-engine's internal LifeLogProtocol with the actual
skills.life_log adapter (which has a different interface).
"""

from __future__ import annotations

import logging
import sys
from collections import defaultdict
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any

from engine import ProactiveEngine
from models import DetectionEvent
from protocols import ClawHubClient, InteractionRecord, LifeLogProtocol, MemupalaceProtocol, PersonaConfig
from settings import ProactiveSettings

logger = logging.getLogger(__name__)


# ---------------------------------------------------------------------------
# Adapter: wraps skills.life_log to satisfy proactive-engine's LifeLogProtocol
# ---------------------------------------------------------------------------


class _LifeLogAdapter:
    """
    Adapts the skills.life_log.LifeLogProtocol to the proactive-engine's
    LifeLogProtocol, which expects:
      - get_persona_summary(persona, days) -> dict with "records" and "last_interaction"
      - query_temporal_patterns(personas, min_occurrences) -> list of pattern rows
    """

    def __init__(self, real_adapter: Any) -> None:
        self._adapter = real_adapter

    async def get_persona_summary(self, persona: str, days: int) -> dict[str, Any]:
        records = await self._adapter.get_persona_summary(persona, days)
        last = None
        adapted = []
        for r in records:
            ts = r.timestamp if hasattr(r, "timestamp") else None
            if ts and (last is None or ts > last):
                last = ts
            adapted.append(
                InteractionRecord(
                    intent=getattr(r, "intent", ""),
                    tools=getattr(r, "tools", []),
                    timestamp=ts or datetime.now(tz=timezone.utc),
                )
            )
        return {"records": adapted, "last_interaction": last}

    async def query_temporal_patterns(
        self, personas: list[str], min_occurrences: int
    ) -> list[dict[str, Any]]:
        """
        Derives temporal patterns from life-log records by aggregating
        (persona, day_of_week, hour_bucket) counts.
        Falls back gracefully if the adapter raises.
        """
        counts: dict[tuple[str, str, int], int] = defaultdict(int)

        for persona in personas:
            try:
                records = await self._adapter.get_persona_summary(persona, days=90)
            except Exception:
                logger.warning("_LifeLogAdapter: failed to fetch records for %r", persona, exc_info=True)
                continue
            for r in records:
                ts = getattr(r, "timestamp", None)
                if ts is None:
                    continue
                if ts.tzinfo is None:
                    ts = ts.replace(tzinfo=timezone.utc)
                day = ts.strftime("%A")
                hour = ts.hour
                counts[(persona, day, hour)] += 1

        rows = [
            {"persona": persona, "day_of_week": day, "hour_bucket": hour, "cnt": cnt}
            for (persona, day, hour), cnt in counts.items()
            if cnt >= min_occurrences
        ]
        return sorted(rows, key=lambda r: r["cnt"], reverse=True)


# ---------------------------------------------------------------------------
# Adapter: wraps ClawHub HTTP client
# ---------------------------------------------------------------------------


class _HttpClawHubClient:
    """Simple HTTP adapter for ClawHub API."""

    def __init__(self, base_url: str, api_key: str) -> None:
        self._base_url = base_url.rstrip("/")
        self._api_key = api_key

    async def get_cves(self, skill_name: str) -> list[dict[str, str]]:
        import httpx

        try:
            async with httpx.AsyncClient(timeout=10) as client:
                resp = await client.get(
                    f"{self._base_url}/api/v1/skills/{skill_name}/cves",
                    headers={"Authorization": f"Bearer {self._api_key}"},
                )
                if resp.status_code == 404:
                    return []
                resp.raise_for_status()
                return resp.json().get("cves", [])
        except Exception:
            raise

    async def get_batch_cves(self, skill_names: list[str]) -> dict[str, list[dict[str, str]]]:
        import httpx

        try:
            async with httpx.AsyncClient(timeout=10) as client:
                resp = await client.post(
                    f"{self._base_url}/api/v1/skills/cves/batch",
                    headers={"Authorization": f"Bearer {self._api_key}"},
                    json={"skills": skill_names},
                )
                resp.raise_for_status()
                return resp.json().get("results", {})
        except Exception:
            raise


class _NullClawHubClient:
    """No-op client used when ClawHub is not configured."""

    async def get_cves(self, skill_name: str) -> list[dict[str, str]]:
        return []

    async def get_batch_cves(self, skill_names: list[str]) -> dict[str, list[dict[str, str]]]:
        return {}


# ---------------------------------------------------------------------------
# Public factory
# ---------------------------------------------------------------------------


def create_engine(
    settings: ProactiveSettings,
    life_log: LifeLogProtocol | None = None,
    clawhub: ClawHubClient | None = None,
    memupalace: MemupalaceProtocol | None = None,
    personas: list[PersonaConfig] | None = None,
    installed_skills: list[str] | None = None,
) -> ProactiveEngine:
    """
    Create a ProactiveEngine with concrete adapters wired from env vars.

    Parameters can be injected explicitly (for tests) or left as None to
    auto-create from environment (for production).
    """
    import os

    # --- Life-log adapter ---
    if life_log is None:
        try:
            _bastion = Path(__file__).parent.parent.parent
            if str(_bastion) not in sys.path:
                sys.path.insert(0, str(_bastion))
            from skills.life_log.factory import Settings as LLSettings, create_adapter

            ll_settings = LLSettings.from_env()
            real_adapter = create_adapter(ll_settings)
            life_log = _LifeLogAdapter(real_adapter)
            logger.info("create_engine: life-log adapter ready (%s)", ll_settings.DB_STRATEGY)
        except Exception:
            logger.error("create_engine: failed to create life-log adapter", exc_info=True)
            raise

    # --- Memupalace adapter ---
    if memupalace is None:
        try:
            from skills.memupalace.factory import MemupalaceSettings, create_memupalace

            mp_settings = MemupalaceSettings.from_env()
            memupalace = create_memupalace(mp_settings)
            logger.info("create_engine: memupalace adapter ready")
        except Exception:
            logger.warning("create_engine: memupalace unavailable — degraded mode", exc_info=True)
            memupalace = None

    # --- ClawHub client ---
    if clawhub is None:
        clawhub_url = os.environ.get("CLAWHUB_URL", "")
        clawhub_key = os.environ.get("CLAWHUB_API_KEY", "")
        if clawhub_url:
            clawhub = _HttpClawHubClient(clawhub_url, clawhub_key)
        else:
            logger.info("create_engine: CLAWHUB_URL not set — CVE checks disabled")
            clawhub = _NullClawHubClient()

    return ProactiveEngine(
        settings=settings,
        life_log=life_log,
        memupalace=memupalace,
        clawhub=clawhub,
        personas=personas or [],
        installed_skills=installed_skills or [],
    )
