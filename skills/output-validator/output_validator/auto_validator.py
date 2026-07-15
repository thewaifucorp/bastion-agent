"""
Auto Validator — validates LLM outputs against JSON Schema Draft 7.

Generates schemas automatically from SKILL.md ## Output Example sections
when no schema.json exists yet.

Fallback implementation: uses jsonschema directly (no intent-compiler required).
"""

import json
import logging
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Dict, List, Optional

import jsonschema
from jsonschema import Draft7Validator

from .schema_extractor import SchemaExtractor

logger = logging.getLogger(__name__)

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

MAX_OUTPUT_BYTES = 1 * 1024 * 1024   # 1 MB
WARN_OUTPUT_BYTES = 100 * 1024        # 100 KB


# ---------------------------------------------------------------------------
# ValidationResult
# ---------------------------------------------------------------------------

@dataclass
class ValidationResult:
    """
    Result of a skill output validation.

    Attributes:
        is_valid: True if the output conforms to the schema (or no schema exists).
        errors: List of validation error messages.
        warnings: List of non-fatal warning messages.
        schema_generated: True if a new schema was generated during this call.
        schema_path: Path to the schema file used (or generated).

    Example::

        result = validate_skill_output("life-log", output)
        if not result.is_valid:
            for err in result.errors:
                print(f"  - {err}")
    """

    is_valid: bool
    errors: List[str] = field(default_factory=list)
    warnings: List[str] = field(default_factory=list)
    schema_generated: bool = False
    schema_path: Optional[Path] = None

    def __repr__(self) -> str:  # pragma: no cover
        status = "VALID" if self.is_valid else "INVALID"
        parts = [f"ValidationResult({status}"]
        if self.errors:
            parts.append(f", errors={self.errors!r}")
        if self.warnings:
            parts.append(f", warnings={self.warnings!r}")
        if self.schema_generated:
            parts.append(", schema_generated=True")
        if self.schema_path:
            parts.append(f", schema_path={str(self.schema_path)!r}")
        parts.append(")")
        return "".join(parts)


# ---------------------------------------------------------------------------
# AutoValidator
# ---------------------------------------------------------------------------

