"""
Self-Improving — persona-aware pattern promotion and conflict resolution.

Fork of ivangdavila/self-improving adapted for Bastion personas.

Implements:
- MemoryTier: HOT / WARM / COLD tiers (tiered memory)
- Pattern: a learned behaviour pattern with metadata
- PromotionPersistenceProtocol: hexagonal port for pattern I/O
- FileSystemAdapter: concrete default adapter (reads/writes markdown files)
- should_promote(): decides whether a pattern qualifies for HOT promotion
- promote_pattern(): promotes a pattern and records the event
- decay_pattern(): demotes a pattern and records the event
- conflict_resolution(): resolves two conflicting patterns by precedence rules

Promotion rules (Requirements 12.1, 12.2, 12.3):
  - Pattern observed 3+ times in 7 days → promote to HOT
  - Persona with current_weight < 0.3 → never promote to HOT global
  - In crisis: patterns of the crisis persona take priority over all others

Conflict resolution order (Requirement 12.4):
  more specific > more recent > higher persona weight

Namespace isolation (Requirement 12.6):
  All file operations are scoped to personas/{slug}/ — never cross-persona.

History recording (Requirement 12.5):
  Every promotion/decay is appended to personas/{slug}/weight-history.md
  with ISO 8601 timestamp and human-readable justification.
"""

from __future__ import annotations

import concurrent.futures
import logging
from dataclasses import dataclass, field
from datetime import UTC, datetime, timedelta
from enum import StrEnum
from pathlib import Path
from typing import Protocol, runtime_checkable

logger = logging.getLogger(__name__)

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

PROMOTION_MIN_OCCURRENCES: int = 3
PROMOTION_WINDOW_DAYS: int = 7
MIN_WEIGHT_FOR_GLOBAL_HOT: float = 0.3


# ---------------------------------------------------------------------------
# Domain models
# ---------------------------------------------------------------------------


class MemoryTier(StrEnum):
    """Tiered memory levels — mirrors the original ivangdavila/self-improving."""

    HOT = "HOT"
    WARM = "WARM"
    COLD = "COLD"


@dataclass
class Pattern:
    """A learned behaviour pattern associated with a persona."""

    id: str
    persona_slug: str
    description: str
    tier: MemoryTier
    specificity: int  # higher = more specific (e.g. number of conditions)
    persona_weight: float  # current_weight of the owning persona at record time
    occurrences: list[datetime] = field(default_factory=list)
    created_at: datetime = field(default_factory=lambda: datetime.now(tz=UTC))
    updated_at: datetime = field(default_factory=lambda: datetime.now(tz=UTC))


# ---------------------------------------------------------------------------
# Persistence protocol (hexagonal port)
# ---------------------------------------------------------------------------


@runtime_checkable
class PromotionPersistenceProtocol(Protocol):
    """Port for reading and writing patterns and promotion history."""

    def get_pattern(self, persona_slug: str, pattern_id: str) -> Pattern | None:
        """Return the Pattern for *pattern_id* in *persona_slug*, or None."""
        ...

    def save_pattern(self, pattern: Pattern) -> None:
        """Persist *pattern* under personas/{slug}/memory.md (HOT tier)."""
        ...

    def get_current_weight(self, persona_slug: str) -> float:
        """Return the current_weight for *persona_slug*."""
        ...

    def append_promotion_history(
        self,
        persona_slug: str,
        timestamp: datetime,
        pattern_id: str,
        action: str,
        justification: str,
    ) -> None:
        """Append an entry to personas/{slug}/weight-history.md."""
        ...


# ---------------------------------------------------------------------------
# Concrete adapter — FileSystem (default)
# ---------------------------------------------------------------------------


