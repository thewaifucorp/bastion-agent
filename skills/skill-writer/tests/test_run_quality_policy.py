"""
Tests for run_quality_policy in the skill-writer skill.
"""

from __future__ import annotations

import sys
from pathlib import Path

# Allow importing skill_writer from the parent directory
sys.path.insert(0, str(Path(__file__).parent.parent))

from skill_writer import run_quality_policy, SkillDiscoveryResult

def test_run_quality_policy_exempt_namespace() -> None:
    """bastion/* skills should pass regardless of rating/reviews/cves."""
    skill = SkillDiscoveryResult(
        name="bastion/core-skill",
        description="A core skill",
        category="system",
        url="https://example.com/bastion/core",
        verified=False,
        rating=1.0,
        reviews=0,
        cves=["CVE-2023-1234"]
    )
    locale = {}

    result = run_quality_policy(skill, locale)

    assert result.approved is True
    assert result.reason is None

def test_run_quality_policy_known_cves() -> None:
    """Skills with known CVEs should fail."""
    skill = SkillDiscoveryResult(
        name="community/hacked-skill",
        description="A hacked skill",
        category="utility",
        url="https://example.com/community/hacked",
        verified=True,
        rating=5.0,
        reviews=100,
        cves=["CVE-2024-5678", "CVE-2024-9012"]
    )
    locale = {"known_cves": "Danger: {cves}"}

    result = run_quality_policy(skill, locale)

    assert result.approved is False
    assert result.reason == "Danger: CVE-2024-5678, CVE-2024-9012"

def test_run_quality_policy_insufficient_rating() -> None:
    """Skills with a rating < 4.0 should fail."""
    skill = SkillDiscoveryResult(
        name="community/bad-skill",
        description="A bad skill",
        category="utility",
        url="https://example.com/community/bad",
        verified=True,
        rating=3.9,
        reviews=100,
        cves=[]
    )
    locale = {"rating_insufficient": "Low rating: {rating}"}

    result = run_quality_policy(skill, locale)

    assert result.approved is False
    assert result.reason == "Low rating: 3.9"

def test_run_quality_policy_insufficient_reviews() -> None:
    """Skills with reviews < 50 should fail."""
    skill = SkillDiscoveryResult(
        name="community/new-skill",
        description="A new skill",
        category="utility",
        url="https://example.com/community/new",
        verified=True,
        rating=4.5,
        reviews=49,
        cves=[]
    )
    locale = {"reviews_insufficient": "Not enough reviews: {count}"}

    result = run_quality_policy(skill, locale)

    assert result.approved is False
    assert result.reason == "Not enough reviews: 49"

def test_run_quality_policy_happy_path() -> None:
    """Skills meeting all criteria should pass."""
    skill = SkillDiscoveryResult(
        name="community/good-skill",
        description="A good skill",
        category="utility",
        url="https://example.com/community/good",
        verified=True,
        rating=4.0,
        reviews=50,
        cves=[]
    )
    locale = {}

    result = run_quality_policy(skill, locale)

    assert result.approved is True
    assert result.reason is None
