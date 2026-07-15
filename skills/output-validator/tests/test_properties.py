"""
Property-based tests for the Output Validator using Hypothesis.

**Validates: Requirements US1, US2, US5, US6, US7**
"""

import json
import textwrap
from pathlib import Path

import pytest
from hypothesis import assume, given, settings
from hypothesis import strategies as st

from output_validator.auto_validator import AutoValidator
from output_validator.metrics_tracker import MetricsTracker
from output_validator.schema_extractor import generate_schema_from_example

import jsonschema
from jsonschema import Draft7Validator


# ---------------------------------------------------------------------------
# Strategies
# ---------------------------------------------------------------------------

# JSON-serialisable scalars
json_scalars = st.one_of(
    st.none(),
    st.booleans(),
    st.integers(min_value=-1_000_000, max_value=1_000_000),
    st.floats(allow_nan=False, allow_infinity=False, min_value=-1e6, max_value=1e6),
    st.text(max_size=100),
)

# Simple flat dicts (keys are short ASCII strings, values are scalars)
flat_dicts = st.dictionaries(
    keys=st.text(alphabet=st.characters(whitelist_categories=("Ll", "Lu", "Nd"), whitelist_characters="_"), min_size=1, max_size=20),
    values=json_scalars,
    min_size=1,
    max_size=8,
)

# Strings that are definitely NOT valid JSON
non_json_strings = st.text(min_size=1).filter(
    lambda s: _is_not_valid_json(s)
)


def _is_not_valid_json(s: str) -> bool:
    try:
        json.loads(s)
        return False
    except (json.JSONDecodeError, ValueError):
        return True


def _make_skill_with_example(tmp_path: Path, skill_name: str, example: dict) -> Path:
    skill_dir = tmp_path / skill_name
    skill_dir.mkdir(parents=True, exist_ok=True)
    content = textwrap.dedent(f"""\
        # {skill_name}

        ## Output Example
        ```json
        {json.dumps(example, indent=2)}
        ```
    """)
    (skill_dir / "SKILL.md").write_text(content, encoding="utf-8")
    return skill_dir


# ---------------------------------------------------------------------------
# Property 1: Generated schema always validates the example that produced it
# **Validates: Requirements US1, US7**
# ---------------------------------------------------------------------------

@given(example=flat_dicts)
@settings(max_examples=100)
def test_generated_schema_always_validates_example(example):
    """
    For any flat dict example, the schema generated from it must validate
    that same example without errors.

    **Validates: Requirements US1, US7**
    """
    schema = generate_schema_from_example(example)
    validator = Draft7Validator(schema)
    errors = list(validator.iter_errors(example))
    assert errors == [], (
        f"Schema generated from {example!r} does not validate the example itself.\n"
        f"Errors: {[e.message for e in errors]}"
    )


# ---------------------------------------------------------------------------
# Property 2: Invalid JSON strings always fail validation
# **Validates: Requirements US2**
# ---------------------------------------------------------------------------

@given(bad_json=non_json_strings)
@settings(max_examples=100)
def test_invalid_json_always_fails_validation(bad_json):
    """
    Any string that is not valid JSON must produce is_valid=False.

    **Validates: Requirements US2**
    """
    import tempfile
    with tempfile.TemporaryDirectory() as tmp:
        skills_dir = Path(tmp) / "skills"
        skills_dir.mkdir()
        validator = AutoValidator(skills_dir)
        result = validator.validate_skill_output("any-skill", bad_json)
        assert not result.is_valid, (
            f"Expected invalid result for non-JSON string: {bad_json!r}"
        )


# ---------------------------------------------------------------------------
# Property 3: Valid output always passes when schema is generated from it
# **Validates: Requirements US2, US7**
# ---------------------------------------------------------------------------

@given(example=flat_dicts)
@settings(max_examples=100)
def test_valid_output_passes_when_schema_generated_from_it(example):
    """
    If we generate a schema from an example and then validate that same example,
    the result must be valid.

    **Validates: Requirements US2, US7**
    """
    import tempfile
    with tempfile.TemporaryDirectory() as tmp:
        skills_dir = Path(tmp) / "skills"
        skill_dir = _make_skill_with_example(skills_dir, "prop-skill", example)

        validator = AutoValidator(skills_dir)
        # First call: generates schema
        validator.validate_skill_output("prop-skill", {}, regenerate=True)

        # Second call: validate the original example
        result = validator.validate_skill_output("prop-skill", example)
        assert result.is_valid, (
            f"Expected valid result for example {example!r}.\n"
            f"Errors: {result.errors}"
        )


# ---------------------------------------------------------------------------
# Property 4: Metrics counters never go negative
# **Validates: Requirements US6**
# ---------------------------------------------------------------------------

@given(
    results=st.lists(st.booleans(), min_size=1, max_size=50)
)
@settings(max_examples=100)
def test_metrics_counters_never_go_negative(results):
    """
    After any sequence of record_validation calls, total and valid counters
    must always be non-negative and valid <= total.

    **Validates: Requirements US6**
    """
    import tempfile
    with tempfile.TemporaryDirectory() as tmp:
        tracker = MetricsTracker(Path(tmp) / "metrics.json", window_size=20)
        for is_valid in results:
            tracker.record_validation("skill-prop", is_valid, [] if is_valid else ["err"])

        m = tracker.metrics["skill-prop"]
        assert m["total"] >= 0
        assert m["valid"] >= 0
        assert m["valid"] <= m["total"]


# ---------------------------------------------------------------------------
# Property 5: Schema generation is deterministic
# **Validates: Requirements US1**
# ---------------------------------------------------------------------------

@given(example=flat_dicts)
@settings(max_examples=100)
def test_schema_generation_is_deterministic(example):
    """
    Calling generate_schema_from_example twice with the same input must
    produce identical schemas.

    **Validates: Requirements US1**
    """
    schema1 = generate_schema_from_example(example)
    schema2 = generate_schema_from_example(example)
    assert schema1 == schema2, (
        f"Schema generation is not deterministic for {example!r}.\n"
        f"First:  {schema1}\nSecond: {schema2}"
    )
