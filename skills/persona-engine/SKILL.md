---
name: bastion/persona-engine
version: 1.0.0
description: >
  Creation, editing and matching of Bastion personas. Conducts conversational
  flow to create new personas, generates personas/{slug}/SOUL.md file with
  mandatory YAML frontmatter, and executes matching algorithm to identify which
  persona (or personas) should be active for each received message.
triggers:
  - "/nova-persona" or "/new-persona"
  - "/criar-persona" or "/create-persona"
  - "/editar-persona" or "/edit-persona"
  - user message requesting to create, edit or list personas
  - internal call from bastion/onboarding during automatic persona creation
---

# Skill: bastion/persona-engine

## Objective

Manage the complete lifecycle of personas: creation via conversational flow,
persistence in `personas/{slug}/SOUL.md`, and real-time matching to determine
which persona (or simultaneous personas) should respond to each message.

---

## Part 1 — Persona Creation (Persona Builder)

### When to trigger

- User sends `/nova-persona` or `/criar-persona` or `/new-persona` or `/create-persona`
- User explicitly requests to create a new persona ("I want to create a persona for X")
- Internal call from `bastion/onboarding` for each informed life area

### Creation Flow

#### Step 1 — Persona Name

Send to user:

```
{locale:create_intro}

{locale:ask_name}
```

Wait for response. Store as `persona.name`.

**Validation:** name cannot be empty. If empty, repeat the question.

---

#### Step 2 — Domain

Send:

```
{locale:ask_domain}
```

Wait for response. Parse response into a list of domains.
Store as `persona.domains: list[str]`.

---

#### Step 3 — Voice Tone

Send:

```
{locale:ask_voice_tone}
```

Wait for response. Store as `persona.voice_tone: str` (free-form descriptive text).

---

#### Step 4 — Activation Keywords

Send:

```
{locale:ask_keywords}
```

Wait for response. Parse into list. Store as `persona.trigger_keywords: list[str]`.

**Validation:** must have at least 1 keyword. If empty, repeat the question.

---

#### Step 5 — ClawHub Skills

Send:

```
{locale:ask_skills}
```

Wait for response. Store as `persona.clawhub_skills: list[str]`.

If user confirms skills, verify each one according to security policy
(Verified badge + rating ≥ 4.0 + 50+ reviews) before installing.

**Skill inference table by domain:**

| Domain contains | Suggested skills |
|---|---|
| code / software / dev / tech | `github-integration`, `code-review-helper`, `jira-tasks` |
| business / entrepreneurship / startup | `google-calendar`, `notion-tasks`, `web-search` |
| health / wellness / fitness | `web-search` |
| family / home | `google-calendar` |
| finance / money / investment | `web-search` |
| study / learning / education | `web-search`, `notion-tasks` |
| projects / creation / hobby | `github-integration`, `notion-tasks` |
| marketing / social media / content | `web-search`, `notion-tasks` |
| schedule / calendar / meetings | `google-calendar` |

For unmapped domains: suggest `web-search` as the minimum default.

---

#### Step 6 — Base Weight

Send:

```
{locale:ask_base_weight}
```

Wait for response. Validate it's a number in range [0.0, 1.0].
Store as `persona.base_weight: float`.

**Validation:** if out of range or non-numeric, inform and repeat the question.

---

#### Step 7 — Confirmation and Generation

Display summary for confirmation:

```
{locale:summary}
```

- If "no" or adjustment request: ask which step to review and return to corresponding step.
- If "yes": execute generation (see section below) and install confirmed skills.

---

### SOUL.md Generation

After confirmation, create the file `personas/{slug}/SOUL.md`:

**Path:** `personas/{persona.slug}/SOUL.md`

**Content:**

```yaml
---
name: "{persona.name}"
slug: "{persona.slug}"
base_weight: {persona.base_weight}
current_weight: {persona.base_weight}
domains: {persona.domains}
trigger_keywords: {persona.trigger_keywords}
clawhub_skills: {persona.clawhub_skills}
voice_tone: "{persona.voice_tone}"
created_at: "{ISO 8601 timestamp}"
---

You are the {persona.name} persona.

Your domain covers: {persona.domains in natural language}.
Voice tone: {persona.voice_tone}.

Focus on helping with tasks, decisions, and information related to your domain.
Maintain consistency with the defined voice tone in all responses.
```

After creating the file, inform the user:

```
{locale:created}
```

Update `USER.md` adding the new persona to the `personas` list.

---

### Slug Generation

Rules for generating the `slug` from `persona.name`:

1. Convert to lowercase
2. Remove accents and special characters (Unicode NFKD normalization)
3. Replace spaces and separators with hyphens
4. Remove characters that are not letters, numbers, or hyphens
5. Remove duplicate hyphens
6. Remove leading and trailing hyphens

Examples:
- `"Tech Lead"` → `tech-lead`
- `"Saúde & Bem-estar"` → `saude-bem-estar`
- `"Pai de Família"` → `pai-de-familia`
- `"Dev/Arquiteto"` → `dev-arquiteto`