class FileSystemAdapter:
    """
    Concrete implementation of PromotionPersistenceProtocol.

    Stores HOT patterns in personas/{slug}/memory.md (≤100 lines, always loaded).
    Appends promotion/decay history to personas/{slug}/weight-history.md.
    Reads current_weight from USER.md frontmatter.

    Namespace isolation is enforced by construction: every path is derived
    from self._personas_dir / persona_slug — never from another slug.
    """

    def __init__(self, personas_dir: Path, user_md_path: Path) -> None:
        self._personas_dir = personas_dir
        self._user_md = user_md_path
        self._executor = concurrent.futures.ThreadPoolExecutor(max_workers=1)

    # ------------------------------------------------------------------
    # PromotionPersistenceProtocol implementation
    # ------------------------------------------------------------------

    def get_pattern(self, persona_slug: str, pattern_id: str) -> Pattern | None:
        """
        Look up a pattern by ID in personas/{slug}/memory.md.

        Returns None if the file or pattern does not exist.
        Namespace isolation: only reads from the given slug's directory.
        """
        memory_path = self._persona_path(persona_slug) / "memory.md"
        if not memory_path.exists():
            return None

        content = memory_path.read_text(encoding="utf-8")
        return self._parse_pattern_from_memory(content, persona_slug, pattern_id)

    def save_pattern(self, pattern: Pattern) -> None:
        """
        Persist *pattern* to personas/{slug}/memory.md.

        Namespace isolation: path is derived exclusively from pattern.persona_slug.
        Creates the directory and file if they do not exist.
        """
        slug = pattern.persona_slug
        persona_dir = self._persona_path(slug)
        persona_dir.mkdir(parents=True, exist_ok=True)

        memory_path = persona_dir / "memory.md"
        entry = self._format_pattern_entry(pattern)

        def _write_save_pattern():
            try:
                if not memory_path.exists():
                    memory_path.write_text(f"# HOT Memory — {slug}\n\n{entry}", encoding="utf-8")
                else:
                    content = memory_path.read_text(encoding="utf-8")
                    # Replace existing entry or append
                    if f"<!-- pattern:{pattern.id} -->" in content:
                        content = self._replace_pattern_entry(content, pattern.id, entry)
                        memory_path.write_text(content, encoding="utf-8")
                    else:
                        with memory_path.open("a", encoding="utf-8") as fh:
                            fh.write(entry)
                logger.debug("Pattern saved: slug=%s id=%s tier=%s", slug, pattern.id, pattern.tier)
            except Exception as e:
                logger.error("Failed to save pattern (slug=%s id=%s): %s", slug, pattern.id, e)

        self._executor.submit(_write_save_pattern)

    def get_current_weight(self, persona_slug: str) -> float:
        """
        Parse USER.md and return current_weight for *persona_slug*.

        Returns 0.0 if the persona is not found (safe default — prevents
        accidental HOT global promotion for unknown personas).
        """
        if not self._user_md.exists():
            logger.warning("USER.md not found at %s; defaulting weight to 0.0", self._user_md)
            return 0.0

        content = self._user_md.read_text(encoding="utf-8")
        weight = self._parse_weight(content, persona_slug)
        if weight is None:
            logger.warning(
                "Persona '%s' not found in USER.md; defaulting weight to 0.0", persona_slug
            )
            return 0.0
        return weight

    def append_promotion_history(
        self,
        persona_slug: str,
        timestamp: datetime,
        pattern_id: str,
        action: str,
        justification: str,
    ) -> None:
        """
        Append a promotion/decay record to personas/{slug}/weight-history.md.

        Namespace isolation: path is derived exclusively from persona_slug.
        Format: - {ISO timestamp} | {action} | pattern:{id} | {justification}
        """
        history_path = self._persona_path(persona_slug) / "weight-history.md"
        history_path.parent.mkdir(parents=True, exist_ok=True)

        iso_ts = timestamp.isoformat()
        line = f"- {iso_ts} | {action} | pattern:{pattern_id} | {justification}\n"

        def _write_append_history():
            try:
                if not history_path.exists():
                    history_path.write_text(f"# Weight History\n\n{line}", encoding="utf-8")
                else:
                    with history_path.open("a", encoding="utf-8") as fh:
                        fh.write(line)
                logger.debug(
                    "Promotion history appended: slug=%s action=%s pattern=%s",
                    persona_slug,
                    action,
                    pattern_id,
                )
            except Exception as e:
                logger.error(
                    "Failed to append promotion history (slug=%s pattern=%s): %s",
                    persona_slug,
                    pattern_id,
                    e,
                )

        self._executor.submit(_write_append_history)

    # ------------------------------------------------------------------
    # Internal helpers — all scoped to a single persona slug
    # ------------------------------------------------------------------

    def _persona_path(self, slug: str) -> Path:
        """Return the directory for *slug*. Never references another slug."""
        return self._personas_dir / slug

    @staticmethod
    def _parse_weight(content: str, slug: str) -> float | None:
        """Extract current_weight for *slug* from USER.md content."""
        import re

        slug_pattern = re.compile(r"slug:\s*[\"']?" + re.escape(slug) + r"[\"']?")
        weight_pattern = re.compile(r"current_weight:\s*([0-9]*\.?[0-9]+)")

        lines = content.splitlines()
        in_target = False

        for line in lines:
            if slug_pattern.search(line):
                in_target = True
                continue
            if in_target:
                if re.match(r"\s{0,4}-\s", line) and "slug:" not in line:
                    in_target = False
                    continue
                m = weight_pattern.search(line)
                if m:
                    return float(m.group(1))
        return None

    @staticmethod
    def _format_pattern_entry(pattern: Pattern) -> str:
        """Render a pattern as a markdown block with an HTML comment anchor."""
        occurrences_str = ", ".join(o.isoformat() for o in pattern.occurrences[-10:])
        return (
            f"\n<!-- pattern:{pattern.id} -->\n"
            f"## {pattern.id}\n\n"
            f"- **Description**: {pattern.description}\n"
            f"- **Tier**: {pattern.tier.value}\n"
            f"- **Specificity**: {pattern.specificity}\n"
            f"- **Persona weight**: {pattern.persona_weight:.4f}\n"
            f"- **Occurrences (last 10)**: {occurrences_str}\n"
            f"- **Updated**: {pattern.updated_at.isoformat()}\n"
            f"<!-- /pattern:{pattern.id} -->\n"
        )

    @staticmethod
    def _replace_pattern_entry(content: str, pattern_id: str, new_entry: str) -> str:
        """Replace an existing pattern block in memory.md content."""
        import re

        pattern = re.compile(
            r"\n<!-- pattern:"
            + re.escape(pattern_id)
            + r" -->.*?<!-- /pattern:"
            + re.escape(pattern_id)
            + r" -->\n",
            re.DOTALL,
        )
        return pattern.sub(new_entry, content)

    @staticmethod
    def _parse_pattern_from_memory(
        content: str, persona_slug: str, pattern_id: str
    ) -> Pattern | None:
        """Extract a Pattern from memory.md content by pattern_id."""
        import re

        block_pattern = re.compile(
            r"<!-- pattern:"
            + re.escape(pattern_id)
            + r" -->(.*?)<!-- /pattern:"
            + re.escape(pattern_id)
            + r" -->",
            re.DOTALL,
        )
        m = block_pattern.search(content)
        if not m:
            return None

        block = m.group(1)

        def _extract(field_name: str) -> str | None:
            fm = re.search(r"\*\*" + re.escape(field_name) + r"\*\*:\s*(.+)", block)
            return fm.group(1).strip() if fm else None

        description = _extract("Description") or ""
        tier_str = _extract("Tier") or "WARM"
        specificity_str = _extract("Specificity") or "0"
        weight_str = _extract("Persona weight") or "0.0"
        updated_str = _extract("Updated") or datetime.now(tz=UTC).isoformat()

        try:
            tier = MemoryTier(tier_str)
        except ValueError:
            tier = MemoryTier.WARM

        return Pattern(
            id=pattern_id,
            persona_slug=persona_slug,
            description=description,
            tier=tier,
            specificity=int(specificity_str),
            persona_weight=float(weight_str),
            updated_at=datetime.fromisoformat(updated_str),
        )


