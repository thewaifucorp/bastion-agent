"""
Weight System — priority calculation and dynamic weight management.

Implements:
- WeightHistoryEntry dataclass: timestamped record of a weight change
- WeightPersistenceProtocol: hexagonal port for weight I/O
- UserMdAdapter: concrete adapter that reads/writes USER.md and weight-history.md
- calculate_priority(): pure function implementing the priority formula
- adjust_weight(): adjusts current_weight, persists, and logs history

Priority formula (clamped to [0.0, 1.0]):
    priority = current_weight
             + 0.1  * (deep_work)
             + 0.2  * (deadline_hours <= 4)
             + 0.15 * (deadline_hours <= 12)
             + 0.1  * (deadline_hours <= 24)
             + 0.05 * (deadline_hours <= 48)
"""

from __future__ import annotations

import logging
import re
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Protocol, runtime_checkable

logger = logging.getLogger(__name__)

# ---------------------------------------------------------------------------
# Domain models
# ---------------------------------------------------------------------------


@dataclass
class WeightHistoryEntry:
    """A single record in a persona's weight-history.md."""

    timestamp: datetime
    old_weight: float
    new_weight: float
    justification: str


# ---------------------------------------------------------------------------
# Persistence protocol (hexagonal port)
# ---------------------------------------------------------------------------


@runtime_checkable
class WeightPersistenceProtocol(Protocol):
    """Port for reading and writing persona weights."""

    def get_current_weight(self, slug: str) -> float:
        """Return the current_weight for the given persona slug."""
        ...

    def set_current_weight(self, slug: str, weight: float) -> None:
        """Persist the new current_weight for the given persona slug."""
        ...

    def append_weight_history(self, slug: str, entry: WeightHistoryEntry) -> None:
        """Append a WeightHistoryEntry to personas/{slug}/weight-history.md."""
        ...


# ---------------------------------------------------------------------------
# Concrete adapter — reads/writes USER.md and weight-history.md
# ---------------------------------------------------------------------------


class UserMdAdapter:
    """
    Concrete implementation of WeightPersistenceProtocol.

    Reads current_weight from USER.md frontmatter (personas list) and
    appends history entries to personas/{slug}/weight-history.md.

    USER.md frontmatter structure (relevant excerpt):
        personas:
          - slug: "tech-lead"
            name: "Tech Lead"
            current_weight: 0.9
    """

    def __init__(self, user_md_path: Path, personas_dir: Path) -> None:
        self._user_md = user_md_path
        self._personas_dir = personas_dir

    # ------------------------------------------------------------------
    # WeightPersistenceProtocol implementation
    # ------------------------------------------------------------------

    def get_current_weight(self, slug: str) -> float:
        """Parse USER.md and return current_weight for *slug*."""
        content = self._user_md.read_text(encoding="utf-8")
        weight = self._parse_weight_from_user_md(content, slug)
        if weight is None:
            raise KeyError(f"Persona slug '{slug}' not found in USER.md")
        return weight

    def set_current_weight(self, slug: str, weight: float) -> None:
        """Update current_weight for *slug* in USER.md frontmatter."""
        content = self._user_md.read_text(encoding="utf-8")
        updated = self._update_weight_in_user_md(content, slug, weight)
        self._user_md.write_text(updated, encoding="utf-8")
        logger.info("USER.md updated: slug=%s current_weight=%.4f", slug, weight)

    def append_weight_history(self, slug: str, entry: WeightHistoryEntry) -> None:
        """Append a markdown entry to personas/{slug}/weight-history.md."""
        history_path = self._personas_dir / slug / "weight-history.md"
        history_path.parent.mkdir(parents=True, exist_ok=True)

        iso_ts = entry.timestamp.isoformat()
        line = (
            f"- {iso_ts} | "
            f"{entry.old_weight:.4f} → {entry.new_weight:.4f} | "
            f"{entry.justification}\n"
        )

        with history_path.open("a", encoding="utf-8") as fh:
            if fh.tell() == 0:
                fh.write("# Weight History\n\n")
            fh.write(line)

        logger.debug(
            "Weight history appended: slug=%s ts=%s", slug, iso_ts
        )

    # ------------------------------------------------------------------
    # Internal helpers
    # ------------------------------------------------------------------

    @staticmethod
    def _parse_weight_from_user_md(content: str, slug: str) -> float | None:
        """
        Extract current_weight for *slug* from USER.md content.

        Looks for a YAML block like:
            - slug: "tech-lead"
              ...
              current_weight: 0.9
        within the frontmatter (between the first pair of '---' delimiters).
        """
        frontmatter = UserMdAdapter._extract_frontmatter(content)
        if frontmatter is None:
            return None

        # Find the persona block for this slug
        # Pattern: slug line followed (within the same list item) by current_weight
        slug_pattern = re.compile(
            r"slug:\s*[\"']?" + re.escape(slug) + r"[\"']?",
        )
        weight_pattern = re.compile(r"current_weight:\s*([0-9]*\.?[0-9]+)")

        lines = frontmatter.splitlines()
        in_target_persona = False

        for line in lines:
            if slug_pattern.search(line):
                in_target_persona = True
                continue

            if in_target_persona:
                # A new list item (starts with "  - ") means we left the block
                if re.match(r"\s{0,4}-\s", line) and "slug:" not in line:
                    in_target_persona = False
                    continue

                m = weight_pattern.search(line)
                if m:
                    return float(m.group(1))

        return None

    @staticmethod
    def _update_weight_in_user_md(content: str, slug: str, weight: float) -> str:
        """
        Return *content* with current_weight updated for *slug*.

        If the persona block already has a current_weight line, replace it.
        If not, insert one after the slug line.
        """
        frontmatter = UserMdAdapter._extract_frontmatter(content)
        if frontmatter is None:
            raise ValueError("USER.md has no valid YAML frontmatter")

        slug_pattern = re.compile(
            r"(slug:\s*[\"']?" + re.escape(slug) + r"[\"']?)"
        )
        weight_line_pattern = re.compile(r"(\s+current_weight:\s*)[0-9]*\.?[0-9]+")

        lines = frontmatter.splitlines(keepends=True)
        in_target_persona = False
        weight_updated = False
        result_lines: list[str] = []

        for line in lines:
            if slug_pattern.search(line):
                in_target_persona = True
                result_lines.append(line)
                continue

            if in_target_persona and not weight_updated:
                # Leaving the block?
                if re.match(r"\s{0,4}-\s", line) and "slug:" not in line:
                    in_target_persona = False
                    result_lines.append(line)
                    continue

                m = weight_line_pattern.match(line)
                if m:
                    indent = m.group(1)
                    result_lines.append(f"{indent}{weight:.4f}\n")
                    weight_updated = True
                    continue

            result_lines.append(line)

        new_frontmatter = "".join(result_lines)

        # If weight line was not found, insert it after the slug line
        if not weight_updated:
            slug_re = re.compile(
                r"([ \t]*slug:\s*[\"']?" + re.escape(slug) + r"[\"']?[ \t]*\n)"
            )
            indent = "    "
            new_frontmatter = slug_re.sub(
                r"\g<1>" + f"{indent}current_weight: {weight:.4f}\n",
                new_frontmatter,
                count=1,
            )

        # Reconstruct full file: replace old frontmatter with updated one
        return content.replace(frontmatter, new_frontmatter, 1)

    @staticmethod
    def _extract_frontmatter(content: str) -> str | None:
        """Return the raw text between the first pair of '---' delimiters."""
        if not content.startswith("---"):
            return None
        end = content.find("\n---", 3)
        if end == -1:
            return None
        # Include the trailing newline so replacement is clean
        return content[3:end + 1]


