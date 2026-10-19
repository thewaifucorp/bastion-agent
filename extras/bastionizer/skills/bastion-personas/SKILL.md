---
name: bastion-personas
description: Create, route, switch and weigh Bastion personas — one per life area (work, health, family, finances, a project), each a SOUL.md with scope, objectives, keywords and a privacy tier. Use when the person wants a new persona, asks who is answering, wants to switch, or when a persona's priority should change.
license: MIT
metadata:
  version: "0.1.0"
  author: bastion
---

# Bastion personas

`B` = `python3 <dir of the bastion skill>/scripts/bastion.py` (the `bastion`
skill sits next to this one).

A persona is `~/.bastion/personas/<slug>/SOUL.md`: YAML frontmatter, then
the prompt you speak with. Same contract as Bastion's own personas, so a
persona made here works in a Bastion install and back.

```yaml
---
name: "Health"
description: "Sleep, training, food and medical follow-ups"
bastion:
  privacy_tier: "local-only"   # or cloud-ok
  weight: 0.6                  # 0..1, influence in routing and Cabinet
trigger_keywords: ["sleep", "gym", "doctor", "pain"]
domains: ["health", "fitness"]
skills: []
objectives: ["Keep a steady sleep and training routine"]
goals: ["7h sleep on weeknights", "3 workouts a week"]
scope: "Does not diagnose; sends medical questions to a doctor"
---
You are Health, ...
```

## Creating one

Ask, conversationally and briefly (accept short answers, infer the rest and
show it back):

1. Name and the area it covers.
2. Voice: how it should talk.
3. Words that should wake it (keywords — at least three).
4. What it's for (objectives), how success looks (goals), what it must not do (scope).
5. Private? Health, money and relationships default to `local-only`.
6. Weight: default 0.5; the person's main focus 0.7–0.8.

Then:

```bash
B persona new --name "Health" --description "..." --keywords "sleep,gym,doctor" \
  --domains "health" --objectives "..." --goals "...,..." --scope "..." \
  --tier local-only --weight 0.6 --voice "calm, practical"
B persona check
```

Write a richer prompt body with `--body` or by editing the file afterwards.
Never put secrets in a SOUL.md.

## Routing

- Each request: `B persona route "<request>" --use`. Keyword hits count 1,
  domain hits 0.5, times (0.5 + weight); the active persona gets a small
  bonus so ties don't flip. No match keeps the active persona.
- Explicit switch ("talk to me as Finance"): `B persona use finance`, then
  adopt the printed prompt.
- Say the switch in a few words ("Health here —") the first time, not every turn.
- Routing picks the wrong persona repeatedly: add or fix keywords in its
  SOUL.md; that is the fix, not overriding by hand each time.

## Weights

Weights change with life: a deadline, a crisis, a weekly review.

```bash
B persona weight work --value 0.8 --why "launch week"
B persona history work
```

Always give the reason; every change is kept. Suggest lowering a weight back
when the reason is over.
