"""
Crisis Mode — crisis detection and sacrifice algorithm.

Implements:
- CrisisResult dataclass: result of crisis detection
- Task dataclass: a schedulable task with priority and movability
- SacrificeResult dataclass: result of the sacrifice algorithm
- detect_crisis(): classifies a message as crisis or not
- sacrifice_algorithm(): boosts persona weight and frees ≥ 2h of Deep Work
- record_crisis_event(): appends a crisis event to personas/{slug}/MEMORY.md

Crisis detection heuristics:
  - Explicit trigger: message contains "/crise"
  - Keyword heuristic: urgency keywords raise confidence; if > 0.8 → is_crisis=True

Sacrifice algorithm (Requirements 4.2, 4.3, 4.4, 4.5, 4.6):
  1. Boost: new_weight = min(current_weight + 0.3, 1.0)
  2. Filter sacrificable tasks: movable=True AND priority < new_weight * 0.6
  3. Cancel/move tasks until ≥ 2h of Deep Work is freed
  4. If insufficient tasks: return fallback=True with available options, no side effects
  5. Record event in personas/{slug}/MEMORY.md
"""

from __future__ import annotations

import logging
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path

logger = logging.getLogger(__name__)

# ---------------------------------------------------------------------------
# Minimum Deep Work hours to free in a crisis
# ---------------------------------------------------------------------------

CRISIS_DEEP_WORK_TARGET_HOURS: float = 2.0

# ---------------------------------------------------------------------------
# Urgency keywords used for heuristic confidence scoring
# ---------------------------------------------------------------------------

_URGENCY_KEYWORDS: list[tuple[str, float]] = [
    # High-weight signals
    ("urgente", 0.35),
    ("emergência", 0.35),
    ("emergencia", 0.35),
    ("crítico", 0.30),
    ("critico", 0.30),
    ("socorro", 0.30),
    ("ajuda agora", 0.30),
    ("preciso agora", 0.30),
    ("imediato", 0.25),
    ("imediatamente", 0.25),
    ("agora mesmo", 0.25),
    ("não pode esperar", 0.25),
    ("nao pode esperar", 0.25),
    ("prazo hoje", 0.25),
    ("deadline hoje", 0.25),
    ("tudo parado", 0.25),
    ("sistema caiu", 0.30),
    ("servidor caiu", 0.30),
    ("produção caiu", 0.30),
    ("producao caiu", 0.30),
    # Medium-weight signals
    ("urgência", 0.20),
    ("urgencia", 0.20),
    ("problema grave", 0.20),
    ("situação crítica", 0.20),
    ("situacao critica", 0.20),
    ("preciso de ajuda", 0.15),
    ("não consigo", 0.10),
    ("nao consigo", 0.10),
    ("travado", 0.10),
    ("bloqueado", 0.10),
]


# ---------------------------------------------------------------------------
# Domain models
# ---------------------------------------------------------------------------


@dataclass
class CrisisResult:
    """Result of crisis detection for a given message."""

    is_crisis: bool
    confidence: float
    affected_persona: str | None


@dataclass
class Task:
    """A schedulable task that may be sacrificed during a crisis."""

    id: str
    title: str
    duration_hours: float
    movable: bool
    priority: float


@dataclass
class SacrificeResult:
    """Result of the sacrifice algorithm."""

    sacrificed_tasks: list[Task]
    freed_hours: float
    new_weight: float
    fallback: bool


# ---------------------------------------------------------------------------
# Crisis detection
# ---------------------------------------------------------------------------


def detect_crisis(message: str, affected_persona: str | None = None) -> CrisisResult:
    """
    Classify a message as a crisis or not.

    Returns is_crisis=True when:
    - The message contains the explicit trigger "/crise", OR
    - The heuristic confidence score exceeds 0.8 (urgency keyword matching)

    Args:
        message: The raw user message to classify.
        affected_persona: Optional slug of the persona context (passed through).

    Returns:
        CrisisResult with is_crisis, confidence, and affected_persona.
    """
    lower = message.lower()

    # Explicit trigger — always a crisis, maximum confidence
    if "/crise" in lower:
        logger.info("Crisis detected via explicit trigger '/crise'")
        return CrisisResult(
            is_crisis=True,
            confidence=1.0,
            affected_persona=affected_persona,
        )

    # Heuristic keyword scoring — accumulate confidence, cap at 1.0
    confidence: float = 0.0
    for keyword, weight in _URGENCY_KEYWORDS:
        if keyword in lower:
            confidence = min(confidence + weight, 1.0)
            logger.debug("Urgency keyword matched: %r (+%.2f → %.2f)", keyword, weight, confidence)

    is_crisis = confidence > 0.8

    if is_crisis:
        logger.info(
            "Crisis detected via heuristic: confidence=%.2f persona=%s",
            confidence,
            affected_persona,
        )
    else:
        logger.debug(
            "No crisis detected: confidence=%.2f persona=%s",
            confidence,
            affected_persona,
        )

    return CrisisResult(
        is_crisis=is_crisis,
        confidence=confidence,
        affected_persona=affected_persona,
    )


