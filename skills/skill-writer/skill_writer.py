"""
Skill Writer — utilities for generating and validating SKILL.md files.

Implements:
- SkillScope enum: PRIVATE (persona-scoped) or GLOBAL (bastion-wide)
- SkillMetadata dataclass: frontmatter fields (name, version, description, triggers)
- SkillContent dataclass: full skill content (metadata + instructions + examples + edge_cases)
- generate_skill_md(): renders a SKILL.md string from SkillContent
- get_skill_path(): returns the correct filesystem path based on scope
- validate_skill_md(): checks that a SKILL.md string has all required fields

Path rules (Requirements 6.5):
  - Private skill  → personas/{slug}/SKILL.md
  - Global skill   → skills/{name}/SKILL.md
"""

from __future__ import annotations

import concurrent.futures
import json
import re
import subprocess
from dataclasses import dataclass, field
from datetime import UTC, datetime
from enum import Enum
from pathlib import Path

from i18n import get_string, load_locale

_log_executor = concurrent.futures.ThreadPoolExecutor(max_workers=1)


def _write_log_entry(log_file: Path, entry_str: str) -> None:
    with log_file.open("a", encoding="utf-8") as f:
        f.write(entry_str + "\n")


# ---------------------------------------------------------------------------
# Enums & dataclasses
# ---------------------------------------------------------------------------


class SkillScope(Enum):
    """Scope of a skill: private to a persona or global to the whole Bastion."""

    PRIVATE = "private"
    GLOBAL = "global"


@dataclass
class SkillMetadata:
    """Frontmatter fields required in every SKILL.md."""

    name: str
    version: str
    description: str
    triggers: list[str] = field(default_factory=list)


@dataclass
class SkillContent:
    """Full content of a skill: metadata + body sections."""

    metadata: SkillMetadata
    instructions: str
    examples: str
    edge_cases: str


# ---------------------------------------------------------------------------
# Required frontmatter keys and body section markers
# ---------------------------------------------------------------------------

_REQUIRED_FRONTMATTER_KEYS: tuple[str, ...] = (
    "name",
    "version",
    "description",
    "triggers",
)

_REQUIRED_BODY_SECTIONS: tuple[str, ...] = (
    "## Instruções",
    "## Exemplos",
    "## Edge Cases",
)

# ---------------------------------------------------------------------------
# generate_skill_md
# ---------------------------------------------------------------------------


def generate_skill_md(content: SkillContent) -> str:
    """
    Generate the full SKILL.md string from a SkillContent object.

    The output contains:
    - YAML frontmatter with name, version, description, triggers
    - Body with ## Instruções, ## Exemplos, ## Edge Cases sections

    Args:
        content: The SkillContent to render.

    Returns:
        A string with the complete SKILL.md content.
    """
    meta = content.metadata

    # Build triggers YAML list
    if meta.triggers:
        triggers_yaml = "\n".join(f"  - {t}" for t in meta.triggers)
    else:
        triggers_yaml = "  []"

    # Derive a display name from the skill name (last segment after /)
    display_name = meta.name.split("/")[-1].replace("-", " ").title()

    skill_md = (
        f"---\n"
        f"name: {meta.name}\n"
        f'version: "{meta.version}"\n'
        f"description: >\n"
        f"  {meta.description}\n"
        f"triggers:\n"
        f"{triggers_yaml}\n"
        f"---\n"
        f"\n"
        f"# {display_name}\n"
        f"\n"
        f"## Instruções\n"
        f"\n"
        f"{content.instructions.strip()}\n"
        f"\n"
        f"## Exemplos\n"
        f"\n"
        f"{content.examples.strip()}\n"
        f"\n"
        f"## Edge Cases\n"
        f"\n"
        f"{content.edge_cases.strip()}\n"
    )

    return skill_md


# ---------------------------------------------------------------------------
# get_skill_path
# ---------------------------------------------------------------------------


