"""Unit tests for AutoValidator."""

import json
import textwrap
from pathlib import Path

import pytest

from output_validator.auto_validator import AutoValidator, ValidationResult


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _make_skill(tmp_path: Path, skill_name: str, example: dict | None = None) -> Path:
    skill_dir = tmp_path / skill_name
    skill_dir.mkdir(parents=True, exist_ok=True)

    if example is not None:
        content = textwrap.dedent(f"""\
            # {skill_name}

            ## Output Example
            ```json
            {json.dumps(example, indent=2)}
            ```
        """)
    else:
        content = f"# {skill_name}\n\nNo example.\n"

    (skill_dir / "SKILL.md").write_text(content, encoding="utf-8")
    return skill_dir


def _write_schema(skill_dir: Path, schema: dict) -> Path:
    schema_path = skill_dir / "schema.json"
    schema_path.write_text(json.dumps(schema), encoding="utf-8")
    return schema_path


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------

@pytest.fixture
def skills_dir(tmp_path):
    return tmp_path / "skills"


@pytest.fixture
def validator(skills_dir):
    return AutoValidator(skills_dir)


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------

class TestValidateSkillOutput:
    def test_valid_output_against_existing_schema(self, validator, skills_dir):
        skill_dir = _make_skill(skills_dir, "test-skill", {"name": "Alice", "age": 30})
        schema = {
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "age": {"type": "integer"},
            },
            "required": ["name", "age"],
        }
        _write_schema(skill_dir, schema)

        result = validator.validate_skill_output("test-skill", {"name": "Bob", "age": 25})
        assert result.is_valid
        assert result.errors == []

    def test_invalid_output_against_existing_schema(self, validator, skills_dir):
        skill_dir = _make_skill(skills_dir, "strict-skill", {"name": "Alice"})
        schema = {
            "type": "object",
            "properties": {"name": {"type": "string"}},
            "required": ["name"],
        }
        _write_schema(skill_dir, schema)

        result = validator.validate_skill_output("strict-skill", {"name": 123})
        assert not result.is_valid
        assert len(result.errors) > 0

    def test_schema_generation_on_first_run(self, validator, skills_dir):
        _make_skill(skills_dir, "new-skill", {"status": "ok"})

        result = validator.validate_skill_output("new-skill", {}, regenerate=False)
        # First run: schema generated
        assert result.schema_generated
        schema_path = skills_dir / "new-skill" / "schema.json"
        assert schema_path.exists()

    def test_regenerate_flag_forces_new_schema(self, validator, skills_dir):
        skill_dir = _make_skill(skills_dir, "regen-skill", {"v": 1})
        old_schema = {"type": "object", "properties": {"old": {"type": "string"}}}
        _write_schema(skill_dir, old_schema)

        result = validator.validate_skill_output("regen-skill", {}, regenerate=True)
        assert result.schema_generated

    def test_missing_skill_md_returns_warning(self, validator, skills_dir):
        # Skill dir exists but no SKILL.md
        (skills_dir / "ghost").mkdir(parents=True)
        result = validator.validate_skill_output("ghost", {"x": 1})
        assert result.is_valid
        assert result.warnings

    def test_no_output_example_returns_valid_with_warning(self, validator, skills_dir):
        _make_skill(skills_dir, "no-ex-skill", example=None)
        result = validator.validate_skill_output("no-ex-skill", {"anything": True})
        assert result.is_valid
        assert result.warnings

    def test_invalid_json_string_returns_error(self, validator, skills_dir):
        _make_skill(skills_dir, "json-skill", {"x": 1})
        result = validator.validate_skill_output("json-skill", "{not json}")
        assert not result.is_valid
        assert any("JSON" in e for e in result.errors)

    def test_valid_json_string_is_parsed(self, validator, skills_dir):
        skill_dir = _make_skill(skills_dir, "str-skill", {"name": "Alice"})
        schema = {
            "type": "object",
            "properties": {"name": {"type": "string"}},
            "required": ["name"],
        }
        _write_schema(skill_dir, schema)

        result = validator.validate_skill_output("str-skill", '{"name": "Bob"}')
        assert result.is_valid

    def test_schema_caching(self, validator, skills_dir):
        skill_dir = _make_skill(skills_dir, "cache-skill", {"x": 1})
        schema = {"type": "object", "properties": {"x": {"type": "integer"}}, "required": ["x"]}
        _write_schema(skill_dir, schema)

        # First call — cache miss
        validator.validate_skill_output("cache-skill", {"x": 1})
        assert "cache-skill" in validator._schema_cache

        # Second call — cache hit (schema still loaded)
        validator.validate_skill_output("cache-skill", {"x": 2})
        assert "cache-skill" in validator._schema_cache

    def test_regenerate_invalidates_cache(self, validator, skills_dir):
        skill_dir = _make_skill(skills_dir, "inv-skill", {"x": 1})
        schema = {"type": "object"}
        _write_schema(skill_dir, schema)

        validator.validate_skill_output("inv-skill", {"x": 1})
        assert "inv-skill" in validator._schema_cache

        validator.validate_skill_output("inv-skill", {"x": 1}, regenerate=True)
        # After regenerate the cache is repopulated with the new schema
        assert "inv-skill" in validator._schema_cache

    def test_output_too_large_returns_error(self, skills_dir):
        small_validator = AutoValidator(skills_dir, max_output_bytes=10)
        _make_skill(skills_dir, "big-skill", {"x": 1})
        result = small_validator.validate_skill_output("big-skill", "x" * 100)
        assert not result.is_valid
        assert any("exceeds limit" in e for e in result.errors)
