---
name: bastion/onboarding
version: 2.0.0
description: >
  Guided initial setup flow for Bastion. Collects user profile, bio, pain points,
  creates personas with structured state/goals, configures TOTP, sets bot identity,
  and generates USER.md and IDENTITY.md. Responds in the user's language.
triggers:
  - "/start"
  - first message from a user without USER.md configured
---

# Skill: bastion/onboarding

Always interact with the user in the language they write in. If `USER.md` has a `language` field, use it; otherwise, detect from the first message.

## Objective

Guide the new user through a complete initial setup flow. At the end, Bastion will be ready to use with:
- A user profile including bio and goals
- Personas with structured current state and specific goals
- Initial TOTP authentication configured
- A bot identity saved to `config/identity/IDENTITY.md`

Triggered automatically when:
- The user sends `/start`
- The user sends any message and `USER.md` does not have `totp_configured: true`

---

## Main Flow

### Step 1 — Welcome and name

```
{locale:welcome}

{locale:ask_name}
```

Wait for response. Store as `user.name`.

**Validation:** name cannot be empty. If empty, repeat the question.

---

### Step 2 — Main occupation

```
{locale:greet_name} {user.name}! 🙌

{locale:ask_occupation}
```

Wait for response. Store as `user.occupation`.

---

### Step 3-1 — Own business

```
{locale:ask_business}
```

Wait for response. Store as `user.has_business` (boolean) and `user.business_description` (string, optional).

---

### Step 3-2 — User bio

```
{locale:ask_bio}
```

Wait for response. Store as `user.user_bio`.

This should be a short free-text paragraph. The user can describe anything they feel is relevant about themselves.

---

### Step 3-3 — Pain points and goals

```
{locale:ask_pain_points}
```

Wait for response. Store as `user.pain_points_and_goals`.

Use this information as context when generating personas in Step 5.

---

### Step 4 — Life areas

```
{locale:ask_life_areas}
```

Wait for response. Parse the list of informed areas. Store as `user.life_areas: list[str]`.

**Validations (see Edge Cases):**
- If list is empty → repeat the question (Edge Case A)
- If there are duplicate areas → deduplicate silently (Edge Case B)

Confirm with the user before proceeding:

```
{locale:confirm_areas}

{numbered list of areas}

{locale:confirm_prompt}
```

If the user requests adjustment, return to area collection. If confirmed, advance.

---

### Step 5 — Automatic persona creation

For **each area** in `user.life_areas`, create a persona automatically:

1. **Generate `slug`** — short, lowercase, no accents, hyphens only. Max 20 characters.
   - Use only the main topic (1–3 words max).
   - Examples: "Saúde e bem-estar" → `saude`, "Negócio / Empreendimento" → `negocio`, "Carreira em Tech" → `carreira`
   - If slug exceeds 20 characters, truncate at the last full word before the limit.
   - Never use descriptive phrases or the full area name as the slug.

2. **Infer `base_weight`** based on implied priority order:
   - First area: `0.8`
   - Second area: `0.7`
   - Third and beyond: `0.6`
   - Minimum: `0.5`

3. **Infer `domains`, `trigger_keywords`, and suggested `clawhub_skills`** from the area name (see inference table below).

4. **Collect per-persona details** — ask the user specifically for each persona:
   ```
   {locale:persona_detail_prompt} "{Area Name}":

   1. {locale:ask_what_to_do}
   2. {locale:ask_current_state}
   3. {locale:ask_specific_goals}
   ```
   Store as:
   - `persona.description`: what the user wants the persona to do
   - `persona.current_state`: current situation in this life area
   - `persona.specific_goals`: measurable objectives for this area

5. Create `personas/{slug}/SOUL.md` with complete YAML frontmatter.

**Inference table by area:**