**Uniqueness check:** if a folder `personas/{slug}/` already exists, add a numeric suffix (`-2`, `-3`, etc.).

---

## Part 2 — Mandatory YAML Frontmatter for SOUL.md

Every `personas/{slug}/SOUL.md` file generated by this skill **must** contain the following fields in the YAML frontmatter:

| Campo | Tipo | Descrição |
|---|---|---|
| `name` | `string` | Nome legível da persona (ex: `"Tech Lead"`) |
| `slug` | `string` | Identificador único em kebab-case (ex: `"tech-lead"`) |
| `base_weight` | `float` | Peso fixo definido na criação, intervalo [0.0, 1.0] |
| `current_weight` | `float` | Peso dinâmico atual; inicializado igual ao `base_weight` |
| `domains` | `list[str]` | Áreas de conhecimento e atuação da persona |
| `trigger_keywords` | `list[str]` | Palavras-chave que ativam esta persona no matching |
| `clawhub_skills` | `list[str]` | Skills do ClawHub instalados para esta persona |

Campos adicionais opcionais (não obrigatórios, mas recomendados):

| Campo | Tipo | Descrição |
|---|---|---|
| `voice_tone` | `string` | Descrição do tom de voz |
| `active_hours` | `object` | Janela de horário preferencial (ver Parte 3) |
| `created_at` | `string` | Timestamp ISO 8601 de criação |

**Exemplo completo de frontmatter válido:**

```yaml
---
name: "Tech Lead"
slug: "tech-lead"
base_weight: 0.9
current_weight: 0.9
domains:
  - code
  - architecture
  - team
trigger_keywords:
  - PR
  - review
  - deploy
  - bug
  - arquitetura
  - refactor
  - código
clawhub_skills:
  - github-integration
  - code-review-helper
  - jira-tasks
voice_tone: "técnico, direto, com exemplos de código quando relevante"
active_hours:
  start: "09:00"
  end: "18:00"
  timezone: "America/Sao_Paulo"
created_at: "2025-01-15T10:30:00-03:00"
---
```

---

## Part 3 — Matching Algorithm

The matching is executed by the orchestrator on every received message, before formulating the response.

### Inputs

- `message`: text of the received message
- `personas`: list of all active personas (read from `USER.md` + respective `SOUL.md`)
- `current_time`: current time (for time-of-day matching)

### Output

- `active_personas`: list of active personas for this message, each with its `current_weight`
- If list is empty after matching: apply fallback (see Step 4)

---

### Step 1 — Keyword Matching

For each persona, check if any of its `trigger_keywords` appear in the message.

**Rules:**
- Case-insensitive comparison
- Partial matching is valid: keyword `"deploy"` activates if the message contains `"deployar"` or `"deployed"`
- Basic stemming: remove common suffixes before comparing (optional, improves recall)

**Result:** list of personas with at least 1 matching keyword → `keyword_matches: list[Persona]`

---

### Step 2 — Semantic Analysis

For personas that were **not** captured by keyword matching, evaluate whether the semantic context of the message is relevant to the persona's domain.

**How to evaluate:**
- Compare message content with the persona's `domains`
- Use the LLM to classify semantic relevance (score 0.0–1.0)
- Minimum threshold for semantic activation: `0.6`

**Result:** additional list of semantically activated personas → `semantic_matches: list[Persona]`

Combine: `candidates = keyword_matches ∪ semantic_matches`

---

### Step 3 — Time-of-Day Filter (if configured)

For each persona in `candidates`, check if it has `active_hours` configured in SOUL.md.

**If `active_hours` is defined:**
- Convert `current_time` to the persona's timezone
- If the current time is **outside** the `active_hours.start`–`active_hours.end` window:
  - Reduce the persona's `current_weight` by 30% for this matching
  - Do not remove from the list — only penalize the weight

**If `active_hours` is not defined:** no time-based weight adjustment.

---

### Step 4 — Simultaneous Activation of Multiple Personas

All personas in `candidates` are activated **simultaneously**.

Each active persona contributes with its `current_weight` (adjusted by the time filter if applicable).

**No limit on simultaneous personas** — if 3 personas have matching keywords, all 3 are activated.

The orchestrator uses `current_weight` to weight each persona's influence on the final response.

**Result:** `active_personas = candidates` with their respective `current_weight`

---

### Step 5 — Fallback

**Fallback condition:** `candidates` is empty after Steps 1, 2, and 3.

**Behavior:**
1. Select the persona with the highest `current_weight` among all active personas
2. In case of tie: select the persona with the highest `base_weight`
3. In case of persistent tie: select the most recently created persona (`created_at`)

**Result:** `active_personas = [persona_with_highest_weight]`

The fallback guarantees there will always be at least one active persona to respond.

---

### Pseudocódigo do Algoritmo Completo

