---
name: bastion/self-improving
version: 1.0.0
description: >
  Fork of ivangdavila/self-improving with persona awareness. Learns behavior
  patterns per persona over time using tiered memory (HOT/WARM/COLD), automatic
  promotion/decay with weight awareness, conflict resolution by precedence, and
  complete namespace isolation between personas.
triggers:
  - HEARTBEAT executes weekly life_log analysis (every 7 days)
  - behavior pattern observed 3+ times in 7 days for a persona
  - persona enters crisis mode (crisis-mode detects is_crisis=true)
  - two patterns conflict during persona matching
  - bastion/weekly-review requests accumulated pattern analysis
---

# Skill: bastion/self-improving

## Objective

Learn behavior patterns per persona over time, progressively improving responses
without user needing to repeat preferences.

---

## Tiered Memory (maintained from original)

Each persona has three memory layers:

| Tier | File | Size | Loading |
|------|------|------|---------|
| **HOT** | `personas/{slug}/memory.md` | ≤ 100 lines | Always — injected in context |
| **WARM** | `personas/{slug}/index.md` | Unlimited | On demand (semantic search) |
| **COLD** | `personas/{slug}/archive/` | Unlimited | Rarely — explicit search |

---

## Promotion Rules with Weight Awareness

### Promotion to HOT (Requirement 12.1)

A pattern is promoted to HOT when:
- Observed **3 or more times** in a **7-day** window

### Low Weight Blocking (Requirement 12.2)

If `current_weight < 0.3` for a persona:
- Pattern is **not promoted to global HOT**
- Remains in WARM until persona weight increases
- Justification recorded: `"Weight gate: current_weight=X.XXXX < 0.3"`

### Crisis Priority (Requirement 12.3)

When a crisis is detected by `bastion/crisis-mode`:
- Patterns from **crisis persona** have priority over all others
- Weight gate is **bypassed** for crisis persona
- Justification recorded: `"Crisis priority: N occurrences (crisis override — weight gate bypassed)"`

---

## Conflict Resolution (Requirement 12.4)

When two patterns conflict, the precedence order is:

```
1. More specific  (higher specificity value)
2. More recent    (higher updated_at)
3. Higher weight  (higher persona_weight)
```

If all criteria tie, `pattern_a` wins (deterministic).

**Example:**

```python
from skills.self_improving.promotion import conflict_resolution

winner = conflict_resolution(pattern_a, pattern_b)
# → returns the winning Pattern with the criterion used logged
```

---

## Promotion and Decay Logging (Requirement 12.5)

Every promotion or decay is recorded in `personas/{slug}/weight-history.md`:

```
# Weight History

- 2025-01-15T10:30:00+00:00 | PROMOTED WARM → HOT | pattern:deploy-checklist | Promotion criteria met: 4 occurrences in last 7 days, current_weight=0.9000
- 2025-01-22T09:00:00+00:00 | DECAYED HOT → WARM  | pattern:deploy-checklist | Pattern not accessed in 14 days
- 2025-01-29T09:00:00+00:00 | PROMOTED WARM → HOT | pattern:deploy-checklist | Crisis priority: 3 occurrences (crisis override — weight gate bypassed)
```

Format of each line:
```
- {ISO 8601 timestamp} | {action} | pattern:{id} | {justification}
```

---

## Namespace Isolation (Requirement 12.6)

**Guarantee:** Operations on `personas/{slug-a}/` **never** touch `personas/{slug-b}/`.

The `FileSystemAdapter` derives all paths from `self._personas_dir / persona_slug`.
No operation accepts two different slugs in the same write call.

---

## Architecture (Hexagonal)

```
PromotionPersistenceProtocol (port)
    ├── get_pattern(persona_slug, pattern_id) → Pattern | None
    ├── save_pattern(pattern) → None
    ├── get_current_weight(persona_slug) → float
    └── append_promotion_history(persona_slug, timestamp, pattern_id, action, justification) → None

FileSystemAdapter (default concrete adapter)
    ├── Reads/writes personas/{slug}/memory.md (HOT tier)
    ├── Reads current_weight from USER.md
    └── Appends to personas/{slug}/weight-history.md
```

To swap the backend (e.g., database), implement a new adapter that satisfies `PromotionPersistenceProtocol` — without changing `promote_pattern()`, `decay_pattern()`, or `conflict_resolution()`.

---

## Comandos CLI

> IMPORTANT: CLI Command
> As you are an OpenClaw agent, you must invoke all operations via command line (`exec python3 ...`). Do not attempt to interpret Python code natively.



```python
from pathlib import Path
from datetime import datetime, timezone
from skills.self_improving.promotion import (
    Pattern,
    MemoryTier,
    FileSystemAdapter,
    promote_pattern,
    decay_pattern,
    conflict_resolution,
)

adapter = FileSystemAdapter(
    personas_dir=Path("personas"),
    user_md_path=Path("USER.md"),
)

# Create a pattern with recent occurrences
pattern = Pattern(
    id="deploy-checklist",
    persona_slug="tech-lead",
    description="Always checks the deploy checklist before pushing",
    tier=MemoryTier.WARM,
    specificity=3,
    persona_weight=0.9,
    occurrences=[
        datetime(2025, 1, 13, tzinfo=timezone.utc),
        datetime(2025, 1, 14, tzinfo=timezone.utc),
        datetime(2025, 1, 15, tzinfo=timezone.utc),
    ],
)

# Try to promote to HOT
promoted = promote_pattern(pattern, adapter, is_crisis=False)
# → True if current_weight >= 0.3 and 3+ occurrences in 7 days

# Resolve conflict between two patterns
winner = conflict_resolution(pattern_a, pattern_b)
# → Winning Pattern by order: specific > recent > weight

# Decay a pattern
decay_pattern(pattern, MemoryTier.WARM, "Pattern not accessed in 14 days", adapter)
```

---

## Integration with HEARTBEAT

The HEARTBEAT triggers this skill every 7 days via `bastion/weekly-review`:

1. Fetches the last 50 `life_log` records per persona
2. Groups by behavior pattern
3. For each pattern with 3+ occurrences in 7 days → calls `promote_pattern()`
4. For patterns not accessed in 14+ days → calls `decay_pattern()`
5. Records all changes in `personas/{slug}/weight-history.md`

---

## Edge Cases

### Persona with weight < 0.3

The pattern stays in WARM. When the persona's weight increases (via crisis boost
or weekly-review), the next HEARTBEAT execution may promote it.

### Active crisis

During a crisis, `is_crisis=True` bypasses the weight gate. After the crisis,
normal behavior is restored automatically.

### Pattern already in HOT

Calling `promote_pattern()` on a pattern already in HOT is idempotent — it updates
`updated_at` and `persona_weight`, but does not duplicate the entry in the history.

### memory.md file doesn't exist

The `FileSystemAdapter` creates the file automatically on the first write,
including the `# HOT Memory — {slug}` header.

### personas/{slug}/ folder doesn't exist

The `FileSystemAdapter` creates the folder automatically via `mkdir(parents=True, exist_ok=True)`.
