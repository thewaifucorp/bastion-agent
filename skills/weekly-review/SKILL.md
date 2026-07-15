---
name: bastion/weekly-review
version: "1.0.0"
description: >
  Executes weekly review of all active personas: aggregates interactions from
  last 7 days via life_log, calculates usage metrics, compares with current
  weights and presents a report with weight adjustment suggestions for user
  confirmation before applying any changes.
triggers:
  - HEARTBEAT every Monday at 9am
  - "/weekly-review"
  - "weekly review"
  - "review weights"
  - "how are my personas"
---

# Weekly Review — Weekly Persona Review

## When this skill is activated

1. **Automatic**: HEARTBEAT executes this skill every Monday at 9am.
2. **Manual**: user sends `/weekly-review` or requests a weekly review.

---

## Complete Flow

```
HEARTBEAT (Monday, 9am) or manual trigger
        │
        ▼
Load list of active personas from USER.md
        │
        ▼
For each active persona:
  life_log.get_persona_summary(persona, days=7)
        │
        ▼
Calculate usage metrics per persona
        │
        ▼
Compare usage pattern with current_weight of each persona
        │
        ▼
Generate report with adjustment suggestions
        │
        ├── No suggestions → inform weights are adequate
        │
        └── Has suggestions → present report to user
                │
                ▼
          Wait for user confirmation
                │
                ├── User confirms all → apply all adjustments via weight-system
                ├── User confirms partially → apply only confirmed ones
                └── User refuses → don't apply any adjustment
```

---

## Step 1 — Collect data from life_log

For each active persona listed in `USER.md`, call:

```
life_log.get_persona_summary(persona="{slug}", days=7)
```

Summary returns:
- `total_interactions`: total number of interactions in last 7 days
- `intents_used`: list of executed intents with count (e.g., `{"code_review": 12, "planning": 3}`)
- `tools_used`: list of called tools with count (e.g., `{"github": 8, "calendar": 2}`)
- `active_hours`: list of hours of day with most activity (e.g., `[9, 10, 14, 15]`)
- `last_interaction`: timestamp of last interaction

If a persona has no interactions in last 7 days, record `total_interactions=0`.

---

## Step 2 — Calculate usage metrics

For each persona, calculate:

| Metric | Calculation |
|--------|-------------|
| **Usage rate** | `total_interactions / total_interactions_all_personas` |
| **Dominant intent** | intent with highest count |
| **Dominant tool** | tool with highest count |
| **Activity window** | hours with ≥ 20% of persona's interactions |
| **Days since last interaction** | `today - last_interaction` in days |

---

## Step 3 — Compare with current weights

For each persona, compare usage rate with `current_weight`:

### Weight increase suggestion criteria

Suggest **increase** of `current_weight` if:
- `usage_rate > current_weight + 0.15` (persona being used much more than weight suggests)
- `total_interactions >= 20` in the week (consistent and expressive usage)

Suggested value: `min(current_weight + 0.1, 1.0)`

### Weight reduction suggestion criteria

Suggest **reduction** of `current_weight` if:
- `usage_rate < current_weight - 0.2` (persona being used much less than weight suggests)
- `total_interactions <= 3` in the week (very low usage)
- `current_weight > 0.3` (don't reduce personas already with low weight)

Suggested value: `max(current_weight - 0.1, 0.0)`

### No suggestion

Keep current weight if none of the above criteria are met.

---

## Step 4 — Generate report

Assemble report in clear and accessible language for user:

```
{locale:title}

{locale:separator}

{For each persona with interactions:}

{locale:persona_header}
   {locale:interactions}
   {locale:most_used_for}
   {locale:favorite_tool}
   {locale:peak_hours}
   {locale:current_weight}
   {If has suggestion:}
   {locale:suggestion}

{locale:separator}

{If has personas without interactions:}
{locale:inactive_title}
   {locale:inactive_item}
   {locale:inactive_item}

{locale:separator}

{If has adjustment suggestions:}
{locale:suggestions_prompt}

{If no suggestions:}
{locale:no_suggestions}
```

**Report language rules:**
- Use simple language, no technical jargon
- Replace "current_weight" with "weight" or "priority"
- Replace "intent" with "task type" or "what was done"
- Replace "tool" with "tool"
- Dates in `DD/MM/YYYY` format

---

## Step 5 — Wait for confirmation before applying

**Never apply adjustments without explicit user confirmation.**

### User response options

| Response | Action |
|----------|--------|
| `yes` / `confirm` / `apply all` | Apply all suggested adjustments |
| `no` / `cancel` / `keep` | Don't apply any adjustment |
| `choose` / `select` | List each suggestion individually for confirmation |

### Individual confirmation flow (when user responds "choose")

For each suggestion, ask:
> "{locale:confirm_individual}"

Wait for response before moving to next.

---

## Step 6 — Apply confirmed adjustments via weight-system

For each confirmed adjustment, call:

```
weight_system.adjust_weight(
    persona_slug="{slug}",
    delta={new_weight - current_weight},
    justification="Weekly review: usage rate {usage_rate:.0%} vs weight {current_weight} — {reason}"
)
```

`weight-system` persists new `current_weight` in `USER.md` and records change
with timestamp and justification in `personas/{slug}/weight-history.md`.

After applying all confirmed adjustments, confirm to user:

```
{locale:applied}
```

---

## Edge Cases

| Situation | Behavior |
|-----------|----------|
| No active personas in USER.md | Inform no personas configured and suggest onboarding |
| Empty life_log (first week) | Inform not enough data yet and review will be more useful next week |
| All personas without interactions | Present inactivity report and suggest resumption, no weight suggestions |
| User doesn't respond in 24h | Don't apply any adjustment; record review was presented but not confirmed |
| Adjustment would result in weight < 0.0 or > 1.0 | Automatic clamp by weight-system; inform user of actual final value |
| Persona in active crisis | Don't suggest weight reduction for persona with active crisis, regardless of usage rate |
| Tie between personas in usage rate | Keep current weights; don't suggest adjustment when difference is < 0.05 |

---

## Dependencies

- `skills/life-log` — `get_persona_summary(persona, days=7)` to collect usage data
- `skills/weight-system` — `adjust_weight(persona_slug, delta, justification)` to apply adjustments
- `USER.md` — list of active personas and their `current_weight`
- `personas/{slug}/weight-history.md` — adjustment history (written by weight-system)
