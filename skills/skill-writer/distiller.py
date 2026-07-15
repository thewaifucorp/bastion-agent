"""Loop-closed distiller — detect task candidates and enqueue async distillation (D-04/D-05/SKWR-06).

Candidate detection is zero-LLM (heuristic only).
Actual distillation (LLM call via gateway) runs async from the approval queue.
"""
from __future__ import annotations

import json
import logging
import os
from datetime import UTC, datetime
from pathlib import Path

logger = logging.getLogger(__name__)

# Calibrate with real usage (A1 from RESEARCH assumptions)
MIN_STEPS = 4

# Persistent queue — processed by approval flow (D-04)
_PENDING_FILE_ENV = os.getenv("SKILL_WRITER_PENDING_FILE", "/data/pending_distillations.jsonl")
PENDING_FILE = Path(_PENDING_FILE_ENV)


def is_distillation_candidate(
    tool_calls: list[str],
) -> tuple[bool, str]:
    """Zero-LLM heuristic: is this task worth distilling into a reusable skill? (D-05).

    Gates on step count alone (CR-03 fix): the memupalace similarity requirement
    was driven by _no_search stub that always returned [] — making this function
    always return False. Step count is the reliable, dependency-free gate.

    Returns:
        (is_candidate, reason) — mirrors should_promote() pattern in promotion.py
    """
    if len(tool_calls) < MIN_STEPS:
        return False, f"Too few steps: {len(tool_calls)} < {MIN_STEPS}"

    return True, f"Distillation candidate: {len(tool_calls)} steps >= {MIN_STEPS} threshold"


def enqueue_pending(prompt: str, context_tier: str) -> None:
    """Append a pending distillation to the JSONL queue.

    Queue is processed by the approval flow (D-04) — never auto-applied.
    Follows log_skill_event() pattern from skill_writer.py.
    """
    entry = json.dumps(
        {
            "timestamp": datetime.now(UTC).isoformat(),
            "prompt": prompt,
            "privacy_tier": context_tier,
            "status": "pending",
        },
        ensure_ascii=False,
    )
    try:
        PENDING_FILE.parent.mkdir(parents=True, exist_ok=True)
        with PENDING_FILE.open("a", encoding="utf-8") as f:
            f.write(entry + "\n")
        logger.debug("distiller: enqueued pending distillation (tier=%s)", context_tier)
    except Exception as e:
        logger.error("distiller: failed to enqueue pending distillation: %s", e)