# ---------------------------------------------------------------------------
# Promotion logic
# ---------------------------------------------------------------------------


def should_promote(
    pattern: Pattern,
    current_weight: float,
    is_crisis: bool = False,
) -> tuple[bool, str]:
    """
    Decide whether *pattern* qualifies for HOT promotion.

    Rules (Requirements 12.1, 12.2, 12.3):
    1. Pattern must have ≥ 3 occurrences within the last 7 days.
    2. If current_weight < 0.3, do NOT promote to HOT global.
    3. In crisis mode, patterns of the crisis persona always qualify
       (crisis overrides the weight gate for the affected persona).

    Args:
        pattern: The pattern to evaluate.
        current_weight: The owning persona's current_weight.
        is_crisis: True if the owning persona is currently in crisis.

    Returns:
        (should_promote: bool, reason: str)
    """
    now = datetime.now(tz=UTC)
    window_start = now - timedelta(days=PROMOTION_WINDOW_DAYS)

    recent_occurrences = [o for o in pattern.occurrences if o >= window_start]
    count = len(recent_occurrences)

    if count < PROMOTION_MIN_OCCURRENCES:
        return False, (
            f"Insufficient occurrences: {count} in last {PROMOTION_WINDOW_DAYS} days "
            f"(need {PROMOTION_MIN_OCCURRENCES})"
        )

    # Crisis mode: bypass weight gate for the crisis persona (Requirement 12.3)
    if is_crisis:
        return True, (
            f"Crisis priority: {count} occurrences in last {PROMOTION_WINDOW_DAYS} days "
            f"(crisis override — weight gate bypassed)"
        )

    # Weight gate: persona with weight < 0.3 cannot promote to HOT global (Requirement 12.2)
    if current_weight < MIN_WEIGHT_FOR_GLOBAL_HOT:
        return False, (
            f"Weight gate: current_weight={current_weight:.4f} < {MIN_WEIGHT_FOR_GLOBAL_HOT} "
            f"(pattern not promoted to HOT global)"
        )

    return True, (
        f"Promotion criteria met: {count} occurrences in last {PROMOTION_WINDOW_DAYS} days, "
        f"current_weight={current_weight:.4f}"
    )


