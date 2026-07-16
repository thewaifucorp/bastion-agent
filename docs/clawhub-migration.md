# ClawHub Migration Guide

**Version:** 1.0 · **Phase:** ECO-02 · **Decision:** D-09

ClawHub skills use a SKILL.md format compatible with Bastion. Migration is a frontmatter
field rename + validation step — no reimplementation required for most skills.

---

## Overview

Bastion reads skills from `skills/<name>/SKILL.md`. ClawHub skills follow the same format
with minor field name differences. To migrate a ClawHub skill:

1. Copy SKILL.md to the local `skills/` directory
2. Rename any divergent frontmatter fields to the Bastion equivalents
3. Validate with `skills-ref validate`
4. Test by typing the trigger phrase

---

## Migration Steps

### Step 1: Copy the skill file

```bash
mkdir -p skills/<name>
cp ~/Downloads/<clawhub-skill>/SKILL.md skills/<name>/SKILL.md
```

Or fetch directly from a ClawHub URL:

```bash
curl -o skills/<name>/SKILL.md https://clawhub.io/skills/<name>/SKILL.md
```

### Step 2: Rename divergent frontmatter fields

Open `skills/<name>/SKILL.md` and apply the field mapping below:

| ClawHub field | Bastion field | Notes |
|---|---|---|
| `skill_name` | `name` | Use `bastion/<slug>` format |
| `version` | `version` | Keep as-is (semver string) |
| `about` | `description` | Rename the key; content unchanged |
| `keywords` | `triggers` | Rename; keep the list values |
| `author_triggers` | `triggers` | Merge into `triggers` if present |
| *(absent)* | `metadata.privacy_tier` | Add if the skill accesses external services |

If the ClawHub skill already uses `name`, `description`, and `triggers` — no rename needed.

### Step 3: Validate

```bash
skills-ref validate skills/<name>/SKILL.md
```

Expected output for a passing skill:

```
✓ name: reminder (valid — no slashes, ≤64 chars)
✓ description: 48 chars (within 1024 limit)
✓ triggers: 3 entries found
✓ Validation passed
```

If validation fails, the error message identifies the failing field. Fix it and re-run.

### Step 4: Test in Bastion

Type one of the trigger phrases in your Bastion chat. Verify that the skill activates and
produces the expected output. If the behavior is wrong, check the SKILL.md instructions section.

---

## Compatibility Notes

Bastion adds two fields not present in ClawHub skills:

| Field | Default if absent | Effect |
|---|---|---|
| `metadata.privacy_tier` | `cloud-ok` | Skills without this field are treated as cloud-safe. If the skill handles sensitive data, add `metadata.privacy_tier: local-only` explicitly. |
| `triggers` | *(required)* | ClawHub skills using `keywords` must rename the field; otherwise the skill will not activate on any phrase. |

ClawHub skills without `privacy_tier` default to `cloud-ok` in Bastion — this means beliefs
matching the skill may be sent to cloud providers. Review each migrated skill and add
`metadata.privacy_tier: local-only` for any skill that should not reach cloud APIs.

---

## Worked Example: Migrating a Reminder Skill from ClawHub

### Before (ClawHub frontmatter)

```yaml
---
skill_name: reminder
version: "1.2.0"
about: >
  Creates and manages reminders with natural language.
  Supports recurring reminders and timezone awareness.
keywords:
  - lembrete
  - reminder
  - me lembra
  - agendar lembrete
author: clawhub-team
license: MIT
---
```

### After (Bastion frontmatter)

```yaml
---
name: bastion/reminder
version: "1.2.0"
description: >
  Creates and manages reminders with natural language.
  Supports recurring reminders and timezone awareness.
triggers:
  - lembrete
  - reminder
  - me lembra
  - agendar lembrete
metadata:
  privacy_tier: local-only
---
```

Changes made:
- `skill_name` → `name` (prefixed with `bastion/`)
- `about` → `description`
- `keywords` → `triggers`
- Added `metadata.privacy_tier: local-only` (reminders are personal data; keep local)
- Removed `author` and `license` (not required by Bastion; can keep as custom metadata)

### Validate the migrated skill

```bash
skills-ref validate skills/reminder/SKILL.md
```

```
✓ name: bastion/reminder (valid)
✓ description: 94 chars (within 1024 limit)
✓ triggers: 4 entries found
✓ Validation passed
```

### Test in Bastion

Type: `me lembra de ligar para o João amanhã às 10h`

Expected: Bastion activates the reminder skill, creates a reminder entry, and confirms:
> "Lembrete criado: ligar para o João — amanhã, 10:00."

---

## Batch Migration

To migrate all ClawHub skills in a directory:

```bash
for dir in ~/clawhub-skills/*/; do
  name=$(basename "$dir")
  mkdir -p "skills/$name"
  cp "$dir/SKILL.md" "skills/$name/SKILL.md"
  skills-ref validate "skills/$name/SKILL.md" || echo "FAILED: $name"
done
```

Review each `FAILED` entry manually for field renaming. Skills that pass can be tested immediately.

---

## Outcome (D-09)

ClawHub migration is a documented and validated path. At least one skill (reminder) has been
migrated through the full workflow: copy → rename → `skills-ref validate` → test. Additional
ClawHub skills follow the same steps without modifications to the Bastion core.