# ---------------------------------------------------------------------------
# Sacrifice algorithm
# ---------------------------------------------------------------------------


def sacrifice_algorithm(
    persona_slug: str,
    current_weight: float,
    tasks: list[Task],
) -> SacrificeResult:
    """
    Apply the crisis sacrifice algorithm for a persona.

    Steps:
    1. Boost: new_weight = min(current_weight + 0.3, 1.0)
    2. Filter sacrificable tasks: movable=True AND priority < new_weight * 0.6
    3. Sort by priority ascending (lowest priority sacrificed first)
    4. Cancel/move tasks until ≥ 2h of Deep Work is freed
    5. If total available hours < 2h: return fallback=True, no tasks sacrificed

    Args:
        persona_slug: The slug of the persona in crisis.
        current_weight: The persona's current dynamic weight in [0.0, 1.0].
        tasks: All tasks to consider for sacrifice.

    Returns:
        SacrificeResult with sacrificed tasks, freed hours, new weight, and fallback flag.
    """
    # Step 1 — apply crisis boost
    new_weight = min(current_weight + 0.3, 1.0)
    crisis_threshold = new_weight * 0.6

    logger.info(
        "Sacrifice algorithm: persona=%s current_weight=%.4f → new_weight=%.4f threshold=%.4f",
        persona_slug,
        current_weight,
        new_weight,
        crisis_threshold,
    )

    # Step 2 — filter sacrificable tasks
    sacrificable = [
        t for t in tasks
        if t.movable and t.priority < crisis_threshold
    ]

    logger.debug(
        "Sacrificable tasks found: %d / %d total",
        len(sacrificable),
        len(tasks),
    )

    # Step 3 — sort by priority ascending (sacrifice lowest-priority first)
    sacrificable.sort(key=lambda t: t.priority)

    # Step 4 — check if we can free enough hours
    total_available = sum(t.duration_hours for t in sacrificable)

    if total_available < CRISIS_DEEP_WORK_TARGET_HOURS:
        # Fallback: not enough tasks to free 2h — return options without executing
        logger.warning(
            "Fallback: insufficient sacrificable hours (%.2fh available, need %.2fh). "
            "persona=%s",
            total_available,
            CRISIS_DEEP_WORK_TARGET_HOURS,
            persona_slug,
        )
        return SacrificeResult(
            sacrificed_tasks=sacrificable,  # available options for the user
            freed_hours=total_available,
            new_weight=new_weight,
            fallback=True,
        )

    # Step 5 — greedily select tasks until ≥ 2h freed
    selected: list[Task] = []
    freed: float = 0.0

    for task in sacrificable:
        selected.append(task)
        freed += task.duration_hours
        if freed >= CRISIS_DEEP_WORK_TARGET_HOURS:
            break

    logger.info(
        "Sacrifice complete: %d tasks sacrificed, %.2fh freed. persona=%s",
        len(selected),
        freed,
        persona_slug,
    )

    return SacrificeResult(
        sacrificed_tasks=selected,
        freed_hours=freed,
        new_weight=new_weight,
        fallback=False,
    )


# ---------------------------------------------------------------------------
# Crisis event recording
# ---------------------------------------------------------------------------


def record_crisis_event(
    persona_slug: str,
    result: SacrificeResult,
    personas_dir: Path | None = None,
) -> None:
    """
    Append a crisis event entry to personas/{slug}/MEMORY.md.

    The entry records the timestamp, new weight, freed hours, and the list
    of sacrificed tasks (or fallback options if no tasks were executed).

    Args:
        persona_slug: The slug of the persona in crisis.
        result: The SacrificeResult from sacrifice_algorithm().
        personas_dir: Base directory for persona files. Defaults to ./personas.
    """
    if personas_dir is None:
        personas_dir = Path("personas")

    memory_path = personas_dir / persona_slug / "MEMORY.md"
    memory_path.parent.mkdir(parents=True, exist_ok=True)

    now = datetime.now(tz=timezone.utc).isoformat()
    status = "FALLBACK" if result.fallback else "EXECUTED"

    task_lines = "\n".join(
        f"  - [{t.id}] {t.title} ({t.duration_hours:.1f}h, priority={t.priority:.3f})"
        for t in result.sacrificed_tasks
    )
    if not task_lines:
        task_lines = "  (no tasks available)"

    entry = (
        f"\n## Crisis Event — {now}\n\n"
        f"- **Status**: {status}\n"
        f"- **New weight**: {result.new_weight:.4f}\n"
        f"- **Freed hours**: {result.freed_hours:.2f}h\n"
        f"- **Tasks {'sacrificed' if not result.fallback else 'available (not executed)'}**:\n"
        f"{task_lines}\n"
    )

    if not memory_path.exists():
        memory_path.write_text(f"# MEMORY — {persona_slug}\n{entry}", encoding="utf-8")
    else:
        with memory_path.open("a", encoding="utf-8") as fh:
            fh.write(entry)

    logger.info(
        "Crisis event recorded: persona=%s status=%s freed=%.2fh path=%s",
        persona_slug,
        status,
        result.freed_hours,
        memory_path,
    )