| Area (contains) | domains | trigger_keywords | suggested clawhub_skills |
|---|---|---|---|
| work / career | `["work", "career"]` | `["meeting", "task", "project", "deadline", "delivery"]` | `google-calendar`, `notion-tasks` |
| business / entrepreneurship | `["business", "entrepreneurship"]` | `["client", "sale", "revenue", "product", "startup"]` | `google-calendar`, `notion-tasks`, `web-search` |
| health / wellness | `["health", "wellness"]` | `["workout", "diet", "sleep", "doctor", "exercise"]` | `web-search` |
| family | `["family"]` | `["family", "child", "spouse", "home", "family commitment"]` | `google-calendar` |
| finance | `["finance", "money"]` | `["expense", "investment", "bill", "budget", "money"]` | `web-search` |
| study / learning | `["learning", "education"]` | `["study", "course", "book", "class", "exam"]` | `web-search`, `notion-tasks` |
| personal projects | `["personal-projects"]` | `["project", "idea", "hobby", "creation"]` | `github-integration`, `notion-tasks` |
| relationships | `["relationships"]` | `["friend", "relationship", "social", "meeting"]` | `google-calendar` |

For unmapped areas: `domains: ["{slug}"]`, `trigger_keywords: ["{area name}"]`, `clawhub_skills: []`.

**Format of `personas/{slug}/SOUL.md`:**

```yaml
---
name: "{Area Name}"
slug: "{slug}"
base_weight: {value}
current_weight: {same as base_weight}
domains: [...]
trigger_keywords: [...]
clawhub_skills: [...]
current_state: "{user's current state in this area}"
specific_goals: "{user's measurable goals for this area}"
---

You are the {Area Name} persona of {user.name}.

Your domain is {domain description}. Respond in a {default tone: direct and practical} manner.
Focus on helping with tasks, decisions, and information related to {domain}.

## Context

**What I do:** {persona.description}
**Current state:** {persona.current_state}
**Goals:** {persona.specific_goals}
```

Report progress to the user:

```
✅ {locale:creating_personas}

{for each persona created}
• {Area Name} ({slug}) — {locale:created}
```

---

### Step 6 — ClawHub skill suggestion and installation

For each persona created with non-empty `clawhub_skills`:

```
{locale:skill_suggestion_prompt} "{Area Name}":

{list of suggested skills with brief description}

{locale:skill_install_prompt}
```

- "yes": install all listed skills (check Verified badge + rating ≥ 4.0 + 50+ reviews per AGENTS.md policy).
- "no": skip, set `clawhub_skills: []` in the persona's SOUL.md.
- "choose": list each skill individually for confirmation.

---

### Step 7 — Initial weight configuration

Display the inferred weights summary:

```
{locale:weights_summary}

{list: Area Name → weight}

{locale:weights_explanation}

{locale:weights_adjust_prompt}
```

- "yes": for each persona, ask for the new weight (0.0 to 1.0). Validate it is in range.
- "no": keep inferred weights.

Persist weights in `USER.md` and in each `personas/{slug}/SOUL.md`.

---

### Step 8 — TOTP Setup

```
{locale:totp_intro}
```

If "no": inform the user that TOTP can be configured later with `/setup-totp`. Proceed to Step 9.

If "yes":

1. Generate TOTP secret: `python3 ~/.openclaw/workspace/skills/onboarding/totp.py generate`
2. Generate QR URI: `python3 ~/.openclaw/workspace/skills/onboarding/totp.py qr <secret> <user.name>`
3. Render the QR code.
4. Send to the user with instructions to scan and enter the 6-digit code.
5. Validate: `python3 ~/.openclaw/workspace/skills/onboarding/totp.py verify <secret> <code>`
   - Valid (output OK): proceed to Step 9.
   - Invalid (output FAIL): see Edge Case C.

6. Save the secret **only** in `.env` as `BASTION_TOTP_SECRET`. Never in USER.md or any versioned file.

---

### Step 9 — Bot identity

```
{locale:ask_identity_name}

{locale:ask_identity_behavior}
```