class AutoValidator:
    """
    Automatic output validator with schema generation and caching.

    Validates LLM outputs against JSON Schema Draft 7. Generates schemas
    automatically from SKILL.md ## Output Example sections when needed.

    Args:
        skills_dir: Root directory containing skill sub-directories.
        max_output_bytes: Maximum allowed output size in bytes (default 1 MB).

    Example::

        validator = AutoValidator(Path("skills"))
        result = validator.validate_skill_output("life-log", output)
        if not result.is_valid:
            logger.error("Validation failed: %s", result.errors)
    """

    def __init__(
        self,
        skills_dir: Path,
        max_output_bytes: int = MAX_OUTPUT_BYTES,
    ) -> None:
        self.skills_dir = Path(skills_dir)
        self.max_output_bytes = max_output_bytes
        self.extractor = SchemaExtractor(self.skills_dir)
        self._schema_cache: Dict[str, Dict[str, Any]] = {}
        logger.debug("AutoValidator initialised (skills_dir=%s)", self.skills_dir)

    # ------------------------------------------------------------------
    # Public API
    # ------------------------------------------------------------------

    def validate_skill_output(
        self,
        skill_name: str,
        output: Any,
        regenerate: bool = False,
    ) -> ValidationResult:
        """
        Validate a skill's LLM output.

        Generates a schema automatically if none exists. Returns a valid result
        with a warning when the skill has no ## Output Example defined.

        Args:
            skill_name: Skill directory name (e.g. ``"life-log"``).
            output: LLM output — dict, list, or JSON string.
            regenerate: Force schema regeneration even if schema.json exists.

        Returns:
            :class:`ValidationResult` with status, errors, and warnings.
        """
        # --- Parse string output ---
        if isinstance(output, str):
            size = len(output.encode("utf-8"))
            size_check = self._check_size(size, skill_name)
            if size_check is not None:
                return size_check
            try:
                output = json.loads(output)
            except json.JSONDecodeError as exc:
                return ValidationResult(
                    is_valid=False,
                    errors=[f"Output is not valid JSON: {exc}"],
                )
        else:
            # Estimate size for non-string outputs
            try:
                serialised = json.dumps(output)
                size = len(serialised.encode("utf-8"))
                size_check = self._check_size(size, skill_name)
                if size_check is not None:
                    return size_check
            except (TypeError, ValueError):
                pass

        schema_path = self.skills_dir / skill_name / "schema.json"

        # --- Invalidate cache on regenerate ---
        if regenerate and skill_name in self._schema_cache:
            del self._schema_cache[skill_name]
            logger.debug("Cache invalidated for '%s'", skill_name)

        # --- Generate schema if missing or regenerate requested ---
        if not schema_path.exists() or regenerate:
            logger.info(
                "Generating schema for '%s' (regenerate=%s)", skill_name, regenerate
            )
            schema = self.extractor.generate_schema_for_skill(skill_name)

            if schema is None:
                return ValidationResult(
                    is_valid=True,
                    warnings=[
                        f"Skill '{skill_name}' has no ## Output Example in SKILL.md "
                        f"— validation skipped"
                    ],
                )

            logger.info("Schema generated for '%s': %s", skill_name, schema_path)
            # Cache the freshly generated schema
            self._schema_cache[skill_name] = schema
            return ValidationResult(
                is_valid=True,
                schema_generated=True,
                schema_path=schema_path,
            )

        # --- Load schema (with cache) ---
        schema = self._load_schema(skill_name, schema_path)
        if schema is None:
            # Corrupted schema.json — try to regenerate
            logger.warning(
                "Corrupted schema.json for '%s', attempting regeneration", skill_name
            )
            regen_schema = self.extractor.generate_schema_for_skill(skill_name)
            if regen_schema is None:
                # Fail-open: don't break the skill
                return ValidationResult(
                    is_valid=True,
                    warnings=[f"Could not load or regenerate schema for '{skill_name}'"],
                )
            self._schema_cache[skill_name] = regen_schema
            schema = regen_schema

        # --- Validate ---
        errors = self._validate(output, schema)
        return ValidationResult(
            is_valid=len(errors) == 0,
            errors=errors,
            schema_path=schema_path,
        )

    # ------------------------------------------------------------------
    # Private helpers
    # ------------------------------------------------------------------

    def _check_size(self, size: int, skill_name: str) -> Optional[ValidationResult]:
        """Return an error ValidationResult if output exceeds size limits."""
        if size > self.max_output_bytes:
            return ValidationResult(
                is_valid=False,
                errors=[
                    f"Output size {size:,} bytes exceeds limit of "
                    f"{self.max_output_bytes:,} bytes"
                ],
            )
        if size > WARN_OUTPUT_BYTES:
            logger.warning(
                "Large output for '%s': %s bytes (>100 KB)", skill_name, f"{size:,}"
            )
        return None

    def _load_schema(
        self, skill_name: str, schema_path: Path
    ) -> Optional[Dict[str, Any]]:
        """Load schema from cache or disk."""
        if skill_name in self._schema_cache:
            logger.debug("Cache hit for '%s'", skill_name)
            return self._schema_cache[skill_name]

        logger.debug("Cache miss for '%s', loading from disk", skill_name)
        try:
            schema = json.loads(schema_path.read_text(encoding="utf-8"))
            self._schema_cache[skill_name] = schema
            return schema
        except (OSError, json.JSONDecodeError) as exc:
            logger.error("Cannot load schema for '%s': %s", skill_name, exc)
            return None

    def _validate(self, output: Any, schema: Dict[str, Any]) -> List[str]:
        """Run JSON Schema Draft 7 validation and return error messages."""
        try:
            validator = Draft7Validator(schema)
            errors = sorted(validator.iter_errors(output), key=lambda e: list(e.path))
            return [e.message for e in errors]
        except jsonschema.SchemaError as exc:
            logger.error("Invalid schema: %s", exc)
            return [f"Schema is invalid: {exc.message}"]
        except Exception as exc:  # pragma: no cover
            logger.error("Unexpected validation error: %s", exc)
            return [f"Validation error: {exc}"]
