---
name: bastion/crisis-mode
version: 1.0.0
description: >
  Detects crisis situations and executes the sacrifice algorithm to free up
  Deep Work time, automatically replanning the affected persona's schedule.
triggers:
  - "/crise" or "/crisis"
  - extreme urgency language (confidence > 0.8)
---

# Crisis Mode

## When this skill is activated

Crisis Mode is activated in two situations:

1. **Explicit trigger**: user sends `/crise` or `/crisis` in any message.
2. **Automatic detection**: message contains extreme urgency language and the
   internal classifier returns `confidence > 0.8`. Examples of language that
   trigger detection: "urgent", "emergency", "system down", "deadline today",
   "everything stopped", "can't wait".

---

## Complete Flow

```
Message received
      │
      ▼
detect_crisis(message)
      │
      ├── is_crisis=False → end, process normally
      │
      └── is_crisis=True
              │
              ▼
        Identify affected persona
              │
              ▼
        sacrifice_algorithm(persona_slug, current_weight, tasks)
              │
              ├── fallback=True (< 2h available)
              │       │
              │       └── Notify user with available options
              │           (without executing anything)
              │
              └── fallback=False
                      │
                      ▼
                Cancel/move selected tasks
                      │
                      ▼
                Notify user with replanning summary
                      │
                      ▼
                record_crisis_event(persona_slug, result)
                      │
                      ▼
                Append event to personas/{slug}/MEMORY.md
```

---

## Sacrifice Algorithm — Details

### 1. Weight Boost

```
new_weight = min(current_weight + 0.3, 1.0)
```

The crisis persona's weight is elevated by 0.3, respecting the maximum limit of 1.0.

### 2. Sacrificeable Tasks Filter

A task is sacrificeable if it satisfies **both** criteria:

- `movable = True` — task can be moved or cancelled
- `priority < new_weight * 0.6` — task priority is low enough

### 3. Task Selection

Sacrificeable tasks are sorted by ascending priority (lowest priority first). 
The algorithm selects tasks until freeing **≥ 2 hours of Deep Work**.

### 4. Fallback

If the sum of sacrificeable task hours is **< 2h**, the algorithm returns
`fallback=True` with the list of available options, **without executing any action**.
The user is notified and can decide what to do.

---

## User Notification

### Normal case (replanning executed)

```
{locale:crisis_activated}

{locale:weight_elevated}

{locale:tasks_moved}
  {locale:task_item}
  {locale:task_item}

{locale:deep_work_available}
```

### Fallback case (insufficient hours)

```
{locale:fallback_title}

{locale:fallback_message}
{locale:fallback_options}
  {locale:task_item}
  {locale:task_item}

{locale:fallback_no_action}
```

---

## MEMORY.md Recording

After each execution (with or without fallback), the event is recorded in
`personas/{slug}/MEMORY.md` with:

- ISO 8601 timestamp
- Status: EXECUTED or FALLBACK
- New persona weight
- Hours freed
- List of sacrificed tasks (or available options in fallback)

---

## Edge Cases

| Situation | Behavior |
|----------|----------|
| No `movable=True` tasks | Immediate fallback with empty list |
| All tasks have high priority | Fallback with empty list |
| `current_weight` is already 1.0 | Boost doesn't change weight (remains 1.0) |
| Message contains `/crise` + other words | Explicit trigger takes precedence (confidence=1.0) |
| Persona not identified | `affected_persona=None`, user is asked which persona |

---

## Dependencies

- `crisis_mode.py` — computational logic (detect_crisis, sacrifice_algorithm, record_crisis_event)
- `personas/{slug}/MEMORY.md` — persona memory file (created automatically if doesn't exist)