# ---------------------------------------------------------------------------
# Pure priority calculation
# ---------------------------------------------------------------------------


def calculate_priority(
    current_weight: float,
    deep_work: bool,
    deadline_hours: float | None,
) -> float:
    """
    Calculate the priority score for a persona given context flags.

    Formula:
        priority = current_weight
                 + 0.1  * deep_work
                 + 0.2  * (deadline_hours <= 4)
                 + 0.15 * (deadline_hours <= 12)
                 + 0.1  * (deadline_hours <= 24)
                 + 0.05 * (deadline_hours <= 48)

    The deadline bonuses are mutually exclusive in the sense that only the
    tightest applicable bracket contributes (they do NOT stack).  A deadline
    of 3h satisfies ≤4h, ≤12h, ≤24h, and ≤48h — but the formula adds each
    applicable term independently, matching the spec exactly.

    Result is clamped to [0.0, 1.0].

    Args:
        current_weight: The persona's current dynamic weight in [0.0, 1.0].
        deep_work: True if the task is a deep-work block.
        deadline_hours: Hours until deadline, or None if no deadline.

    Returns:
        Priority score in [0.0, 1.0].
    """
    score = current_weight

    if deep_work:
        score += 0.1

    if deadline_hours is not None:
        if deadline_hours <= 4:
            score += 0.2
        if deadline_hours <= 12:
            score += 0.15
        if deadline_hours <= 24:
            score += 0.1
        if deadline_hours <= 48:
            score += 0.05

    return max(0.0, min(1.0, score))


# ---------------------------------------------------------------------------
# Weight adjustment
# ---------------------------------------------------------------------------


def adjust_weight(
    persona_slug: str,
    delta: float,
    justification: str,
    persistence: WeightPersistenceProtocol,
) -> float:
    """
    Adjust the current_weight of a persona by *delta*, persist, and log.

    Steps:
    1. Read current_weight via persistence.get_current_weight()
    2. Compute new_weight = clamp(current_weight + delta, 0.0, 1.0)
    3. Persist via persistence.set_current_weight()
    4. Append a WeightHistoryEntry via persistence.append_weight_history()
    5. Return new_weight

    Args:
        persona_slug: The persona's unique slug identifier.
        delta: Amount to add to current_weight (may be negative).
        justification: Human-readable reason for the adjustment.
        persistence: Adapter implementing WeightPersistenceProtocol.

    Returns:
        The new current_weight after clamping.
    """
    old_weight = persistence.get_current_weight(persona_slug)
    new_weight = max(0.0, min(1.0, old_weight + delta))

    persistence.set_current_weight(persona_slug, new_weight)

    entry = WeightHistoryEntry(
        timestamp=datetime.now(tz=timezone.utc),
        old_weight=old_weight,
        new_weight=new_weight,
        justification=justification,
    )
    persistence.append_weight_history(persona_slug, entry)

    logger.info(
        "Weight adjusted: slug=%s %.4f → %.4f (delta=%.4f) reason=%r",
        persona_slug,
        old_weight,
        new_weight,
        delta,
        justification,
    )

    return new_weight


# ---------------------------------------------------------------------------
# CLI Interface for OpenClaw Agent
# ---------------------------------------------------------------------------
def main() -> None:
    import argparse
    import json
    import sys
    
    parser = argparse.ArgumentParser(description="CLI wrapper generated by refactoring")
    parser.add_argument("--action", help="Action to perform")
    parser.add_argument("--args-json", default="{}", help="Arguments as JSON string")
    
    args = parser.parse_args()
    print("Execution of stub CLI for", __file__)
    print("Action:", args.action)
    print("Args:", args.args_json)

if __name__ == "__main__":
    main()
