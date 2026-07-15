"""
Persona Engine — core logic for persona creation and matching.

Implements:
- Persona dataclass with all required SOUL.md fields
- ActivePersona dataclass (persona + effective weight)
- PersonaPersistenceProtocol (hexagonal port for SOUL.md I/O)
- create_persona() — builds a Persona and persists SOUL.md via the protocol
- match_personas() — keyword matching + fallback (Steps 1, 4, 5 of SKILL.md algorithm)

Semantic matching (Step 2) and time-of-day filtering (Step 3) require an LLM
and are intentionally out of scope for pure unit/property tests.
"""

from __future__ import annotations

import logging
import re
import unicodedata
from dataclasses import dataclass
from typing import Protocol, runtime_checkable

logger = logging.getLogger(__name__)

# ---------------------------------------------------------------------------
# Domain models
# ---------------------------------------------------------------------------


@dataclass
class Persona:
    """Represents a persona with all required SOUL.md frontmatter fields."""

    name: str
    slug: str
    base_weight: float
    current_weight: float
    domains: list[str]
    trigger_keywords: list[str]
    clawhub_skills: list[str]


@dataclass
class ActivePersona:
    """A persona that is active for a given message, with its effective weight."""

    persona: Persona
    weight: float


# ---------------------------------------------------------------------------
# Persistence protocol (hexagonal port)
# ---------------------------------------------------------------------------


@runtime_checkable
class PersonaPersistenceProtocol(Protocol):
    """Port for reading and writing persona SOUL.md files."""

    def write_soul_md(self, persona: Persona) -> None:
        """Persist the persona's SOUL.md to storage."""
        ...

    def read_soul_md(self, slug: str) -> Persona:
        """Load a persona from its SOUL.md file."""
        ...

    def slug_exists(self, slug: str) -> bool:
        """Return True if a persona with this slug already exists."""
        ...


# ---------------------------------------------------------------------------
# Slug generation
# ---------------------------------------------------------------------------


def _generate_slug(name: str, persistence: PersonaPersistenceProtocol) -> str:
    """
    Convert a persona name to a unique kebab-case slug.

    Rules (from SKILL.md):
    1. Lowercase
    2. Remove accents (NFKD normalisation)
    3. Replace spaces/separators with hyphens
    4. Remove characters that are not letters, digits, or hyphens
    5. Collapse consecutive hyphens
    6. Strip leading/trailing hyphens
    7. Append numeric suffix (-2, -3, …) if slug already exists
    """
    # Normalise unicode and strip accents
    normalised = unicodedata.normalize("NFKD", name)
    ascii_str = normalised.encode("ascii", "ignore").decode("ascii")

    # Lowercase
    slug = ascii_str.lower()

    # Replace separators with hyphens
    slug = re.sub(r"[\s/\\&_]+", "-", slug)

    # Keep only alphanumeric and hyphens
    slug = re.sub(r"[^a-z0-9-]", "", slug)

    # Collapse consecutive hyphens
    slug = re.sub(r"-{2,}", "-", slug)

    # Strip leading/trailing hyphens
    slug = slug.strip("-")

    if not slug:
        slug = "persona"

    # Ensure uniqueness
    if not persistence.slug_exists(slug):
        return slug

    # Exponential search to find an upper bound
    low = 2
    high = 2
    while persistence.slug_exists(f"{slug}-{high}"):
        low = high
        high *= 2

    # Binary search to find the exact first available suffix between low and high
    ans = high
    while low <= high:
        mid = (low + high) // 2
        if not persistence.slug_exists(f"{slug}-{mid}"):
            ans = mid
            high = mid - 1
        else:
            low = mid + 1

    return f"{slug}-{ans}"


# ---------------------------------------------------------------------------
# Persona creation
# ---------------------------------------------------------------------------


