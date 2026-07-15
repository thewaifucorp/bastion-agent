"""self-improving MCP server — observe usage, suggest skill improvements (SELF-01/SELF-02).

Transport: streamable-http, porta 8003 (ou SELF_IMPROVING_PORT env var).
SELF observes and suggests; SKWR executes (D-10). Nothing is applied without approval (D-11).
"""

from __future__ import annotations

import json
import logging
import os
import re
from datetime import UTC, datetime
from pathlib import Path

import httpx
from fastmcp import FastMCP

from promotion import (
    FileSystemAdapter,
    Pattern,
    should_promote,
)

logger = logging.getLogger(__name__)
mcp = FastMCP("self-improving")

MEMUPALACE_URL = os.getenv("MEMUPALACE_URL", "http://memupalace:8001/mcp")
SKILLS_DIR = Path(os.getenv("SKILLS_DIR", "/skills"))
SUGGESTIONS_FILE = Path(os.getenv("SELF_SUGGESTIONS_FILE", "/data/suggestions.jsonl"))


# ── helpers ──────────────────────────────────────────────────────────────────


def _validate_str(name: str, value: object) -> str:
    if not isinstance(value, str) or not str(value).strip():
        raise ValueError(f"Parameter '{name}' must be a non-empty, non-whitespace string.")
    return str(value)


_SEGMENT_RE = re.compile(r"^[a-z0-9][a-z0-9_-]{0,63}$")


def _safe_segment(value: str) -> str:
    """Allowlist single path segment (SEC: path traversal — WR-03 fix).

    Mirrors skill-writer._safe_segment. Lowercases, then requires
    ^[a-z0-9][a-z0-9_-]{0,63}$ — no '..', '/', '\\', dots-only, or empty segments.
    """
    seg = str(value).strip().lower()
    if not _SEGMENT_RE.match(seg):
        raise ValueError(f"invalid path segment: {value!r}")
    return seg


def _assert_inside_skills_dir(path: Path) -> Path:
    """Verify the path stays under SKILLS_DIR (fail-closed — WR-03 fix)."""
    base = SKILLS_DIR.resolve()
    resolved = path.resolve()
    if resolved != base and not resolved.is_relative_to(base):
        raise ValueError("path traversal detected")
    return path


def _get_adapter() -> FileSystemAdapter:
    """Build FileSystemAdapter for the skills volume."""
    personas_path = SKILLS_DIR / "personas"
    return FileSystemAdapter(
        personas_dir=personas_path,
        user_md_path=SKILLS_DIR / "USER.md",
    )


async def _add_to_memupalace(content: str, wing: str = "skill-usage") -> None:
    """Forward usage event to memupalace (SELF-02 feedback loop)."""
    try:
        async with httpx.AsyncClient() as client:
            await client.post(
                f"{MEMUPALACE_URL}/call-tool",
                json={
                    "name": "memory_add",
                    "arguments": {"content": content, "wing": wing},
                },
                timeout=10.0,
            )
    except Exception as e:
        logger.warning("self-improving: memupalace add failed: %s", e)


def _save_suggestion(suggestion: dict) -> None:
    """Persist suggestion to JSONL queue (D-12 demand-pull + proactive delivery)."""
    try:
        SUGGESTIONS_FILE.parent.mkdir(parents=True, exist_ok=True)
        entry = json.dumps(
            {**suggestion, "timestamp": datetime.now(UTC).isoformat()},
            ensure_ascii=False,
        )
        with SUGGESTIONS_FILE.open("a", encoding="utf-8") as f:
            f.write(entry + "\n")
    except Exception as e:
        logger.error("self-improving: failed to save suggestion: %s", e)


# ── tools ─────────────────────────────────────────────────────────────────────


@mcp.tool()
def suggest_promotion(pattern_id: str, persona_slug: str) -> dict:
    """Analyze a pattern and suggest promotion to HOT tier (D-10).

    SELF observes and suggests; SKWR executes.
    Returns status: 'pending_approval' invariably — never auto-applies (D-11).
    SEC (WR-03 fix): persona_slug and pattern_id are allowlist-sanitized before
    reaching the filesystem adapter.
    """
    _validate_str("pattern_id", pattern_id)
    _validate_str("persona_slug", persona_slug)

    # SEC: allowlist-sanitize before any filesystem path construction (WR-03 fix)
    safe_slug = _safe_segment(persona_slug)
    safe_pattern_id = _safe_segment(pattern_id)
    # Verify resolved path stays under SKILLS_DIR/personas (defense in depth)
    _assert_inside_skills_dir(SKILLS_DIR / "personas" / safe_slug)

    adapter = _get_adapter()
    pattern = adapter.get_pattern(safe_slug, safe_pattern_id)
    if pattern is None:
        return {
            "eligible": False,
            "reason": f"Pattern '{safe_pattern_id}' not found for persona '{safe_slug}'",
            "pattern_id": safe_pattern_id,
            "status": "not_found",
        }

    current_weight = adapter.get_current_weight(safe_slug)
    eligible, reason = should_promote(pattern, current_weight)

    suggestion = {
        "eligible": eligible,
        "reason": reason,
        "pattern_id": safe_pattern_id,
        "persona_slug": safe_slug,
        "status": "pending_approval",  # D-11 invariant — SELF never applies
    }

    if eligible:
        _save_suggestion(suggestion)
        logger.info(
            "self-improving: suggestion queued for pattern=%s persona=%s",
            safe_pattern_id,
            safe_slug,
        )

    return suggestion


@mcp.tool()
def list_pending_suggestions() -> list[dict]:
    """Return all pending improvement suggestions for proactive delivery (D-12).

    Called by the Rust proactive engine during heartbeat/idle to build a nudge.
    """
    if not SUGGESTIONS_FILE.exists():
        return []
    suggestions = []
    try:
        with SUGGESTIONS_FILE.open(encoding="utf-8") as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue
                try:
                    entry = json.loads(line)
                    if entry.get("status") == "pending_approval":
                        suggestions.append(entry)
                except json.JSONDecodeError:
                    continue
    except Exception as e:
        logger.error("self-improving: failed to read suggestions: %s", e)
    return suggestions


@mcp.tool()
async def observe_usage(
    skill_name: str,
    persona_slug: str,
    success: bool,
    context_summary: str = "",
) -> dict:
    """Record a skill usage event in memupalace (SELF-02 promote loop data feed).

    Feeds the promote loop: usage patterns accumulate in memupalace 'skill-usage' wing.
    suggest_promotion reads these patterns via promotion.py.
    context_summary is truncated to 200 chars before persisting (T-03-05-02).
    """
    _validate_str("skill_name", skill_name)
    _validate_str("persona_slug", persona_slug)

    content = (
        f"skill_usage: skill={skill_name} persona={persona_slug} "
        f"success={success} ts={datetime.now(UTC).isoformat()}"
    )
    if context_summary:
        content += f" summary={context_summary[:200]}"

    await _add_to_memupalace(content, wing="skill-usage")
    logger.debug("self-improving: observed usage skill=%s success=%s", skill_name, success)

    return {"observed": True, "skill_name": skill_name, "persona_slug": persona_slug}


if __name__ == "__main__":
    port = int(os.getenv("SELF_IMPROVING_PORT", "8003"))
    mcp.run(transport="streamable-http", host="0.0.0.0", port=port)