def get_skill_path(
    scope: SkillScope,
    name: str,
    persona_slug: str | None = None,
) -> Path:
    """
    Return the correct filesystem path for a skill based on its scope.

    Path rules (Requirements 6.5):
      - PRIVATE → personas/{slug}/SKILL.md
      - GLOBAL  → skills/{name}/SKILL.md

    Args:
        scope: SkillScope.PRIVATE or SkillScope.GLOBAL.
        name: The skill name in kebab-case (used as directory name for global skills).
        persona_slug: Required when scope is PRIVATE; the persona's slug.

    Returns:
        A Path object pointing to the SKILL.md location.

    Raises:
        ValueError: If scope is PRIVATE and persona_slug is not provided.
    """
    if scope == SkillScope.PRIVATE:
        if not persona_slug:
            raise ValueError("persona_slug is required for PRIVATE scope skills")
        return Path("personas") / persona_slug / "SKILL.md"

    # GLOBAL
    # Use only the last segment of the name as the directory (strip namespace prefix)
    skill_dir = name.split("/")[-1]
    return Path("skills") / skill_dir / "SKILL.md"


# ---------------------------------------------------------------------------
# validate_skill_md
# ---------------------------------------------------------------------------

_FRONTMATTER_RE = re.compile(r"^---\s*\n(.*?)\n---", re.DOTALL)


def validate_skill_md(content: str) -> bool:
    """
    Validate that a SKILL.md string has all required fields.

    Checks:
    1. YAML frontmatter block is present (delimited by ---)
    2. All required frontmatter keys are present: name, version, description, triggers
    3. All required body sections are present: ## Instruções, ## Exemplos, ## Edge Cases

    Args:
        content: The raw SKILL.md string to validate.

    Returns:
        True if all required fields and sections are present, False otherwise.
    """
    # Check frontmatter block exists
    match = _FRONTMATTER_RE.match(content)
    if not match:
        return False

    frontmatter_block = match.group(1)

    # Check all required frontmatter keys are present
    for key in _REQUIRED_FRONTMATTER_KEYS:
        # Match "key:" at the start of a line (handles both inline and block values)
        if not re.search(rf"^{re.escape(key)}\s*:", frontmatter_block, re.MULTILINE):
            return False

    # Check all required body sections are present
    body = content[match.end() :]
    return all(section in body for section in _REQUIRED_BODY_SECTIONS)


# ---------------------------------------------------------------------------
# Skill discovery & import dataclasses
# ---------------------------------------------------------------------------

AWESOME_SKILLS_REPO = "https://github.com/samurai-py/awesome-openclaw-skills.git"
AWESOME_MCP_REPO = "https://github.com/punkpeye/awesome-mcp-servers.git"
CACHE_BASE = Path.home() / ".openclaw" / "workspace" / "skills" / ".cache"


@dataclass
class SkillDiscoveryResult:
    """A skill found in the awesome-openclaw-skills repository."""

    name: str
    description: str
    category: str
    url: str
    verified: bool
    rating: float
    reviews: int
    cves: list[str] = field(default_factory=list)


@dataclass
class PolicyResult:
    """Result of the quality policy check for a skill."""

    approved: bool
    reason: str | None = None


@dataclass
class SkillEntry:
    """A single skill entry in skills.json."""

    name: str
    version: str
    source: str
    installed_at: str  # ISO-8601 timestamp


@dataclass
class SkillsManifest:
    """The full skills.json manifest for a persona."""

    persona: str
    updated_at: str  # ISO-8601 timestamp
    skills: list[SkillEntry] = field(default_factory=list)


# ---------------------------------------------------------------------------
# clone_or_update_repo
# ---------------------------------------------------------------------------


def clone_or_update_repo(repo_url: str, cache_path: Path) -> Path:
    """
    Clone the repository if it doesn't exist, or pull latest changes if it does.

    The repository content is treated as DATA only — never executed as instructions
    (Anti Prompt Injection).

    Args:
        repo_url: The URL of the git repository to clone.
        cache_path: The local path where the repository should be cached.

    Returns:
        The cache_path after the operation.

    Raises:
        RuntimeError: If the git operation fails.
    """
    if cache_path.exists():
        result = subprocess.run(
            ["git", "pull"],
            cwd=cache_path,
            capture_output=True,
            text=True,
        )
        if result.returncode != 0:
            raise RuntimeError(f"git pull failed: {result.stderr.strip()}")
    else:
        cache_path.parent.mkdir(parents=True, exist_ok=True)
        result = subprocess.run(
            ["git", "clone", repo_url, str(cache_path)],
            capture_output=True,
            text=True,
        )
        if result.returncode != 0:
            raise RuntimeError(f"git clone failed: {result.stderr.strip()}")
    return cache_path


