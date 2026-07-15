"""
CLI interface for the Bastion Output Validator.

Commands:
    regenerate  — regenerate schema for a skill
    stats       — show validation statistics
    validate    — validate an output file against a skill's schema
    dashboard   — show all skills validation status with colour coding
"""

import json
import sys
from pathlib import Path
from typing import Optional

import click

from .auto_validator import AutoValidator
from .metrics_tracker import MetricsTracker
from .alerts import run_alert_scan

_SKILLS_DIR = Path("skills")
_METRICS_FILE = Path("config/logs/validation-metrics.json")


@click.group()
def cli() -> None:
    """Bastion Output Validator — schema generation and validation."""


# ---------------------------------------------------------------------------
# regenerate
# ---------------------------------------------------------------------------

@cli.command()
@click.argument("skill_name")
def regenerate(skill_name: str) -> None:
    """Regenerate the JSON Schema for SKILL_NAME from its SKILL.md example."""
    validator = AutoValidator(_SKILLS_DIR)
    click.echo(f"Regenerating schema for '{skill_name}'…")

    result = validator.validate_skill_output(skill_name, {}, regenerate=True)

    if result.schema_generated:
        click.echo(f"✓ Schema generated: {result.schema_path}")
    elif result.warnings:
        click.echo(f"✗ {result.warnings[0]}")
        sys.exit(1)
    else:
        click.echo("✓ Schema already up-to-date (no changes needed)")


# ---------------------------------------------------------------------------
# stats
# ---------------------------------------------------------------------------

@cli.command()
@click.argument("skill_name", required=False, default=None)
def stats(skill_name: Optional[str]) -> None:
    """Show validation statistics. Optionally filter by SKILL_NAME."""
    tracker = MetricsTracker(_METRICS_FILE)
    data = tracker.get_stats(skill_name)

    if not data:
        click.echo("No metrics recorded yet.")
        return

    if skill_name:
        _print_skill_stats(data)
    else:
        for skill_stats in data.values():
            _print_skill_stats(skill_stats)
            click.echo()


def _print_skill_stats(stats: dict) -> None:
    """Print formatted statistics for a single skill."""
    click.echo(f"Skill: {stats['skill']}")
    click.echo(f"  Total validations : {stats['total_validations']}")
    click.echo(f"  Overall success   : {stats['overall_success_rate']:.1%}")
    click.echo(f"  Recent success    : {stats['recent_success_rate']:.1%} "
               f"(last {stats['recent_window_size']} runs)")
    if stats.get("last_error"):
        err = stats["last_error"]
        click.echo(f"  Last error        : {err['timestamp']}")
        for msg in err.get("errors", []):
            click.echo(f"    - {msg}")
    if stats.get("last_updated"):
        click.echo(f"  Last updated      : {stats['last_updated']}")


# ---------------------------------------------------------------------------
# validate
# ---------------------------------------------------------------------------

@cli.command()
@click.argument("skill_name")
@click.argument("output_file", type=click.Path(exists=True))
def validate(skill_name: str, output_file: str) -> None:
    """Validate OUTPUT_FILE (JSON) against SKILL_NAME's schema."""
    try:
        with open(output_file, encoding="utf-8") as fh:
            output = json.load(fh)
    except (OSError, json.JSONDecodeError) as exc:
        click.echo(f"✗ Cannot read output file: {exc}", err=True)
        sys.exit(1)

    validator = AutoValidator(_SKILLS_DIR)
    result = validator.validate_skill_output(skill_name, output)

    if result.warnings:
        for w in result.warnings:
            click.echo(f"⚠  {w}")

    if result.is_valid:
        click.echo("✓ Output is valid")
    else:
        click.echo("✗ Output is invalid:")
        for err in result.errors:
            click.echo(f"  - {err}")
        sys.exit(1)


# ---------------------------------------------------------------------------
# dashboard
# ---------------------------------------------------------------------------

@cli.command()
def dashboard() -> None:
    """Show all skills validation status with colour-coded success rates."""
    tracker = MetricsTracker(_METRICS_FILE)
    all_stats = tracker.get_stats()

    if not all_stats:
        click.echo("No metrics recorded yet.")
        return

    click.echo(f"{'Skill':<25} {'Total':>7} {'Overall':>9} {'Recent':>9} {'Status'}")
    click.echo("-" * 65)

    for skill_stats in all_stats.values():
        recent_rate = skill_stats["recent_success_rate"]
        overall_rate = skill_stats["overall_success_rate"]

        if recent_rate >= 0.95:
            colour = "green"
            status = "✓ OK"
        elif recent_rate >= 0.90:
            colour = "yellow"
            status = "⚠ WARN"
        else:
            colour = "red"
            status = "✗ DRIFT"

        line = (
            f"{skill_stats['skill']:<25} "
            f"{skill_stats['total_validations']:>7} "
            f"{overall_rate:>8.1%} "
            f"{recent_rate:>8.1%}  "
            f"{status}"
        )
        click.echo(click.style(line, fg=colour))


# ---------------------------------------------------------------------------
# alerts
# ---------------------------------------------------------------------------

@cli.command()
def alerts() -> None:
    """Scan all skills for monitoring alerts (drift, missing schema, error spikes)."""
    triggered = run_alert_scan(
        metrics_file=_METRICS_FILE,
        skills_dir=_SKILLS_DIR,
    )

    if not triggered:
        click.echo(click.style("✓ No alerts — all skills within normal parameters.", fg="green"))
        return

    for alert in triggered:
        colour = "red" if alert.level == "error" else "yellow"
        click.echo(click.style(alert.message, fg=colour))

    sys.exit(1)


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------

if __name__ == "__main__":
    cli()