def promote_pattern(
    pattern: Pattern,
    persistence: PromotionPersistenceProtocol,
    is_crisis: bool = False,
) -> bool:
    """
    Attempt to promote *pattern* to HOT tier.

    Reads current_weight from persistence, evaluates promotion rules,
    and if eligible: updates tier to HOT, saves pattern, and records history.

    Namespace isolation: all writes are scoped to pattern.persona_slug.

    Args:
        pattern: The pattern to promote.
        persistence: Adapter implementing PromotionPersistenceProtocol.
        is_crisis: True if the owning persona is currently in crisis.

    Returns:
        True if the pattern was promoted, False otherwise.
    """
    current_weight = persistence.get_current_weight(pattern.persona_slug)
    eligible, reason = should_promote(pattern, current_weight, is_crisis)

    if not eligible:
        logger.debug(
            "Promotion skipped: slug=%s pattern=%s reason=%r",
            pattern.persona_slug,
            pattern.id,
            reason,
        )
        return False

    old_tier = pattern.tier
    pattern.tier = MemoryTier.HOT
    pattern.persona_weight = current_weight
    pattern.updated_at = datetime.now(tz=UTC)

    persistence.save_pattern(pattern)
    persistence.append_promotion_history(
        persona_slug=pattern.persona_slug,
        timestamp=pattern.updated_at,
        pattern_id=pattern.id,
        action=f"PROMOTED {old_tier.value} → HOT",
        justification=reason,
    )

    logger.info(
        "Pattern promoted to HOT: slug=%s pattern=%s reason=%r",
        pattern.persona_slug,
        pattern.id,
        reason,
    )
    return True


