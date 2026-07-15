---
name: bastion/agentskills-publish
version: "1.0.0"
description: >
  Publishes a local Bastion skill to agentskills.io by stripping the bastion/ prefix,
  writing publish-ready frontmatter, validating with skills-ref, and guiding the git push.
triggers:
  - /publish-skill
  - "publicar skill"
  - "publish skill"
---

# bastion/agentskills-publish

Guides the user through publishing a local Bastion skill to agentskills.io — handling the
`bastion/` prefix strip (Pitfall 4), required metadata fields, and validation via `skills-ref`.

## Objective

Turn a local `skills/<name>/SKILL.md` into a published agentskills.io entry by transforming
the frontmatter, validating the result, and walking the user through the git push step.

---

## Flow

| Step | Action |
|------|--------|
| 1 | Ask the user which skill to publish: "Which skill do you want to publish? (e.g., `weekly-review`)" |
| 2 | Read `skills/<name>/SKILL.md` and confirm it exists |
| 3 | Strip `bastion/` prefix from the `name` field (see Frontmatter Transform below) |
| 4 | Write the publish-ready frontmatter, preserving `metadata.bastion_name` |
| 5 | Run `skills-ref validate skills/<name>/SKILL.md` — must pass before proceeding |
| 6 | Show the user the git push steps to submit to agentskills.io |
| 7 | Confirm the expected public URL and inform the user of the review timeline |

---

## Frontmatter Transform (Pitfall 4)

Existing Bastion skills use `name: bastion/<slug>`. The agentskills.io registry rejects names
with slashes. The publish action **must** strip the prefix and preserve it in metadata.

**Before (local SKILL.md):**

```yaml
---
name: bastion/my-skill
version: "1.0.0"
description: > Does something useful.
triggers:
  - /my-skill
  - "my trigger phrase"
---
```

**After (publish-ready frontmatter):**

```yaml
---
name: my-skill
version: "1.0.0"
description: > Does something useful.
triggers:
  - /my-skill
  - "my trigger phrase"
metadata:
  bastion_name: bastion/my-skill
  version: "1.0.0"
  triggers:
    - /my-skill
    - "my trigger phrase"
---
```

Key rules:
- `name` field: strip `bastion/` prefix → bare slug (max 64 chars, no slashes)
- `metadata.bastion_name`: preserve the original `bastion/<slug>` for reinstall mapping
- `metadata.version`: copy from the `version` field
- `metadata.triggers`: copy the triggers list so the hub can index by intent
- `description`: max 1024 chars; truncate with `...` if longer

---

## Validation Step

After writing the publish-ready frontmatter, run:

```bash
skills-ref validate skills/<name>/SKILL.md
```

This checks:
- `name` has no slashes (immediate rejection otherwise)
- `description` is within 1024 chars
- Required fields are present

If validation fails, show the error and ask the user to fix before proceeding.

---

## Git Push Steps

After validation passes, guide the user:

1. Fork or clone the agentskills.io skills repository
2. Copy the skill directory to `skills/<name>/`
3. Commit: `git commit -m "add: <name> skill"`
4. Open a pull request — the agentskills.io team reviews within ~48 h
5. Once merged, the skill is available at: `https://agentskills.io/skills/<name>`

---

## Security Notes

Before publishing, check:

- **No secrets in SKILL.md**: API keys, tokens, passwords, and personal data must not appear in the file.
  SKILL.md is public after merge — treat it as public documentation.
- **Privacy tier**: only skills with `privacy_tier: cloud-ok` (or no privacy_tier set) are safe to publish.
  Skills that reference `privacy_tier: local-only` beliefs must not be published — they may leak
  information about the user's local data handling.
- **No hardcoded URLs or internal hostnames**: replace with placeholders (e.g., `https://your-bastion.example.com`).

---

## Edge Cases

- **Skill does not exist**: if `skills/<name>/SKILL.md` is not found, ask the user to check the name and path.
- **Name already on agentskills.io**: warn the user that the name exists; suggest renaming with `-v2` suffix or
  contacting the hub maintainers to claim the existing entry.
- **Description too long**: truncate to 1024 chars and warn the user to review the description for coherence.
- **Non-bastion/ prefix**: if the skill name does not start with `bastion/` (e.g., it is already a bare slug),
  skip the strip step and proceed directly to validation.