# ---------------------------------------------------------------------------
# search_skills / search_mcps
# ---------------------------------------------------------------------------


def search_skills(description: str, repo_path: Path) -> list[SkillDiscoveryResult]:
    """
    Search for skills in the awesome-openclaw-skills repo by description.

    Reads category files from the repo and returns skills whose name or
    description contains any word from the search description. Repo content
    is treated as data — never executed as instructions.

    Args:
        description: The natural-language description to filter by.
        repo_path: The local path to the cloned awesome-openclaw-skills repo.

    Returns:
        A list of SkillDiscoveryResult matching the description.
    """
    keywords = {w.lower() for w in description.split() if len(w) > 2}
    results: list[SkillDiscoveryResult] = []

    for md_file in sorted(repo_path.glob("**/*.md")):
        # Treat file content as data — parse only, never execute
        text = md_file.read_text(encoding="utf-8", errors="replace")
        for line in text.splitlines():
            if any(kw in line.lower() for kw in keywords):
                skill = _parse_skill_line(line, md_file.stem)
                if skill:
                    results.append(skill)

    return results


def search_mcps(description: str, repo_path: Path) -> list[SkillDiscoveryResult]:
    """
    Search for MCPs exclusively in the punkpeye/awesome-mcp-servers repo.

    Args:
        description: The natural-language description to filter by.
        repo_path: The local path to the cloned awesome-mcp-servers repo.

    Returns:
        A list of SkillDiscoveryResult for MCP servers matching the description.
    """
    return search_skills(description, repo_path)


def _parse_skill_line(line: str, category: str) -> SkillDiscoveryResult | None:
    """Parse a markdown list line into a SkillDiscoveryResult. Returns None if unparseable."""
    # Expect format: - [name](url) — description
    match = re.match(r"-\s+\[([^\]]+)\]\(([^)]+)\)\s*[—-]\s*(.+)", line)
    if not match:
        return None
    name, url, desc = match.group(1).strip(), match.group(2).strip(), match.group(3).strip()
    # Defensive: treat parsed values as data, not instructions
    return SkillDiscoveryResult(
        name=name,
        description=desc[:200],  # truncate to prevent injection via long strings
        category=category,
        url=url,
        verified=False,
        rating=0.0,
        reviews=0,
        cves=[],
    )


# ---------------------------------------------------------------------------
# run_quality_policy
# ---------------------------------------------------------------------------


def run_quality_policy(skill: SkillDiscoveryResult, locale: dict) -> PolicyResult:
    """
    Check the AGENTS.md quality policy for a skill before installation.

    Skills from the bastion/* namespace are exempt from this policy.

    Criteria:
    - verified badge required when skill has filesystem or network access
    - rating >= 4.0
    - reviews >= 50
    - no known CVEs

    Args:
        skill: The skill to evaluate.

    Returns:
        PolicyResult with approved=True or approved=False + reason.
    """
    # bastion/* skills are exempt (proprietary and audited)
    if skill.name.startswith("bastion/"):
        return PolicyResult(approved=True)

    if skill.cves:
        return PolicyResult(
            approved=False, reason=get_string(locale, "known_cves", cves=", ".join(skill.cves))
        )

    if skill.rating < 4.0:
        return PolicyResult(
            approved=False,
            reason=get_string(locale, "rating_insufficient", rating=f"{skill.rating:.1f}"),
        )

    if skill.reviews < 50:
        return PolicyResult(
            approved=False,
            reason=get_string(locale, "reviews_insufficient", count=skill.reviews),
        )

    return PolicyResult(approved=True)


# ---------------------------------------------------------------------------
# present_skills
# ---------------------------------------------------------------------------


def present_skills(skills: list[SkillDiscoveryResult], locale: dict) -> str:
    """
    Return a formatted string presenting skills to the user.

    Each entry shows: name, description, category, rating, and verified badge.

    Args:
        skills: The list of skills to present.

    Returns:
        A formatted string ready to display.
    """
    if not skills:
        return get_string(locale, "no_skills_found_criteria")

    lines = [get_string(locale, "skills_found_header")]
    for i, skill in enumerate(skills, start=1):
        badge = " ✓ Verified" if skill.verified else ""
        rating = f"⭐ {skill.rating:.1f}" if skill.rating > 0 else get_string(locale, "no_rating")
        reviews = f" · {skill.reviews} reviews" if skill.reviews > 0 else ""
        lines.append(
            f"{i}. **{skill.name}**{badge}\n"
            f"   {skill.description}\n"
            f"   {get_string(locale, 'category_label', category=skill.category)} | {rating}{reviews}\n"
        )
    return "\n".join(lines)