def decay_pattern(
    pattern: Pattern,
    target_tier: MemoryTier,
    justification: str,
    persistence: PromotionPersistenceProtocol,
) -> None:
    """
    Decay *pattern* to *target_tier* and record the event.

    Namespace isolation: all writes are scoped to pattern.persona_slug.

    Args:
        pattern: The pattern to decay.
        target_tier: The tier to decay to (WARM or COLD).
        justification: Human-readable reason for the decay.
        persistence: Adapter implementing PromotionPersistenceProtocol.
    """
    old_tier = pattern.tier
    pattern.tier = target_tier
    pattern.updated_at = datetime.now(tz=UTC)

    persistence.save_pattern(pattern)
    persistence.append_promotion_history(
        persona_slug=pattern.persona_slug,
        timestamp=pattern.updated_at,
        pattern_id=pattern.id,
        action=f"DECAYED {old_tier.value} → {target_tier.value}",
        justification=justification,
    )

    logger.info(
        "Pattern decayed: slug=%s pattern=%s %s → %s reason=%r",
        pattern.persona_slug,
        pattern.id,
        old_tier.value,
        target_tier.value,
        justification,
    )


# ---------------------------------------------------------------------------
# Conflict resolution
# ---------------------------------------------------------------------------


def conflict_resolution(pattern_a: Pattern, pattern_b: Pattern) -> Pattern:
    """
    Resolve a conflict between two patterns and return the winner.

    Precedence order (Requirement 12.4):
    1. More specific (higher specificity value)
    2. More recent (later updated_at)
    3. Higher persona weight (higher persona_weight)

    If all three criteria are tied, pattern_a wins (stable, deterministic).

    Args:
        pattern_a: First conflicting pattern.
        pattern_b: Second conflicting pattern.

    Returns:
        The winning Pattern.
    """
    # 1. More specific wins
    if pattern_a.specificity != pattern_b.specificity:
        winner = pattern_a if pattern_a.specificity > pattern_b.specificity else pattern_b
        logger.debug(
            "Conflict resolved by specificity: winner=%s (%d > %d)",
            winner.id,
            winner.specificity,
            (pattern_b if winner is pattern_a else pattern_a).specificity,
        )
        return winner

    # 2. More recent wins
    if pattern_a.updated_at != pattern_b.updated_at:
        winner = pattern_a if pattern_a.updated_at > pattern_b.updated_at else pattern_b
        logger.debug(
            "Conflict resolved by recency: winner=%s updated_at=%s",
            winner.id,
            winner.updated_at.isoformat(),
        )
        return winner

    # 3. Higher persona weight wins
    if pattern_a.persona_weight != pattern_b.persona_weight:
        winner = pattern_a if pattern_a.persona_weight > pattern_b.persona_weight else pattern_b
        logger.debug(
            "Conflict resolved by persona weight: winner=%s weight=%.4f",
            winner.id,
            winner.persona_weight,
        )
        return winner

    # Tie — pattern_a wins (stable)
    logger.debug("Conflict tie — defaulting to pattern_a: %s", pattern_a.id)
    return pattern_a


# ---------------------------------------------------------------------------
# CLI Interface for OpenClaw Agent
# ---------------------------------------------------------------------------
def main() -> None:
    import argparse

    parser = argparse.ArgumentParser(description="CLI wrapper generated by refactoring")
    parser.add_argument("--action", help="Action to perform")
    parser.add_argument("--args-json", default="{}", help="Arguments as JSON string")

    args = parser.parse_args()
    print("Execution of stub CLI for", __file__)
    print("Action:", args.action)
    print("Args:", args.args_json)


if __name__ == "__main__":
    main()
