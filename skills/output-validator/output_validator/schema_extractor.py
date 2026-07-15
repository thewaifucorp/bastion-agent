"""
Schema Extractor — parses SKILL.md for ## Output Example sections and generates
JSON Schema Draft 7 from the extracted example.

Fallback implementation (no intent-compiler): uses jsonschema + custom inference.
"""

import json
import logging
import re
from pathlib import Path
from typing import Any, Dict, Optional

logger = logging.getLogger(__name__)


# ---------------------------------------------------------------------------
# Schema generation helpers (fallback — no intent-compiler)
# ---------------------------------------------------------------------------

def _infer_string_constraints(value: str) -> Dict[str, Any]:
    """Infer string constraints from an example value."""
    schema: Dict[str, Any] = {"type": "string"}
    length = len(value)
    if length > 0:
        schema["minLength"] = 1
        schema["maxLength"] = max(length * 2, 64)

    # Format detection
    import re as _re
    if _re.match(r'^[^@]+@[^@]+\.[^@]+$', value):
        schema["format"] = "email"
    elif _re.match(r'^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}', value):
        schema["format"] = "date-time"
    elif _re.match(r'^\d{4}-\d{2}-\d{2}$', value):
        schema["format"] = "date"
    elif _re.match(r'^https?://', value):
        schema["format"] = "uri"
    elif _re.match(
        r'^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$',
        value, _re.IGNORECASE
    ):
        schema["format"] = "uuid"
    return schema


def _infer_number_constraints(value: float | int) -> Dict[str, Any]:
    """Infer number constraints from an example value."""
    if isinstance(value, bool):
        return {"type": "boolean"}
    t = "integer" if isinstance(value, int) else "number"
    schema: Dict[str, Any] = {"type": t}
    if value != 0:
        low = min(value * 0.1, value * 10) if value > 0 else value * 10
        high = max(value * 0.1, value * 10) if value > 0 else value * 0.1
        schema["minimum"] = low
        schema["maximum"] = high
    return schema


def generate_schema_from_example(example: Any) -> Dict[str, Any]:
    """
    Generate a JSON Schema Draft 7 from a Python value.

    This is the fallback implementation used when intent-compiler is not
    available. It infers types and basic constraints from the example.

    Args:
        example: Any JSON-serialisable Python value.

    Returns:
        A JSON Schema Draft 7 dict.
    """
    if example is None:
        return {"type": "null"}

    if isinstance(example, bool):
        return {"type": "boolean"}

    if isinstance(example, int):
        return _infer_number_constraints(example)

    if isinstance(example, float):
        return _infer_number_constraints(example)

    if isinstance(example, str):
        return _infer_string_constraints(example)

    if isinstance(example, list):
        schema: Dict[str, Any] = {"type": "array"}
        if example:
            # Use first item as representative
            schema["items"] = generate_schema_from_example(example[0])
            schema["minItems"] = len(example)
            schema["maxItems"] = len(example) * 2
        return schema

    if isinstance(example, dict):
        schema = {
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "object",
            "properties": {},
            "required": list(example.keys()),
            "additionalProperties": False,
        }
        for key, val in example.items():
            sub = generate_schema_from_example(val)
            # Remove top-level $schema from nested schemas
            sub.pop("$schema", None)
            schema["properties"][key] = sub
        return schema

    # Fallback
    return {}


# ---------------------------------------------------------------------------
# SchemaExtractor
# ---------------------------------------------------------------------------

class SchemaExtractor:
    """
    Extracts JSON examples from SKILL.md files and generates JSON Schema Draft 7.

    Usage::

        extractor = SchemaExtractor(Path("skills"))
        example = extractor.extract_example_from_skill(Path("skills/life-log/SKILL.md"))
        schema  = extractor.generate_schema_for_skill("life-log")
    """

    # Regex: ## Output Example (case-insensitive) followed by ```json ... ```
    _PATTERN = re.compile(
        r'##\s+Output\s+Example\s*\n\s*```json\s*\n(.*?)\n\s*```',
        re.DOTALL | re.IGNORECASE,
    )

    def __init__(self, skills_dir: Path) -> None:
        self.skills_dir = Path(skills_dir)

    # ------------------------------------------------------------------
    # Public API
    # ------------------------------------------------------------------

    def extract_example_from_skill(
        self, skill_path: Path
    ) -> Optional[Dict[str, Any]]:
        """
        Extract the JSON example from a SKILL.md file.

        Args:
            skill_path: Path to the SKILL.md file (or its parent directory).

        Returns:
            Parsed JSON dict/list, or None if no example found.
        """
        skill_md = self._resolve_skill_md(skill_path)
        if skill_md is None:
            return None

        try:
            content = skill_md.read_text(encoding="utf-8")
        except OSError as exc:
            logger.error("Cannot read %s: %s", skill_md, exc)
            return None

        return self._parse_example(content, skill_md)

    def generate_schema_for_skill(
        self, skill_name: str
    ) -> Optional[Dict[str, Any]]:
        """
        Generate and persist a JSON Schema for a skill.

        Reads ``skills/{skill_name}/SKILL.md``, extracts the ``## Output Example``
        block, generates a JSON Schema Draft 7, and saves it to
        ``skills/{skill_name}/schema.json``.

        Args:
            skill_name: Skill directory name (e.g. ``"life-log"``).

        Returns:
            The generated schema dict, or None on failure.
        """
        skill_dir = self.skills_dir / skill_name
        skill_md_path = skill_dir / "SKILL.md"

        if not skill_md_path.exists():
            logger.error("SKILL.md not found: %s", skill_md_path)
            return None

        example = self.extract_example_from_skill(skill_md_path)
        if example is None:
            logger.info(
                "Skill '%s' has no ## Output Example in SKILL.md", skill_name
            )
            return None

        try:
            schema = generate_schema_from_example(example)
        except Exception as exc:  # pragma: no cover
            logger.error("Schema generation failed for '%s': %s", skill_name, exc)
            return None

        # Add a human-readable title
        if isinstance(schema, dict):
            schema["title"] = f"{skill_name} Output Schema"

        schema_path = skill_dir / "schema.json"
        try:
            schema_path.write_text(
                json.dumps(schema, indent=2, ensure_ascii=False),
                encoding="utf-8",
            )
            logger.info("Schema saved to %s", schema_path)
        except OSError as exc:
            logger.error("Cannot write schema to %s: %s", schema_path, exc)
            return None

        return schema

    # ------------------------------------------------------------------
    # Private helpers
    # ------------------------------------------------------------------

    def _resolve_skill_md(self, path: Path) -> Optional[Path]:
        """Return the SKILL.md path, accepting either the file or its parent dir."""
        p = Path(path)
        if p.is_dir():
            candidate = p / "SKILL.md"
        else:
            candidate = p

        if not candidate.exists():
            logger.error("SKILL.md not found: %s", candidate)
            return None
        return candidate

    def _parse_example(
        self, content: str, source: Path
    ) -> Optional[Dict[str, Any]]:
        """Parse the first JSON block from an ## Output Example section."""
        match = self._PATTERN.search(content)
        if not match:
            logger.debug("No ## Output Example found in %s", source)
            return None

        json_str = match.group(1).strip()
        try:
            return json.loads(json_str)
        except json.JSONDecodeError as exc:
            logger.error(
                "Invalid JSON in ## Output Example of %s: %s", source, exc
            )
            return None
