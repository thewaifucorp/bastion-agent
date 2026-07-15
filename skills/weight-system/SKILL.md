---
name: bastion/weight-system
version: 1.0.0
description: >
  Calculates dynamic persona priority and manages weight adjustments
  (current_weight). Persists changes in USER.md and records history
  with timestamp and justification in personas/{slug}/weight-history.md.
triggers:
  - internal call from bastion/crisis-mode when applying crisis boost
  - internal call from bastion/weekly-review when suggesting weight adjustments
  - internal call from bastion/self-improving when recording promotions/decays
  - user explicitly requests to adjust a persona's weight
  - HEARTBEAT executes weekly weight review task
---

# Skill: bastion/weight-system

## Objective

Manage Bastion's dynamic persona weight system:

1. **Calculate priority** of a persona given current context (deep work, deadline)
2. **Adjust current_weight** in a persisted and auditable way
3. **Record history** of all changes with timestamp and justification

---

## Concepts

### base_weight vs current_weight

| Field | Description | When it changes |
|---|---|---|
| `base_weight` | Fixed weight defined at persona creation (0.0–1.0) | Rarely — only by explicit user edit |
| `current_weight` | Current dynamic weight (0.0–1.0) | Crises, weekly reviews, learnings, manual adjustments |

`current_weight` is the value used in all priority and matching calculations.

### Priority Formula

```
priority = current_weight
         + 0.1  × (deep_work)
         + 0.2  × (deadline ≤ 4h)
         + 0.15 × (deadline ≤ 12h)
         + 0.1  × (deadline ≤ 24h)
         + 0.05 × (deadline ≤ 48h)
```

Result is always clamped to **[0.0, 1.0]**.

Deadline bonuses are **additive** — a 3h deadline accumulates bonuses for ≤4h, ≤12h, ≤24h and ≤48h simultaneously.

**Examples:**

| current_weight | deep_work | deadline | priority |
|---|---|---|---|
| 0.7 | false | none | 0.70 |
| 0.7 | true | none | 0.80 |
| 0.7 | false | 3h | 1.00 (clamped de 1.20) |
| 0.5 | true | 20h | 0.75 |
| 0.9 | false | 50h | 0.90 |

---

## When This Skill is Triggered

### 1. Crisis Boost (bastion/crisis-mode)

When `crisis-mode` detects a crisis and identifies affected persona:

```
adjust_weight(
    persona_slug=<crisis persona slug>,
    delta=+0.3,
    justification="Crisis boost: <crisis description>",
    persistence=UserMdAdapter(...)
)
```

Clamp ensures result never exceeds 1.0.

---

### 2. Weekly Review (bastion/weekly-review)

Every Monday at 9am, HEARTBEAT triggers `weekly-review`, which:

1. Analyzes last 50 life_log records per persona
2. Compares usage pattern with current weights
3. Suggests adjustments to user
4. After confirmation, calls `adjust_weight()` for each persona with accepted suggestion

---

### 3. Self-Improving (bastion/self-improving)

When a pattern is promoted or decayed, `self-improving` records the change:

```
adjust_weight(
    persona_slug=<slug>,
    delta=<+0.05 for promotion, -0.05 for decay>,
    justification="Pattern promoted to HOT: <pattern name>",
    persistence=UserMdAdapter(...)
)
```

---

### 4. Manual Adjustment by User

User can request direct adjustment:

```
"Increase Tech Lead persona weight to 0.95"
"Reduce Entrepreneur weight by 0.1"
```

Flow:
1. Identify persona by name or slug
2. Calculate necessary delta
3. Confirm with user: `"{locale:confirm_adjust}"`
4. After confirmation, call `adjust_weight()`

---

## Persistence

### USER.md

`current_weight` of each persona is maintained in `USER.md` frontmatter:

```yaml
personas:
  - slug: "tech-lead"
    name: "Tech Lead"
    current_weight: 0.9
  - slug: "empreendedor"
    name: "Entrepreneur"
    current_weight: 0.7
```

`UserMdAdapter` updates this value automatically on each `adjust_weight()` call.

### personas/{slug}/weight-history.md

Each adjustment generates a history line:

```
# Weight History

- 2025-01-15T10:30:00+00:00 | 0.7000 → 1.0000 | Crisis boost: production server down
- 2025-01-22T09:00:00+00:00 | 1.0000 → 0.8500 | Weekly review: usage normalized after crisis
- 2025-01-29T09:00:00+00:00 | 0.8500 → 0.9000 | Pattern promoted to HOT: deploy-checklist
```

Format of each line:
```
- {ISO 8601 timestamp} | {old_weight:.4f} → {new_weight:.4f} | {justification}
```

---

## Architecture (Hexagonal)

Skill uses **Protocol/Adapter** pattern to decouple business logic from persistence:

```
WeightPersistenceProtocol (port)
    ├── get_current_weight(slug) → float
    ├── set_current_weight(slug, weight) → None
    └── append_weight_history(slug, entry) → None

UserMdAdapter (default concrete adapter)
    ├── Reads/writes USER.md (YAML frontmatter)
    └── Appends to personas/{slug}/weight-history.md
```

To swap the persistence backend (e.g., database), implement a new adapter that satisfies `WeightPersistenceProtocol` — without changing `calculate_priority()` or `adjust_weight()`.

---

## CLI Commands

> IMPORTANT: CLI Command
> As you are an OpenClaw agent, you must invoke all operations via command line (`exec python3 ...`). Do not attempt to interpret Python code natively.



```python
from skills.weight_system.weight_system import (
    calculate_priority,
    adjust_weight,
    UserMdAdapter,
    WeightHistoryEntry,
)
from pathlib import Path

# Instantiate the adapter
adapter = UserMdAdapter(
    user_md_path=Path("USER.md"),
    personas_dir=Path("personas"),
)

# Calculate priority
priority = calculate_priority(
    current_weight=0.7,
    deep_work=True,
    deadline_hours=6.0,
)
# → 0.95 (0.7 + 0.1 + 0.15)

# Adjust weight (crisis boost)
new_weight = adjust_weight(
    persona_slug="tech-lead",
    delta=+0.3,
    justification="Crisis boost: critical production deploy",
    persistence=adapter,
)
# → persists to USER.md + appends to personas/tech-lead/weight-history.md
```

---

## Edge Cases

### Clamp to [0.0, 1.0]

Any delta that would take the weight below 0.0 or above 1.0 is silently clamped:

```python
adjust_weight("tech-lead", delta=+0.5, ...)  # current=0.9 → new=1.0 (not 1.4)
adjust_weight("tech-lead", delta=-0.5, ...)  # current=0.1 → new=0.0 (not -0.4)
```

### Persona not found in USER.md

If the slug doesn't exist in USER.md, `UserMdAdapter.get_current_weight()` raises `KeyError`.
The caller must handle this error and verify the persona exists before adjusting.

### weight-history.md file doesn't exist

The `UserMdAdapter` creates the file automatically on the first write, including the `# Weight History` header.

### personas/{slug}/ folder doesn't exist

The `UserMdAdapter` creates the folder automatically via `mkdir(parents=True, exist_ok=True)`.
