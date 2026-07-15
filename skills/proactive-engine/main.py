"""CLI entrypoint for bastion/proactive-engine.

Invoked by HEARTBEAT via:
    exec python3 skills/proactive-engine/main.py <command> [options]
"""

from __future__ import annotations

import argparse
import asyncio
import json
import logging
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

from factory import create_engine
from protocols import PersonaConfig
from settings import ProactiveSettings

logging.basicConfig(level=logging.INFO, format="%(levelname)s %(name)s: %(message)s")
logger = logging.getLogger(__name__)


def _personas(raw: str) -> list[PersonaConfig]:
    return [PersonaConfig(slug=s, current_weight=1.0) for s in json.loads(raw)]


def _skills(raw: str) -> list[str]:
    return json.loads(raw)


async def _run(args: argparse.Namespace) -> None:
    settings = ProactiveSettings.from_env()

    if not settings.enabled:
        logger.info("PROACTIVE_ENABLED=false — skipping")
        return

    personas = _personas(args.personas) if getattr(args, "personas", None) else []
    installed_skills = _skills(args.skills) if getattr(args, "skills", None) else []

    engine = create_engine(
        settings=settings,
        personas=personas,
        installed_skills=installed_skills,
    )

    if args.command == "run-cycle":
        await engine.run_cycle()
    elif args.command == "run-cve-check":
        await engine.run_cve_check()
    elif args.command == "run-weekly":
        await engine.run_weekly()


def main() -> None:
    parser = argparse.ArgumentParser(prog="proactive-engine")
    sub = parser.add_subparsers(dest="command", required=True)

    p_cycle = sub.add_parser("run-cycle", help="Run the proactive detection and suggestion cycle")
    p_cycle.add_argument("--personas", default="[]", help="JSON array of persona slugs")
    p_cycle.add_argument("--skills", default="[]", help="JSON array of installed skill names")

    p_cve = sub.add_parser("run-cve-check", help="Check CVEs for all installed skills")
    p_cve.add_argument("--skills", default="[]", help="JSON array of installed skill names")

    p_weekly = sub.add_parser("run-weekly", help="Run the weekly synthesis cycle")
    p_weekly.add_argument("--personas", default="[]", help="JSON array of persona slugs")

    args = parser.parse_args()
    asyncio.run(_run(args))


if __name__ == "__main__":
    main()
