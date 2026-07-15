"""Unit tests for SchemaExtractor."""

import json
import textwrap
from pathlib import Path

import pytest

from output_validator.schema_extractor import SchemaExtractor, generate_schema_from_example


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------

@pytest.fixture
def tmp_skills(tmp_path):
    """Return a temporary skills directory."""
    return tmp_path / "skills"


@pytest.fixture
def extractor(tmp_skills):
    return SchemaExtractor(tmp_skills)


def _make_skill_md(tmp_skills: Path, skill_name: str, content: str) -> Path:
    skill_dir = tmp_skills / skill_name
    skill_dir.mkdir(parents=True, exist_ok=True)
    skill_md = skill_dir / "SKILL.md"
    skill_md.write_text(content, encoding="utf-8")
    return skill_md


# ---------------------------------------------------------------------------
# extract_example_from_skill
# ---------------------------------------------------------------------------

class TestExtractExampleFromSkill:
    def test_valid_example(self, extractor, tmp_skills):
        content = textwrap.dedent("""\
            # My Skill

            ## Output Example
            ```json
            {"status": "ok", "value": 42}
            ```
        """)
        skill_md = _make_skill_md(tmp_skills, "my-skill", content)
        result = extractor.extract_example_from_skill(skill_md)
        assert result == {"status": "ok", "value": 42}

    def test_missing_example_section(self, extractor, tmp_skills):
        content = "# My Skill\n\nNo output example here.\n"
        skill_md = _make_skill_md(tmp_skills, "no-example", content)
        result = extractor.extract_example_from_skill(skill_md)
        assert result is None

    def test_invalid_json_in_example(self, extractor, tmp_skills):
        content = textwrap.dedent("""\
            ## Output Example
            ```json
            {not valid json}
            ```
        """)
        skill_md = _make_skill_md(tmp_skills, "bad-json", content)
        result = extractor.extract_example_from_skill(skill_md)
        assert result is None

    def test_case_insensitive_matching(self, extractor, tmp_skills):
        content = textwrap.dedent("""\
            ## OUTPUT EXAMPLE
            ```json
            {"key": "value"}
            ```
        """)
        skill_md = _make_skill_md(tmp_skills, "case-test", content)
        result = extractor.extract_example_from_skill(skill_md)
        assert result == {"key": "value"}

    def test_multiple_code_blocks_uses_first(self, extractor, tmp_skills):
        content = textwrap.dedent("""\
            ## Output Example
            ```json
            {"first": true}
            ```

            Some text.

            ```json
            {"second": true}
            ```
        """)
        skill_md = _make_skill_md(tmp_skills, "multi-block", content)
        result = extractor.extract_example_from_skill(skill_md)
        assert result == {"first": True}

    def test_missing_skill_md(self, extractor, tmp_skills):
        result = extractor.extract_example_from_skill(tmp_skills / "nonexistent" / "SKILL.md")
        assert result is None

    def test_accepts_directory_path(self, extractor, tmp_skills):
        content = textwrap.dedent("""\
            ## Output Example
            ```json
            {"dir": "test"}
            ```
        """)
        _make_skill_md(tmp_skills, "dir-skill", content)
        result = extractor.extract_example_from_skill(tmp_skills / "dir-skill")
        assert result == {"dir": "test"}

    def test_unreadable_file_oserror(self, extractor, tmp_skills, monkeypatch):
        content = textwrap.dedent("""\
            ## Output Example
            ```json
            {"status": "ok"}
            ```
        """)
        skill_md = _make_skill_md(tmp_skills, "unreadable-skill", content)

        def mock_read_text(*args, **kwargs):
            raise OSError("Mocked permission denied")

        monkeypatch.setattr(Path, "read_text", mock_read_text)

        result = extractor.extract_example_from_skill(skill_md)
        assert result is None


# ---------------------------------------------------------------------------
# generate_schema_for_skill
# ---------------------------------------------------------------------------

class TestGenerateSchemaForSkill:
    def test_generates_and_saves_schema(self, extractor, tmp_skills):
        content = textwrap.dedent("""\
            ## Output Example
            ```json
            {"name": "Alice", "age": 30}
            ```
        """)
        _make_skill_md(tmp_skills, "gen-skill", content)
        schema = extractor.generate_schema_for_skill("gen-skill")

        assert schema is not None
        assert schema["type"] == "object"
        assert "name" in schema["properties"]
        assert "age" in schema["properties"]

        schema_file = tmp_skills / "gen-skill" / "schema.json"
        assert schema_file.exists()
        saved = json.loads(schema_file.read_text())
        assert saved["type"] == "object"

    def test_returns_none_when_no_example(self, extractor, tmp_skills):
        content = "# Skill\n\nNo example.\n"
        _make_skill_md(tmp_skills, "no-ex", content)
        result = extractor.generate_schema_for_skill("no-ex")
        assert result is None

    def test_returns_none_when_skill_md_missing(self, extractor, tmp_skills):
        result = extractor.generate_schema_for_skill("ghost-skill")
        assert result is None


# ---------------------------------------------------------------------------
# generate_schema_from_example (unit)
# ---------------------------------------------------------------------------

class TestGenerateSchemaFromExample:
    def test_string_field(self):
        schema = generate_schema_from_example("hello")
        assert schema["type"] == "string"
        assert schema["minLength"] == 1

    def test_integer_field(self):
        schema = generate_schema_from_example(42)
        assert schema["type"] == "integer"

    def test_float_field(self):
        schema = generate_schema_from_example(3.14)
        assert schema["type"] == "number"

    def test_bool_field(self):
        schema = generate_schema_from_example(True)
        assert schema["type"] == "boolean"

    def test_null_field(self):
        schema = generate_schema_from_example(None)
        assert schema["type"] == "null"

    def test_array_field(self):
        schema = generate_schema_from_example([1, 2, 3])
        assert schema["type"] == "array"
        assert schema["items"]["type"] == "integer"

    def test_object_field(self):
        schema = generate_schema_from_example({"x": 1, "y": "hello"})
        assert schema["type"] == "object"
        assert "x" in schema["properties"]
        assert "y" in schema["properties"]
        assert "x" in schema["required"]

    def test_email_format_detection(self):
        schema = generate_schema_from_example("user@example.com")
        assert schema.get("format") == "email"

    def test_datetime_format_detection(self):
        schema = generate_schema_from_example("2024-01-15T10:30:00Z")
        assert schema.get("format") == "date-time"

    def test_uri_format_detection(self):
        schema = generate_schema_from_example("https://example.com")
        assert schema.get("format") == "uri"