Store as:
- `identity.bot_name`: how the user wants to call the bot
- `identity.base_behavior`: personality and tone for default mode

Save to `config/identity/IDENTITY.md`:

```markdown
---
bot_name: "{identity.bot_name}"
base_behavior: "{identity.base_behavior}"
configured_at: "{ISO 8601 timestamp}"
---

# Bot Identity

**Name:** {identity.bot_name}
**Base behavior:** {identity.base_behavior}

This identity is active when no persona is loaded. Active personas override this identity.
```

---

### Step 10 — Generate USER.md

Generate the `USER.md` file with the complete profile:

```yaml
---
name: "{user.name}"
language: "{detected language}"
timezone: "{TIMEZONE from .env}"
occupation: "{user.occupation}"
has_business: {true/false}
business_description: "{user.business_description or ''}"
user_bio: "{user.user_bio}"
pain_points_and_goals: "{user.pain_points_and_goals}"
authorized_user_ids:
  - "{current session user ID}"
totp_configured: {true/false}
personas:
{for each persona}
  - slug: "{slug}"
    name: "{Area Name}"
    base_weight: {value}
    current_weight: {value}
onboarding_completed_at: "{ISO 8601 timestamp}"
---

# {user.name}'s Profile

**Occupation:** {user.occupation}
{if has_business: **Business:** {business_description}}

**About:** {user.user_bio}

**Goals and challenges:** {user.pain_points_and_goals}

## Active Personas

{list of personas with name, slug and weight}

## Configuration

- TOTP: {configured / not configured}
- Bot name: {identity.bot_name}
- Timezone: {timezone}
- Onboarding completed at: {date}
```

**Important:** Use a YAML library (e.g., `ruamel.yaml`) to write the frontmatter — never string concatenation. This prevents YAML corruption.

---

### Step 11 — Completion message

```
🎉 {locale:completion} {user.name}!

{locale:your_personas}

{numbered list: Area Name (slug) — current weight}

{locale:completion_instructions}
```

---

## Edge Cases

### Edge Case A — User provides 0 life areas

**Situation:** The user responds to Step 4 with an empty message or content with no identifiable area.

**Behavior:** Ask again until at least 1 valid area is received. Do not advance to Step 5 with an empty list.

---

### Edge Case B — Duplicate area

**Situation:** The user informs the same area more than once (e.g., "Work, work, Health").

**Behavior:** Deduplicate silently before showing the confirmation list. Use case-insensitive comparison after normalization (remove accents, trim).

Semantically similar but textually different areas (e.g., "Health" and "Wellness") are **not** deduplicated — create separate personas.

---

### Edge Case C — Invalid TOTP code

**Situation:** The user enters an incorrect TOTP code in Step 8.

**Behavior:** Show the QR code again and ask for a new code. Repeat until the user confirms successfully or types `/cancel`.

If `/cancel`: inform that TOTP was not configured and can be set up later with `/setup-totp`. Proceed to Step 9 with `totp_configured: false`.

**No attempt limit during onboarding** — the user can try as many times as needed. The limit (`BASTION_MAX_AUTH_ATTEMPTS`) applies only to normal authentication sessions.

---

## Implementation Notes

- TOTP secret must be saved **exclusively** in `.env` as `BASTION_TOTP_SECRET`. Never in USER.md.
- `authorized_user_ids` in USER.md can only be **added to** by onboarding (current session user ID). Never remove existing IDs.
- All `personas/{slug}/SOUL.md` files must be created before advancing to Step 6.
- Onboarding is idempotent: if interrupted and restarted, detect current state and resume from the pending step.
- After onboarding, the `bastion/persona-engine` skill takes over continuous persona management.
- Slugs must be validated: lowercase, hyphens only, no accents, max 20 characters. Reject slugs that fail this rule and re-generate.
- `USER.md` frontmatter must be written with a proper YAML library, not string formatting.
- The language injected into this prompt at runtime: `Always respond and interact with the user in {user_language}`.
