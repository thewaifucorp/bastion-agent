---
name: bastion/output-validator
version: 1.0.0
description: >
  Automatic output validation for Bastion skills. Generates JSON Schema Draft 7
  from ## Output Example sections in SKILL.md files and validates LLM outputs
  against those schemas at runtime. Tracks validation metrics and detects drift.
triggers:
  - after any LLM output that should be validated
  - on skill installation (schema generation)
  - via CLI for manual validation and stats
---

# Skill: bastion/output-validator

## Overview

The output-validator automatically validates structured LLM outputs against
JSON Schema Draft 7. It generates schemas from examples defined in SKILL.md
files — zero manual schema writing required.

Key features:
- Auto-generates schemas from `## Output Example` sections in SKILL.md
- Validates outputs at runtime with detailed error messages
- Tracks success rates per skill and detects drift below 90%
- CLI for manual validation, schema regeneration, and stats

---

## Installation

The skill is bundled with Bastion. No extra installation needed.

Dependencies (already in the Bastion environment):
- `jsonschema>=4.0.0`
- `click>=8.0.0`
- `hypothesis>=6.0.0` (for tests only)

---

## Usage

### Python API

```python
from output_validator import validate_skill_output

# Validate a skill output (schema auto-generated on first call)
result = validate_skill_output("life-log", output)

if not result.is_valid:
    logger.error("Validation failed: %s", result.errors)

# Disable metrics tracking
result = validate_skill_output("life-log", output, track_metrics=False)
```

### CLI

```bash
# Validate an output file
python -m output_validator.cli validate life-log output.json

# Regenerate schema from SKILL.md
python -m output_validator.cli regenerate life-log

# Show validation stats
python -m output_validator.cli stats
python -m output_validator.cli stats life-log

# Show dashboard (all skills, colour-coded)
python -m output_validator.cli dashboard
```

---

## How to Add Validation to an Existing Skill

### Step 1 — Add `## Output Example` to SKILL.md

```markdown
## Output Example
```json
{
  "entry": "Had a productive morning working on the API.",
  "mood": "good",
  "tags": ["work", "api"],
  "timestamp": "2024-01-15T10:30:00Z"
}
```
```

### Step 2 — Add validation call in your skill code

```python
from output_validator import validate_skill_output

# After getting LLM output:
result = validate_skill_output("your-skill-name", llm_output)
if not result.is_valid:
    logger.warning("Output validation failed: %s", result.errors)
    # Handle gracefully — don't crash the skill
```

### Step 3 — First run generates the schema automatically

On the first call, `schema.json` is created in `skills/your-skill-name/`.
You can inspect and manually adjust it if needed.

---

## ValidationResult

```python
@dataclass
class ValidationResult:
    is_valid: bool           # True if output conforms to schema
    errors: List[str]        # Validation error messages
    warnings: List[str]      # Non-fatal warnings
    schema_generated: bool   # True if schema was just generated
    schema_path: Optional[Path]  # Path to schema.json used
```

---

## Troubleshooting

**Schema not generated / "no ## Output Example"**
- Check that your SKILL.md has a `## Output Example` section with a valid JSON block.
- The JSON block must use triple backticks with `json` language tag.

**Validation fails unexpectedly**
- Run `python -m output_validator.cli regenerate skill-name` to regenerate the schema.
- Check `skills/skill-name/schema.json` — you can manually adjust constraints.

**Drift warning in logs**
- Success rate dropped below 90% in the last 100 validations.
- Check recent errors: `python -m output_validator.cli stats skill-name`
- The LLM may be generating outputs that don't match the expected schema.

**Output too large error**
- Default limit is 1 MB. For larger outputs, instantiate `AutoValidator` directly:
  ```python
  from output_validator.auto_validator import AutoValidator
  validator = AutoValidator(Path("skills"), max_output_bytes=5 * 1024 * 1024)
  ```

---

## Output Example

```json
{
  "skill": "output-validator",
  "version": "1.0.0",
  "status": "healthy",
  "total_skills_tracked": 3,
  "skills_with_drift": []
}
```
