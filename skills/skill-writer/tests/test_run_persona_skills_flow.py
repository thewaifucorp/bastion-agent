import sys
from pathlib import Path
import builtins

import pytest

# Allow importing skill_writer from the parent directory
sys.path.insert(0, str(Path(__file__).parent.parent))

import skill_writer
from skill_writer import run_persona_skills_flow, SkillDiscoveryResult

@pytest.fixture
def mock_flow_deps(monkeypatch):
    """Mocks common dependencies for run_persona_skills_flow to avoid real I/O and repo cloning."""
    monkeypatch.setattr(skill_writer, "clone_or_update_repo", lambda *args, **kwargs: None)
    monkeypatch.setattr(skill_writer, "get_string", lambda l, k, **kw: k)
    monkeypatch.setattr(skill_writer, "load_locale", lambda *args, **kwargs: {})
    monkeypatch.setattr(skill_writer, "present_skills", lambda *args, **kwargs: "Mock Presentation")

    prints = []
    monkeypatch.setattr(builtins, "print", lambda s: prints.append(str(s)))

    return prints


def create_mock_skill(name: str) -> SkillDiscoveryResult:
    return SkillDiscoveryResult(
        name=name,
        description="mock desc",
        category="mock cat",
        url="http://mock",
        verified=False,
        rating=5.0,
        reviews=1
    )

def test_flow_no_skills(monkeypatch, mock_flow_deps):
    monkeypatch.setattr(skill_writer, "search_skills", lambda *args, **kwargs: [])

    run_persona_skills_flow("desc", "slug")

    assert "no_skills_for_persona" in mock_flow_deps

def test_flow_input_none(monkeypatch, mock_flow_deps):
    mock_skill = create_mock_skill("skill1")
    monkeypatch.setattr(skill_writer, "search_skills", lambda *args, **kwargs: [mock_skill])
    monkeypatch.setattr(builtins, "input", lambda p: "none")

    run_persona_skills_flow("desc", "slug")

    assert "none_installed" in mock_flow_deps
    assert "Mock Presentation" in mock_flow_deps

def test_flow_input_all_success(monkeypatch, mock_flow_deps):
    mock_skill1 = create_mock_skill("skill1")
    mock_skill2 = create_mock_skill("skill2")
    monkeypatch.setattr(skill_writer, "search_skills", lambda *args, **kwargs: [mock_skill1, mock_skill2])
    monkeypatch.setattr(builtins, "input", lambda p: "all")
    monkeypatch.setattr(skill_writer, "install_skill_for_persona", lambda *args, **kwargs: True)

    run_persona_skills_flow("desc", "slug")

    assert "installed_label" in mock_flow_deps
    assert "failed_label" not in mock_flow_deps
    # Since get_string is mocked to return the key, installed_label is printed

def test_flow_input_indices_mixed_results(monkeypatch, mock_flow_deps):
    mock_skill1 = create_mock_skill("skill1")
    mock_skill2 = create_mock_skill("skill2")
    mock_skill3 = create_mock_skill("skill3")
    monkeypatch.setattr(skill_writer, "search_skills", lambda *args, **kwargs: [mock_skill1, mock_skill2, mock_skill3])
    # Select skill1 and skill3
    monkeypatch.setattr(builtins, "input", lambda p: "1, 3")

    def mock_install(skill, slug, locale):
        if skill.name == "skill1":
            return True
        return False

    monkeypatch.setattr(skill_writer, "install_skill_for_persona", mock_install)

    run_persona_skills_flow("desc", "slug")

    assert "installed_label" in mock_flow_deps
    assert "failed_label" in mock_flow_deps

def test_flow_invalid_selection_value_error(monkeypatch, mock_flow_deps):
    mock_skill = create_mock_skill("skill1")
    monkeypatch.setattr(skill_writer, "search_skills", lambda *args, **kwargs: [mock_skill])
    monkeypatch.setattr(builtins, "input", lambda p: "invalid")

    run_persona_skills_flow("desc", "slug")

    assert "invalid_selection" in mock_flow_deps