# ---------------------------------------------------------------------------
# sage_scan
# ---------------------------------------------------------------------------


def sage_scan(skill: SkillDiscoveryResult, locale: dict) -> PolicyResult:
    """
    Invoke the Sage plugin's before_tool_call hook before installing a skill.

    If Sage blocks the skill, the individual skill is rejected without aborting
    the installation of other skills.

    Args:
        skill: The skill to scan.

    Returns:
        PolicyResult indicating whether Sage approved the installation.
    """
    result = subprocess.run(
        ["clawhub", "sage-scan", skill.url],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        return PolicyResult(
            approved=False,
            reason=get_string(
                locale, "sage_blocked", reason=result.stderr.strip() or result.stdout.strip()
            ),
        )
    return PolicyResult(approved=True)


# ---------------------------------------------------------------------------
# install_skill_for_persona / update_skills_json / log_skill_event
# ---------------------------------------------------------------------------


def install_skill_for_persona(skill: SkillDiscoveryResult, persona_slug: str, locale: dict) -> bool:
    """
    Install a skill via clawhub and update skills.json for the given persona.

    Runs quality policy and Sage scan before installation. On success, records
    the skill in skills.json and logs the event. Sage rejection blocks the
    individual skill only — other skills continue normally.

    Args:
        skill: The skill to install.
        persona_slug: The persona's slug.

    Returns:
        True if the skill was installed successfully, False otherwise.
    """
    policy = run_quality_policy(skill, locale)
    if not policy.approved:
        log_skill_event(skill.name, "unknown", persona_slug, "blocked", policy.reason)
        return False

    scan = sage_scan(skill, locale)
    if not scan.approved:
        log_skill_event(skill.name, "unknown", persona_slug, "blocked_sage", scan.reason)
        return False

    result = subprocess.run(
        ["clawhub", "install", skill.url],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        log_skill_event(skill.name, "unknown", persona_slug, "failed", result.stderr.strip())
        return False

    entry = SkillEntry(
        name=skill.name,
        version="latest",
        source=skill.url,
        installed_at=datetime.now(UTC).isoformat(),
    )
    update_skills_json(persona_slug, entry, locale)
    log_skill_event(skill.name, entry.version, persona_slug, "installed")
    return True


def update_skills_json(persona_slug: str, skill_entry: SkillEntry, locale: dict) -> None:
    """
    Create or update config/workspace/personas/{slug}/skills.json.

    If the file contains invalid JSON, logs an error and recreates it.

    Args:
        persona_slug: The persona's slug.
        skill_entry: The skill entry to add or update.
    """
    skills_path = Path("config") / "workspace" / "personas" / persona_slug / "skills.json"
    skills_path.parent.mkdir(parents=True, exist_ok=True)

    manifest: SkillsManifest
    if skills_path.exists():
        try:
            manifest = parse_skills_json(skills_path)
        except (ValueError, KeyError):
            print(get_string(locale, "skills_json_corrupt", path=skills_path))
            manifest = SkillsManifest(
                persona=persona_slug,
                updated_at=datetime.now(UTC).isoformat(),
            )
    else:
        manifest = SkillsManifest(
            persona=persona_slug,
            updated_at=datetime.now(UTC).isoformat(),
        )

    # Replace existing entry with same name, or append
    manifest.skills = [s for s in manifest.skills if s.name != skill_entry.name]
    manifest.skills.append(skill_entry)
    manifest.updated_at = datetime.now(UTC).isoformat()

    skills_path.write_text(serialize_skills_json(manifest), encoding="utf-8")


def serialize_skills_json(manifest: SkillsManifest) -> str:
    """Serialize a SkillsManifest to a JSON string."""
    return json.dumps(
        {
            "persona": manifest.persona,
            "updated_at": manifest.updated_at,
            "skills": [
                {
                    "name": s.name,
                    "version": s.version,
                    "source": s.source,
                    "installed_at": s.installed_at,
                }
                for s in manifest.skills
            ],
        },
        indent=2,
        ensure_ascii=False,
    )


def parse_skills_json(path: Path) -> SkillsManifest:
    """
    Parse a skills.json file into a SkillsManifest.

    Raises:
        ValueError: If the JSON is malformed.
        KeyError: If required fields are missing.
    """
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        raise ValueError(f"Invalid JSON in {path}: {exc}") from exc

    skills = [
        SkillEntry(
            name=entry["name"],
            version=entry["version"],
            source=entry["source"],
            installed_at=entry["installed_at"],
        )
        for entry in data["skills"]
    ]
    return SkillsManifest(
        persona=data["persona"],
        updated_at=data["updated_at"],
        skills=skills,
    )


def log_skill_event(
    skill_name: str,
    version: str,
    persona: str,
    result: str,
    reason: str | None = None,
) -> None:
    """
    Register a skill installation event in the life_log.

    Required fields: timestamp, name, version, persona, result.

    Args:
        skill_name: The name of the skill.
        version: The version installed (or "unknown").
        persona: The persona slug.
        result: One of "installed", "failed", "blocked", "blocked_sage".
        reason: Optional reason for non-installed results.
    """
    log_dir = Path("config") / "workspace" / "life_log"
    log_dir.mkdir(parents=True, exist_ok=True)
    log_file = log_dir / "skill_events.jsonl"

    entry: dict = {
        "timestamp": datetime.now(UTC).isoformat(),
        "name": skill_name,
        "version": version,
        "persona": persona,
        "result": result,
    }
    if reason:
        entry["reason"] = reason

    entry_str = json.dumps(entry, ensure_ascii=False)
    _log_executor.submit(_write_log_entry, log_file, entry_str)


# ---------------------------------------------------------------------------
# run_persona_skills_flow — full wiring
# ---------------------------------------------------------------------------


def run_persona_skills_flow(
    persona_description: str, persona_slug: str, language: str = "en"
) -> None:
    """
    Full skill import flow for a persona.

    Steps:
      1. Clone or update awesome-openclaw-skills repo
      2. Search skills by persona description
      3. Present skills to user for approval
      4. For each approved skill: run_quality_policy → sage_scan → install → update_skills_json → log
      5. Summarize results

    Args:
        persona_description: Natural-language description of what the persona needs.
        persona_slug: The persona's slug (used for skills.json path).
    """
    locale = load_locale(language, skill_dir=Path(__file__).parent)

    cache_path = CACHE_BASE / "awesome-openclaw-skills"
    clone_or_update_repo(AWESOME_SKILLS_REPO, cache_path)

    skills = search_skills(persona_description, cache_path)
    if not skills:
        print(get_string(locale, "no_skills_for_persona"))
        return

    print(present_skills(skills, locale))

    approved_input = input(get_string(locale, "install_prompt")).strip()
    if approved_input.lower() in ("none", "nenhuma"):
        print(get_string(locale, "none_installed"))
        return

    if approved_input.lower() in ("all", "todas"):
        selected = skills
    else:
        try:
            indices = [int(i.strip()) - 1 for i in approved_input.split(",")]
            selected = [skills[i] for i in indices if 0 <= i < len(skills)]
        except (ValueError, IndexError):
            print(get_string(locale, "invalid_selection"))
            return

    installed, failed = [], []
    for skill in selected:
        ok = install_skill_for_persona(skill, persona_slug, locale)
        (installed if ok else failed).append(skill.name)

    print(get_string(locale, "installed_label", names=", ".join(installed) or "-"))
    if failed:
        print(get_string(locale, "failed_label", names=", ".join(failed)))


# ---------------------------------------------------------------------------
# CLI Interface for OpenClaw Agent
# ---------------------------------------------------------------------------
def main() -> None:
    import argparse

    parser = argparse.ArgumentParser(description="CLI wrapper generated by refactoring")
    parser.add_argument("--action", help="Action to perform")
    parser.add_argument("--args-json", default="{}", help="Arguments as JSON string")

    args = parser.parse_args()
    print("Execution of stub CLI for", __file__)
    print("Action:", args.action)
    print("Args:", args.args_json)


if __name__ == "__main__":
    main()
