"""
Persona Activation — auto-install missing skills on persona activation.

Reads config/workspace/personas/{slug}/skills.json and ensures every listed
skill is installed. Missing skills are installed automatically. A corrupted
or missing skills.json causes a warning but does not block persona activation.
"""

from __future__ import annotations

import subprocess
from pathlib import Path

from skill_writer import parse_skills_json, log_skill_event


def activate_persona_skills(persona_slug: str) -> None:
    """
    Verify and auto-install skills listed in the persona's skills.json.

    - Missing skills are installed via `clawhub install {source || name}`.
    - A single skill failure is non-fatal: activation continues with a warning.
    - A corrupted or missing skills.json logs an error and skips skill checks.

    Args:
        persona_slug: The persona's slug.
    """
    skills_path = (
        Path("config") / "workspace" / "personas" / persona_slug / "skills.json"
    )

    if not skills_path.exists():
        return

    try:
        manifest = parse_skills_json(skills_path)
    except (ValueError, KeyError) as exc:
        print(
            f"[persona-activation] ERRO: skills.json inválido para '{persona_slug}': {exc}. "
            "Ativando persona sem verificação de skills."
        )
        return

    for entry in manifest.skills:
        if _is_skill_installed(entry.name):
            continue

        source = entry.source if entry.source else entry.name
        print(f"[persona-activation] Skill '{entry.name}' ausente. Instalando...")

        result = subprocess.run(
            ["clawhub", "install", source],
            capture_output=True,
            text=True,
        )

        if result.returncode == 0:
            log_skill_event(entry.name, entry.version, persona_slug, "installed")
            print(f"[persona-activation] Skill '{entry.name}' instalada.")
        else:
            log_skill_event(
                entry.name,
                entry.version,
                persona_slug,
                "failed",
                result.stderr.strip(),
            )
            print(
                f"[persona-activation] AVISO: falha ao instalar '{entry.name}'. "
                "Continuando ativação da persona."
            )


def _is_skill_installed(skill_name: str) -> bool:
    """Check if a skill is currently installed via clawhub list."""
    result = subprocess.run(
        ["clawhub", "list"],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        return False
    return skill_name in result.stdout
