"""
Property-based tests for the Skill Writer skill.

**Validates: Requirements 6.4, 6.5**

Properties tested:
  - Property 14: SKILL.md gerado contém estrutura obrigatória completa
                 (frontmatter com name, version, description, triggers +
                  body com instruções, exemplos, edge cases)
  - Property 15: Skill é salva no caminho correto conforme escopo
                 (personas/{slug}/SKILL.md para privada,
                  skills/{nome}/SKILL.md para global)
"""

from __future__ import annotations

import sys
from pathlib import Path

# Allow importing skill_writer from the parent directory
sys.path.insert(0, str(Path(__file__).parent.parent))

import pytest
from hypothesis import given, settings
from hypothesis import strategies as st
from skill_writer import (
    SkillContent,
    SkillMetadata,
    SkillScope,
    generate_skill_md,
    get_skill_path,
    validate_skill_md,
)

# ---------------------------------------------------------------------------
# Hypothesis strategies
# ---------------------------------------------------------------------------

_slug = st.text(
    alphabet="abcdefghijklmnopqrstuvwxyz0123456789-",
    min_size=1,
    max_size=30,
).filter(lambda s: not s.startswith("-") and not s.endswith("-"))

_skill_name = st.text(
    alphabet="abcdefghijklmnopqrstuvwxyz0123456789-/",
    min_size=1,
    max_size=50,
).filter(lambda s: "/" not in s or s.split("/")[-1])  # last segment must be non-empty

_version = st.from_regex(r"[0-9]+\.[0-9]+\.[0-9]+", fullmatch=True)

_description = st.text(min_size=1, max_size=200).filter(lambda s: "\n" not in s)

_trigger = st.text(min_size=1, max_size=50).filter(lambda s: "\n" not in s)

_triggers = st.lists(_trigger, min_size=1, max_size=10)

_body_text = st.text(min_size=1, max_size=500)


def _metadata_strategy() -> st.SearchStrategy[SkillMetadata]:
    return st.builds(
        SkillMetadata,
        name=_skill_name,
        version=_version,
        description=_description,
        triggers=_triggers,
    )


def _content_strategy() -> st.SearchStrategy[SkillContent]:
    return st.builds(
        SkillContent,
        metadata=_metadata_strategy(),
        instructions=_body_text,
        examples=_body_text,
        edge_cases=_body_text,
    )


# ---------------------------------------------------------------------------
# Property 14 — SKILL.md gerado contém estrutura obrigatória completa
# Validates: Requirements 6.4
# ---------------------------------------------------------------------------


@given(content=_content_strategy())
@settings(max_examples=300)
def test_property14_generated_skill_md_has_required_frontmatter(
    content: SkillContent,
) -> None:
    """
    **Property 14: SKILL.md gerado contém estrutura obrigatória completa**

    For any SkillContent, generate_skill_md() must produce a string that
    contains the frontmatter with all required keys: name, version,
    description, triggers.

    **Validates: Requirements 6.4**
    """
    skill_md = generate_skill_md(content)

    assert "---" in skill_md, "SKILL.md must contain YAML frontmatter delimiters"
    assert "name:" in skill_md, "SKILL.md frontmatter must contain 'name'"
    assert "version:" in skill_md, "SKILL.md frontmatter must contain 'version'"
    assert "description:" in skill_md, "SKILL.md frontmatter must contain 'description'"
    assert "triggers:" in skill_md, "SKILL.md frontmatter must contain 'triggers'"


@given(content=_content_strategy())
@settings(max_examples=300)
def test_property14_generated_skill_md_has_required_body_sections(
    content: SkillContent,
) -> None:
    """
    **Property 14: SKILL.md gerado contém body com instruções, exemplos e edge cases**

    For any SkillContent, generate_skill_md() must produce a string that
    contains the body sections: ## Instruções, ## Exemplos, ## Edge Cases.

    **Validates: Requirements 6.4**
    """
    skill_md = generate_skill_md(content)

    assert "## Instruções" in skill_md, "SKILL.md body must contain '## Instruções'"
    assert "## Exemplos" in skill_md, "SKILL.md body must contain '## Exemplos'"
    assert "## Edge Cases" in skill_md, "SKILL.md body must contain '## Edge Cases'"