def create_persona(
    name: str,
    domains: list[str],
    trigger_keywords: list[str],
    clawhub_skills: list[str],
    base_weight: float,
    persistence: PersonaPersistenceProtocol,
) -> Persona:
    """
    Create a Persona, generate its slug, and persist SOUL.md via *persistence*.

    Args:
        name: Human-readable persona name.
        domains: List of knowledge/responsibility domains.
        trigger_keywords: Keywords that activate this persona.
        clawhub_skills: ClawHub skill identifiers for this persona.
        base_weight: Fixed priority weight in [0.0, 1.0].
        persistence: Adapter that handles SOUL.md I/O.

    Returns:
        The created Persona instance.
    """
    if not 0.0 <= base_weight <= 1.0:
        raise ValueError(f"base_weight must be in [0.0, 1.0], got {base_weight}")

    slug = _generate_slug(name, persistence)

    persona = Persona(
        name=name,
        slug=slug,
        base_weight=base_weight,
        current_weight=base_weight,  # initialised equal to base_weight
        domains=list(domains),
        trigger_keywords=list(trigger_keywords),
        clawhub_skills=list(clawhub_skills),
    )

    persistence.write_soul_md(persona)
    logger.info("Persona created: slug=%s", slug)

    return persona


# ---------------------------------------------------------------------------
# Persona matching
# ---------------------------------------------------------------------------


def match_personas(
    message: str,
    personas: list[Persona],
) -> list[ActivePersona]:
    """
    Identify which personas should be active for *message*.

    Implements Steps 1, 4, and 5 of the SKILL.md matching algorithm:
      - Step 1: keyword matching (case-insensitive substring match)
      - Step 4: activate ALL matching personas simultaneously
      - Step 5: fallback to persona with highest current_weight when no match

    Semantic matching (Step 2) and time-of-day filtering (Step 3) require
    external dependencies (LLM, clock) and are handled at the orchestrator level.

    Args:
        message: The incoming message text.
        personas: All available personas.

    Returns:
        List of ActivePersona instances. Never empty if *personas* is non-empty.
    """
    if not personas:
        logger.warning("match_personas called with empty personas list")
        return []

    message_lower = message.lower()

    # Step 1 — keyword matching (partial, case-insensitive)
    keyword_matches: list[ActivePersona] = [
        ActivePersona(persona=p, weight=p.current_weight)
        for p in personas
        if any(kw.lower() in message_lower for kw in p.trigger_keywords)
    ]

    # Step 4 — return all matches simultaneously
    if keyword_matches:
        logger.debug("Keyword matches: %d persona(s)", len(keyword_matches))
        return keyword_matches

    # Step 5 — fallback: persona with highest current_weight
    fallback = max(
        personas,
        key=lambda p: (p.current_weight, p.base_weight),
    )
    logger.debug("Fallback to persona: slug=%s", fallback.slug)
    return [ActivePersona(persona=fallback, weight=fallback.current_weight)]


# ---------------------------------------------------------------------------
# CLI Interface for OpenClaw Agent
# ---------------------------------------------------------------------------
def main() -> None:
    import argparse
    import json

    parser = argparse.ArgumentParser(description="Persona Engine CLI")
    subparsers = parser.add_subparsers(dest="command", required=True)

    # create
    create_parser = subparsers.add_parser("create")
    create_parser.add_argument("--name", required=True)
    create_parser.add_argument("--domains", default="[]")
    create_parser.add_argument("--trigger-keywords", default="[]")
    create_parser.add_argument("--clawhub-skills", default="[]")
    create_parser.add_argument("--base-weight", type=float, default=0.5)

    # match
    match_parser = subparsers.add_parser("match")
    match_parser.add_argument("--message", required=True)
    match_parser.add_argument("--personas-json", required=True, help="JSON list of personas")

    args = parser.parse_args()

    class DummyPersistence:
        def write_soul_md(self, p):
            pass

        def read_soul_md(self, s):
            return None

        def slug_exists(self, s):
            return False

    if args.command == "create":
        domains = json.loads(args.domains)
        t_keywords = json.loads(args.trigger_keywords)
        c_skills = json.loads(args.clawhub_skills)
        p = create_persona(
            args.name, domains, t_keywords, c_skills, args.base_weight, DummyPersistence()
        )
        print(f"Created: {p.slug}")
    elif args.command == "match":
        p_list = []
        raw = json.loads(args.personas_json)
        for rp in raw:
            p_list.append(
                Persona(
                    name=rp.get("name", ""),
                    slug=rp.get("slug", ""),
                    base_weight=rp.get("base_weight", 0.5),
                    current_weight=rp.get("current_weight", 0.5),
                    domains=rp.get("domains", []),
                    trigger_keywords=rp.get("trigger_keywords", []),
                    clawhub_skills=rp.get("clawhub_skills", []),
                )
            )
        res = match_personas(args.message, p_list)
        for r in res:
            print(f"Active: {r.persona.slug} (weight: {r.weight})")


if __name__ == "__main__":
    main()
