---
name: bastion-skill-forge
description: Bastion writes and improves its own skills from experience — after a task that worked, save the procedure as a SKILL.md; when a saved skill fails or is incomplete, patch it and bump its version. Use after any non-trivial task that succeeded, after the person corrects how you did something, and whenever a learned skill you used went wrong.
license: MIT
metadata:
  version: "0.1.0"
  author: bastion
---

# Bastion skill forge

`B` = `python3 <dir of the bastion skill>/scripts/bastion.py`.

Learned skills live in `~/.bastion/skills/<name>/SKILL.md` (or
`$BASTION_SKILLS_DIR`), in the agentskills format, so any agent can load
them. `B brief` lists them every session; read the SKILL.md before doing a
task one of them covers.

## When to write a new skill

Right after the task, while the details are fresh, when **any** of these held:

- it took 5 or more steps or tool calls and worked;
- you got there after a dead end (the working path is the valuable part);
- the person corrected your approach ("no, do it like this");
- the person asked for the same kind of thing before.

Don't write one for a one-off, or for something any agent already does well.
Check `B skill list` first: improving an existing skill beats a near-duplicate.

```bash
B skill new "weekly-budget-review" --description "Review last week's spending \
  from the bank CSV export and flag categories over budget"
```

Then fill the created file. Write it for a future agent with no memory of today:

- **When to use** — the trigger, concretely.
- **Steps** — numbered, exact commands, paths, flags; the order that worked.
- **Pitfalls** — the dead ends you hit and why they fail.
- **Verification** — how to know it worked.

Keep it under ~150 lines. No secrets, no personal data that belongs in
memory instead (the skill says "the person's bank", memory says which bank).

## Every time you use a learned skill

```bash
B skill log weekly-budget-review --outcome ok
B skill log weekly-budget-review --outcome fail --note "CSV columns changed"
```

## Improving a skill

A failure, a missing step, a better way found, or the person's correction:

```bash
B skill snapshot weekly-budget-review     # saves the current version
# edit the SKILL.md: fix the step, add the pitfall
B skill commit weekly-budget-review --note "handle the new CSV columns"
```

`B brief` flags skills whose last run failed and hasn't been fixed since.
A bad edit: `B skill rollback <name>` (or `--to N`).

## Sharing with the host agent

To have the host load a learned skill natively, link it into the host's
skills folder (Claude Code: `~/.claude/skills`):

```bash
B skill link weekly-budget-review --target ~/.claude/skills
```