@given(content=_content_strategy())
@settings(max_examples=300)
def test_property14_generated_skill_md_passes_validation(
    content: SkillContent,
) -> None:
    """
    **Property 14: SKILL.md gerado passa na validação completa**

    For any SkillContent, the output of generate_skill_md() must pass
    validate_skill_md() — i.e., the generator and validator are consistent.

    **Validates: Requirements 6.4**
    """
    skill_md = generate_skill_md(content)
    assert validate_skill_md(skill_md), (
        f"Generated SKILL.md failed validation.\n"
        f"Content preview:\n{skill_md[:500]}"
    )


@given(content=_content_strategy())
@settings(max_examples=200)
def test_property14_metadata_values_present_in_output(
    content: SkillContent,
) -> None:
    """
    **Property 14: Valores do metadata aparecem no SKILL.md gerado**

    The name, version, and description from SkillMetadata must appear
    verbatim in the generated SKILL.md.

    **Validates: Requirements 6.4**
    """
    skill_md = generate_skill_md(content)

    assert content.metadata.name in skill_md, (
        f"Skill name '{content.metadata.name}' not found in generated SKILL.md"
    )
    assert content.metadata.version in skill_md, (
        f"Skill version '{content.metadata.version}' not found in generated SKILL.md"
    )
    assert content.metadata.description in skill_md, (
        "Skill description not found in generated SKILL.md"
    )


@given(content=_content_strategy())
@settings(max_examples=200)
def test_property14_all_triggers_present_in_output(
    content: SkillContent,
) -> None:
    """
    **Property 14: Todos os triggers aparecem no SKILL.md gerado**

    Every trigger in SkillMetadata.triggers must appear in the generated
    SKILL.md frontmatter.

    **Validates: Requirements 6.4**
    """
    skill_md = generate_skill_md(content)

    for trigger in content.metadata.triggers:
        assert trigger in skill_md, (
            f"Trigger '{trigger}' not found in generated SKILL.md"
        )


# ---------------------------------------------------------------------------
# Property 15 — Skill é salva no caminho correto conforme escopo
# Validates: Requirements 6.5
# ---------------------------------------------------------------------------


@given(
    name=_slug,
    persona_slug=_slug,
)
@settings(max_examples=300)
def test_property15_private_skill_path_uses_personas_dir(
    name: str,
    persona_slug: str,
) -> None:
    """
    **Property 15: Skill privada é salva em personas/{slug}/SKILL.md**

    For any skill name and persona slug, get_skill_path() with
    SkillScope.PRIVATE must return a path of the form
    personas/{slug}/SKILL.md.

    **Validates: Requirements 6.5**
    """
    path = get_skill_path(SkillScope.PRIVATE, name, persona_slug=persona_slug)

    assert path.parts[0] == "personas", (
        f"Private skill path must start with 'personas/', got: {path}"
    )
    assert path.parts[1] == persona_slug, (
        f"Private skill path must use persona slug '{persona_slug}', got: {path}"
    )
    assert path.name == "SKILL.md", (
        f"Private skill path must end with 'SKILL.md', got: {path}"
    )
    assert str(path) == f"personas/{persona_slug}/SKILL.md", (
        f"Expected 'personas/{persona_slug}/SKILL.md', got: {path}"
    )


@given(name=_slug)
@settings(max_examples=300)
def test_property15_global_skill_path_uses_skills_dir(name: str) -> None:
    """
    **Property 15: Skill global é salva em skills/{nome}/SKILL.md**

    For any skill name, get_skill_path() with SkillScope.GLOBAL must
    return a path of the form skills/{nome}/SKILL.md.

    **Validates: Requirements 6.5**
    """
    path = get_skill_path(SkillScope.GLOBAL, name)

    assert path.parts[0] == "skills", (
        f"Global skill path must start with 'skills/', got: {path}"
    )
    assert path.name == "SKILL.md", (
        f"Global skill path must end with 'SKILL.md', got: {path}"
    )
    assert str(path) == f"skills/{name}/SKILL.md", (
        f"Expected 'skills/{name}/SKILL.md', got: {path}"
    )


