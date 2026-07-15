---
name: bastion/agentskills-install
version: "1.0.0"
description: >
  Installs a skill from agentskills.io (or any GitHub / direct URL) by fetching SKILL.md,
  validating the name for path safety, confirming with the user, and writing to skills/.
triggers:
  - /install-skill
  - "instalar skill"
  - "install skill"
---

# bastion/agentskills-install

Lets the user install any agentskills.io-compatible skill by conversation — no manual file copy needed.
Handles URL resolution, security validation, and confirmation before writing to disk.

## Objective

Fetch a remote `SKILL.md`, validate it for path traversal and privacy risks, confirm with the user,
and write it to `skills/<name>/SKILL.md` so Bastion activates it immediately.

---

## Flow

| Step | Action |
|------|--------|
| 1 | Ask: "Which skill do you want to install? (bare name, GitHub URL, or direct URL)" |
| 2 | Resolve the URL using the URL Resolution table below |
| 3 | Fetch `SKILL.md` from the resolved URL |
| 4 | Read the `name` field from the fetched frontmatter |
| 5 | Validate the name for path traversal (see Security section) |
| 6 | Show the skill description and confirm: "I'll save this skill to `skills/<name>/SKILL.md`. Confirm? (yes/no)" |
| 7 | If confirmed: write the file to `skills/<name>/SKILL.md` |
| 8 | Confirm activation: "Skill `<name>` is now active. Try it with: `<first trigger>`" |

---

## URL Resolution

| Input format | Resolution strategy |
|---|---|
| Bare name (e.g., `reminder`) | Fetch `https://agentskills.io/.well-known/agent-skills/index.json`, find entry by `name == "reminder"`, follow `url` field |
| GitHub URL (e.g., `github.com/user/repo/blob/main/skills/reminder/SKILL.md`) | Transform to raw content URL: `raw.githubusercontent.com/user/repo/main/skills/reminder/SKILL.md` |
| Direct URL (ends in `SKILL.md`) | Fetch verbatim |

If resolution fails or the file is not found, inform the user and ask them to check the name or URL.

---

## Security

**Path traversal prevention (T-06-04-01 — mandatory check):**

The `name` field from the fetched frontmatter is used to construct the local path
`skills/<name>/SKILL.md`. A malicious skill could set `name: ../../etc/passwd` to write
outside the `skills/` directory.

Reject the install if `name` contains any of:
- `..` (directory traversal)
- `/` (path separator — only allowed as the `bastion/` prefix, which is stripped)
- `\` (Windows path separator)

If the name contains a `bastion/` prefix, strip it to get the bare slug before constructing the path.
After stripping, re-validate the bare slug.

Validation pseudocode:
```
slug = name.removePrefix("bastion/")
if ".." in slug or "/" in slug or "\" in slug:
    REJECT — "Skill name contains unsafe characters. Install aborted."
path = "skills/" + slug + "/SKILL.md"
```

**Privacy tier warning:**

If `metadata.privacy_tier == "cloud-only"` is present in the fetched skill, warn the user:
> "This skill is marked `cloud-only`. It may send data to external services. Install anyway? (yes/no)"

**No code execution:**

SKILL.md is a definition file — Bastion reads and interprets it as instructions but never
executes code blocks embedded in the file. This skill does NOT `eval` or execute any content
from the fetched SKILL.md.

---

## Confirmation Pattern

Before writing any file, always show:

> "I'll save the skill to `skills/<name>/SKILL.md`. Confirm? (yes/no)"

If the user says no, abort silently. If a file already exists at that path:

> "A skill already exists at `skills/<name>/SKILL.md`. Overwrite? (yes/no/rename)"

---

## Edge Cases

- **Name not found on agentskills.io index**: inform the user; suggest searching with a different name or providing a direct URL.
- **Fetch fails (network error)**: inform the user; ask them to check connectivity and try again.
- **Frontmatter missing or malformed**: reject the install — "SKILL.md frontmatter is invalid. Cannot install."
- **name field missing from frontmatter**: reject — "Skill has no `name` field. Cannot determine install path."
- **Skill already installed**: ask the user whether to overwrite, skip, or rename (e.g., `<slug>-v2`).
- **Privacy tier cloud-only with user running LocalOnly persona**: warn that the skill's intent conflicts with the active persona's privacy stance.
