---
name: bastion/skill-writer
version: "3.0.0"
description: >
  Creates and evolves local Bastion skills from an explicit user request,
  with scoped paths, validation, version snapshots, and approval before reload.
triggers:
  - "criar skill"
  - "nova skill"
  - "escrever skill"
  - "quero ensinar"
  - "novo comportamento"
  - "skill personalizada"
  - "/skill-writer"
---

# Skill Writer

## Objective

Turn a recurring, user-approved behavior into a reviewable `SKILL.md`. Bastion
keeps the skill local to the deployment; it does not silently download, publish,
or activate third-party code.

## Required flow

1. Ask what the skill must do, when it should activate, and what output it must
   produce.
2. Search the existing local skills for overlapping behavior. Prefer editing an
   existing skill when that avoids duplicate or conflicting instructions.
3. Choose the scope with the user:
   - global: `skills/{skill-name}/SKILL.md`
   - persona-private: `skills/personas/{persona-slug}/{skill-name}/SKILL.md`
4. Generate frontmatter with `name`, `version`, `description`, and specific
   triggers, followed by objective, instructions, examples, and edge cases.
5. Show the proposed path and meaningful behavioral changes before writing.
6. Validate the document with a real user example. If it fails, revise and test
   again.
7. Keep the previous snapshot available for rollback and reload only after the
   user approves the validated version.

## Safety requirements

- Never install or publish a skill as a side effect of this flow.
- Never overwrite an existing skill without explicit approval and a snapshot.
- Reject path traversal; names and persona slugs are single kebab-case segments.
- Warn when triggers are generic enough to activate accidentally.
- Treat generated skills as instructions, not as permission to bypass Kekkai,
  privacy tiers, tool approvals, or runtime policy.
- In managed deployments, return a reviewable proposal to the control plane
  instead of mutating the worker loadout.

## Minimum skill shape

```markdown
---
name: bastion/example-skill
version: "1.0.0"
description: A precise description of the behavior.
triggers:
  - "specific trigger"
---

# Example Skill

## Objective

What the skill accomplishes.

## Instructions

1. Concrete action.
2. Concrete validation.

## Examples

One input and its expected behavior.

## Edge cases

- Known failure mode and safe response.
```