@given(
    name=_slug,
    persona_slug=_slug,
)
@settings(max_examples=200)
def test_property15_private_and_global_paths_are_distinct(
    name: str,
    persona_slug: str,
) -> None:
    """
    **Property 15: Caminhos privado e global são sempre distintos**

    For any skill name and persona slug, the private path and global path
    must never be the same.

    **Validates: Requirements 6.5**
    """
    private_path = get_skill_path(SkillScope.PRIVATE, name, persona_slug=persona_slug)
    global_path = get_skill_path(SkillScope.GLOBAL, name)

    assert private_path != global_path, (
        f"Private and global paths must differ, but both are: {private_path}"
    )


def test_property15_private_scope_without_slug_raises() -> None:
    """
    **Property 15: Escopo privado sem persona_slug levanta ValueError**

    Calling get_skill_path() with PRIVATE scope and no persona_slug must
    raise a ValueError.

    **Validates: Requirements 6.5**
    """
    with pytest.raises(ValueError, match="persona_slug"):
        get_skill_path(SkillScope.PRIVATE, "my-skill")


@given(
    namespace=_slug,
    skill_name=_slug,
    persona_slug=_slug,
)
@settings(max_examples=200)
def test_property15_global_path_uses_last_name_segment(
    namespace: str,
    skill_name: str,
    persona_slug: str,
) -> None:
    """
    **Property 15: Caminho global usa o último segmento do nome**

    When the skill name contains a namespace prefix (e.g., "bastion/my-skill"),
    the global path must use only the last segment as the directory name.

    **Validates: Requirements 6.5**
    """
    namespaced_name = f"{namespace}/{skill_name}"
    path = get_skill_path(SkillScope.GLOBAL, namespaced_name)

    assert path.parts[1] == skill_name, (
        f"Global path directory should be '{skill_name}' (last segment), got: {path}"
    )
    assert str(path) == f"skills/{skill_name}/SKILL.md"


# ---------------------------------------------------------------------------
# validate_skill_md — unit tests for the validator itself
# ---------------------------------------------------------------------------


def test_validate_skill_md_accepts_valid_content() -> None:
    """validate_skill_md returns True for a well-formed SKILL.md."""
    content = SkillContent(
        metadata=SkillMetadata(
            name="bastion/test-skill",
            version="1.0.0",
            description="A test skill",
            triggers=["test", "exemplo"],
        ),
        instructions="Passo 1: faça isso.\nPasso 2: faça aquilo.",
        examples="Exemplo 1: input → output.",
        edge_cases="Edge case: se X então Y.",
    )
    skill_md = generate_skill_md(content)
    assert validate_skill_md(skill_md) is True


def test_validate_skill_md_rejects_missing_frontmatter() -> None:
    """validate_skill_md returns False when frontmatter is absent."""
    assert validate_skill_md("# Just a heading\n\nSome content.") is False


def test_validate_skill_md_rejects_missing_name() -> None:
    """validate_skill_md returns False when 'name' key is missing."""
    bad = (
        "---\n"
        "version: \"1.0.0\"\n"
        "description: >\n  A skill\n"
        "triggers:\n  - test\n"
        "---\n\n"
        "## Instruções\n\nstep\n\n"
        "## Exemplos\n\nexample\n\n"
        "## Edge Cases\n\nedge\n"
    )
    assert validate_skill_md(bad) is False


def test_validate_skill_md_rejects_missing_body_section() -> None:
    """validate_skill_md returns False when a required body section is missing."""
    bad = (
        "---\n"
        "name: bastion/test\n"
        "version: \"1.0.0\"\n"
        "description: >\n  A skill\n"
        "triggers:\n  - test\n"
        "---\n\n"
        "## Instruções\n\nstep\n\n"
        "## Exemplos\n\nexample\n"
        # Missing ## Edge Cases
    )
    assert validate_skill_md(bad) is False