```python
def match_personas(message: str, personas: list[Persona], current_time: datetime) -> list[ActivePersona]:
    # Step 1: keyword matching
    keyword_matches = [
        p for p in personas
        if any(kw.lower() in message.lower() for kw in p.trigger_keywords)
    ]

    # Step 2: semantic matching for personas not captured by keywords
    remaining = [p for p in personas if p not in keyword_matches]
    semantic_matches = [
        p for p in remaining
        if semantic_relevance(message, p.domains) >= 0.6
    ]

    candidates = keyword_matches + semantic_matches

    # Step 3: time-of-day weight adjustment
    active_personas = []
    for persona in candidates:
        weight = persona.current_weight
        if persona.active_hours:
            if not is_within_active_hours(current_time, persona.active_hours):
                weight = weight * 0.7  # 30% penalty
        active_personas.append(ActivePersona(persona=persona, weight=weight))

    # Step 4: return all active personas simultaneously
    if active_personas:
        return active_personas

    # Step 5: fallback — persona with highest current_weight
    fallback = max(
        personas,
        key=lambda p: (p.current_weight, p.base_weight, p.created_at)
    )
    return [ActivePersona(persona=fallback, weight=fallback.current_weight)]
```

---

## Part 4 — Persona Editing

### When to trigger

- User sends `/editar-persona` or `/edit-persona` or requests to edit an existing persona

### Flow

1. List existing personas for the user to choose from
2. Ask which field to edit (name, domains, voice tone, keywords, skills, base weight)
3. Run the corresponding creation flow step for the chosen field
4. Confirm the change
5. Update `personas/{slug}/SOUL.md` with the new value
6. If the name was changed: generate new slug, create new folder, move files, update `USER.md`

---

## Edge Cases

### Edge Case A — Slug already exists

**Situation:** User tries to create a persona with a name that generates an already existing slug.
(e.g., `personas/tech-lead/` already exists and the user wants to create "Tech Lead 2")

**Behavior:**
- Generate slug with suffix: `tech-lead-2`
- Inform the user: `"A persona with slug 'tech-lead' already exists. The new persona will be created as 'tech-lead-2'."`
- Proceed normally with the adjusted slug

---

### Edge Case B — No personas registered (fallback impossible)

**Situation:** The matching algorithm tries to apply the fallback, but no personas are registered.

**Behavior:**
- Respond with Bastion's base personality (root SOUL.md)
- Suggest the user create their first persona using the `{locale:no_personas_fallback}` message

---

### Edge Case C — Too generic keyword

**Situation:** The user defines an extremely generic keyword (e.g., "a", "o", "de", "e") that would activate the persona in almost every message.

**Behavior:**
- Detect keywords with fewer than 3 characters or that are common stopwords in Portuguese/English
- Warn the user using `{locale:generic_keyword_warning}`
- If "yes": accept and record the warning in SOUL.md as a comment
- If "no": remove the keyword and ask for a replacement

---

### Edge Case D — Base weight out of range

**Situation:** User types an invalid value for the base weight (e.g., "1.5", "-0.1", "high").

**Behavior:**

```
{locale:invalid_weight}
```

Repeat until a valid value is received.

---

### Edge Case E — Multiple personas with same weight in fallback

**Situation:** Two or more personas have exactly the same `current_weight` and `base_weight` at the time of fallback.

**Behavior:**
- Use `created_at` as the final tiebreaker: most recently created persona has priority
- If `created_at` is also equal (unlikely): use alphabetical order of slug

---

### Edge Case F — Persona without keywords activated only semantically

**Situation:** A persona has `trigger_keywords: []` (empty list) and can only be activated by semantic analysis.

**Behavior:**
- Allow creation (empty keywords are valid)
- Warn the user during creation that this persona will only be activated by semantic analysis and may result in less precise activations
- In matching, skip Step 1 for this persona and go directly to Step 2

---

## Output Example

```json
{
  "name": "Tech Lead",
  "slug": "tech-lead",
  "base_weight": 0.9,
  "current_weight": 0.9,
  "domains": ["code", "architecture", "team"],
  "trigger_keywords": ["PR", "review", "deploy", "bug", "arquitetura"],
  "clawhub_skills": ["github-integration", "code-review-helper"],
  "voice_tone": "técnico, direto, com exemplos de código quando relevante",
  "created_at": "2024-01-15T10:30:00Z"
}
```

---

## Implementation Notes

- `current_weight` is initialized equal to `base_weight` at creation and managed by the `bastion/weight-system` skill after that. The `persona-engine` does not alter `current_weight` directly — it only reads it for matching and fallback.
- Matching is executed by the orchestrator before each response. The result (`active_personas`) is injected into the response context.
- Personas created during onboarding (`bastion/onboarding`) follow the same SOUL.md format defined here. Onboarding calls this skill internally to ensure consistency.
- `USER.md` must be updated whenever a persona is created, edited, or removed — keeping the `personas` list in sync with the folders under `personas/`.
- ClawHub skills installed for a persona are recorded in `personas/{slug}/skills.json` in addition to the SOUL.md frontmatter.
