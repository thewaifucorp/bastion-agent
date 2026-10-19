---
name: bastion-cabinet
description: Bastion's Cabinet — convene two or more personas to deliberate on a decision that crosses life areas, each arguing from its own objectives, keeping dissent, then a weighted recommendation. Use for trade-offs ("should I take the job", "work tonight or rest"), weekly reviews, and when the person types /cabinet.
license: MIT
metadata:
  version: "0.1.0"
  author: bastion
---

# Bastion Cabinet

`B` = `python3 <dir of the bastion skill>/scripts/bastion.py`.

Cabinet is for trade-offs, not fake consensus. Use it when the personas'
areas pull in different directions; for questions inside one area, just
answer as that persona.

## Running one

1. Pick the personas the question touches (two to four), or all of them for
   a weekly review.
2. `B cabinet work health family -q "take the Saturday on-call shift?"` —
   prints each persona's weight and share of the vote, objectives, scope,
   and what memory holds for it on the question.
3. Each persona speaks **in its own voice**, from its objectives, in a few
   lines: its position, the strongest reason, what it would need to change
   its mind. Personas disagree when their objectives conflict — don't
   smooth that over.
4. **Recommendation**: weigh the positions by their share, say which way it
   goes and why, and what the losing side's concern costs. Name the dissent.
5. The person decides. Store the decision:
   `B mem add "<decision and why>" --kind decision --importance 4`
   and turn any follow-up into a reminder.

## Weekly review

Run all personas on "how did this week go and what matters next week": each
reports on its goals (use `B mem search` per persona), the Cabinet proposes
next week's focus and any weight changes (`B persona weight … --why`), and
the person confirms.

## Privacy

A `local-only` persona can take part, but its private memories stay out
unless the person asks: argue from its objectives, not from private facts.
