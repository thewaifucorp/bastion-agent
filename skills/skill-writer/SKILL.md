---
name: bastion/skill-writer
version: "2.0.0"
description: >
  Guia o usuário na criação de novas skills personalizadas (SKILL.md).
  Verifica o ClawHub antes de criar, escreve o arquivo com estrutura obrigatória,
  salva no caminho correto conforme o escopo e testa com um caso de uso real.
  Também busca e instala skills do repositório awesome-openclaw-skills para personas,
  aplicando política de qualidade e scan de segurança Sage antes de cada instalação.
triggers:
  - "criar skill"
  - "nova skill"
  - "escrever skill"
  - "quero ensinar"
  - "novo comportamento"
  - "skill personalizada"
  - "/skill-writer"
  - "configurar skills para persona"
  - "skills para persona"
  - "instalar skills"
  - "/skills-persona"
---

# Skill Writer — Roteiro de Criação de Skills

## Objective

Guide the user through creating a new skill (SKILL.md) in a conversational way,
ensuring the generated file has a complete structure and is saved in the correct path.

---

## Conversational Flow

### Step 1 — Understand the Need

Ask the three questions below, one at a time, waiting for the answer before proceeding:

1. **What**: "What do you want this skill to do? Describe the expected behavior."
2. **When**: "When should it be activated? What keywords or situations trigger this behavior?"
3. **Output**: "What is the expected result? What should the agent deliver at the end?"

Record the answers as:
- `skill_purpose`: what the skill does
- `skill_triggers`: when it is activated (list of keywords/phrases)
- `skill_output`: what it delivers at the end

---

### Step 2 — Check ClawHub

Before creating a new skill, check if an equivalent one already exists on ClawHub:

1. Search ClawHub for skills with a purpose similar to `skill_purpose`
2. If an equivalent skill is found → go to **Step 3a**
3. If not found → go to **Step 3b**

---

### Step 3a — Equivalent Skill Exists on ClawHub

If an equivalent skill already exists on ClawHub:

1. Present the found skill to the user:
   - Name, description, rating, number of reviews, Verified badge
2. Suggest installation instead of creating a new one:
   > "I found the skill `{name}` on ClawHub that does exactly this (⭐ {rating} · {reviews} reviews · Verified).
   > Would you like me to install it instead of creating a new one?"
3. If the user confirms → install the skill following the installation policy in AGENTS.md
4. If the user prefers to create anyway → continue to **Step 3b**

---

### Step 3b — Create New Skill

If no equivalent exists on ClawHub (or the user preferred to create one):

#### 3b.1 — Define the Scope

Ask the user:
> "Is this skill for exclusive use by a specific persona or for all of Bastion?"

- **Private** (specific persona): save to `personas/{slug}/SKILL.md`
- **Global** (all of Bastion): save to `skills/{name}/SKILL.md`

If the user doesn't know the scope:
> "If the skill only makes sense for the '{active_persona}' persona, it's private.
> If any persona can use it, it's global. Which do you prefer?"

#### 3b.2 — Generate the SKILL.md

Assemble the file with the required structure:

```markdown
---
name: {namespace}/{slug}
version: "1.0.0"
description: >
  {skill_purpose}
triggers:
  - {trigger_1}
  - {trigger_2}
  ...
---

# {Skill Name}

## Objective

{skill_purpose}

## Step-by-Step Instructions

1. {step_1}
2. {step_2}
3. {step_3}
...

## Usage Examples

### Example 1 — {scenario}

**User input:** "{example_input}"

**Expected behavior:**
{example_output}

### Example 2 — {scenario_2}

**User input:** "{example_input_2}"

**Expected behavior:**
{example_output_2}

## Edge Cases

- **Skill already exists locally**: If a file already exists at the destination path, ask the user whether to overwrite or create a new version (e.g., `v2`).
- **User doesn't know the scope**: Explain the difference between private and global and suggest based on the conversation context.
- **Skill with same name on ClawHub**: Warn that the name already exists on ClawHub and suggest an alternative name to avoid future conflicts.
- **Too generic triggers**: If triggers are very common words (e.g., "ok", "yes"), warn that they may cause unwanted activations and suggest more specific triggers.
- **Undefined output**: If the user can't describe the output, ask clarifying questions before proceeding.
```

**Naming rules:**
- For private skills: `name: personas/{slug}/{skill-slug}`
- For global skills: `name: bastion/{skill-slug}` or `name: {namespace}/{skill-slug}`
- The `slug` uses only lowercase letters, numbers, and hyphens

#### 3b.3 — Save to the Correct Path

**Path rule (mandatory):**

| Scope | Path |
|--------|---------|
| Private (specific persona) | `personas/{slug}/SKILL.md` |
| Global (all of Bastion) | `skills/{name}/SKILL.md` |

Where:
- `{slug}` is the persona slug (e.g., `tech-lead`, `entrepreneur`)
- `{name}` is the skill name in kebab-case (e.g., `weekly-review`, `code-reviewer`)

Before saving, confirm with the user:
> "I'll save the skill to `{path}`. Confirm? (yes/no)"

---

### Step 4 — Test with a Real Use Case

After saving the file, trigger the skill with a real use case:

1. Ask the user for a concrete usage example:
   > "To validate the skill, give me a real example of a message that should activate it."
2. Run the skill flow with that input
3. Present the result to the user:
   > "The skill was activated with input '{input}' and produced: {output}"
4. Ask if the result is correct:
   > "Is the result as expected? (yes/no/adjust)"
5. If not correct → adjust the SKILL.md and repeat the test

---

### Step 5 — Publish to ClawHub (Optional)

After successful validation, ask:
> "Could this skill be useful for other Bastion users? Would you like to publish it to ClawHub?"

If the user confirms:
1. Check that the name doesn't conflict with existing ClawHub skills
2. Guide the publishing process:
   - Add `author`, `license`, and `repository` to the frontmatter
   - Create `README.md` with public documentation
   - Submit via `clawhub publish {path}`
3. Inform that the skill will go through review before appearing in the marketplace

---

## Global Edge Cases

### Skill already exists locally

If a file already exists at the destination path:
> "A skill already exists at `{path}`. Would you like to overwrite, create `{name}-v2`, or cancel?"

### User doesn't know the scope

If the user can't define whether the skill is private or global:
1. Explain: "Private skills live inside the persona's folder and only that persona uses them. Global skills live in `skills/` and any persona can use them."
2. Suggest based on context: if the skill uses keywords very specific to the active persona, it's probably private.

### Skill with same name on ClawHub

If the chosen name already exists on ClawHub:
> "The name `{name}` already exists on ClawHub. To avoid future conflicts, I suggest using `{suggestion}`. Accept?"

### Too generic triggers

If triggers include very common words:
> "The trigger '{trigger}' is too generic and may activate the skill in unintended contexts. I suggest using '{more_specific_suggestion}'. Would you like to adjust?"

### User wants to create a skill for another persona

If the user wants to create a private skill for a persona other than the active one:
1. Confirm the target persona's slug
2. Save to `personas/{target-slug}/SKILL.md`
3. Inform that the skill will only be available when that persona is active
