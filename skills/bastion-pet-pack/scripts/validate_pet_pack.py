#!/usr/bin/env python3
import argparse
import re
import sys
import tomllib
import unicodedata
from pathlib import Path

STATES = ("onboarding", "guard", "thinking", "build", "cabinet", "success", "alert")
TAGS = {"primary", "secondary", "cyan", "blue", "magenta", "amber", "green", "red", "muted", "reset"}
TAG = re.compile(r"\{([^{}]+)\}")
HEX_COLOR = re.compile(r"#?[0-9A-Fa-f]{6}")
TOP_LEVEL_KEYS = {"schema", "id", "name", "rarity", "palette", "states"}
PALETTE_KEYS = {"primary", "secondary"}
ANIMATION_KEYS = {"interval_ms", "frames"}
RARITIES = {"common", "uncommon", "rare", "epic", "legendary"}


def width(text: str) -> int:
    return sum(0 if unicodedata.combining(char) else 2 if unicodedata.east_asian_width(char) in "WF" else 1 for char in text)


def validate(path: Path) -> list[str]:
    errors: list[str] = []
    if path.stat().st_size > 64 * 1024:
        return ["file exceeds 65536 bytes"]
    with path.open("rb") as stream:
        data = tomllib.load(stream)
    if not isinstance(data, dict):
        return ["root must be a TOML table"]
    unknown = set(data) - TOP_LEVEL_KEYS
    if unknown:
        errors.append(f"unknown root fields: {', '.join(sorted(unknown))}")
    if data.get("schema") != 1:
        errors.append("schema must be 1")
    if not re.fullmatch(r"[a-z0-9-]+/[a-z0-9-]+", str(data.get("id", ""))):
        errors.append("id must use lowercase publisher/name")
    if not 1 <= len(str(data.get("name", ""))) <= 32:
        errors.append("name must contain 1-32 characters")
    if data.get("rarity", "common") not in RARITIES:
        errors.append("rarity must be common, uncommon, rare, epic, or legendary")
    palette = data.get("palette", {})
    if not isinstance(palette, dict):
        errors.append("palette must be a table")
    else:
        unknown_palette = set(palette) - PALETTE_KEYS
        if unknown_palette:
            errors.append(f"unknown palette fields: {', '.join(sorted(unknown_palette))}")
        for key, value in palette.items():
            if not isinstance(value, str) or not HEX_COLOR.fullmatch(value):
                errors.append(f"palette.{key} must use #RRGGBB")
    states = data.get("states", {})
    if not isinstance(states, dict):
        return errors + ["states must be a table"]
    unknown_states = set(states) - set(STATES)
    if unknown_states:
        errors.append(f"unknown states: {', '.join(sorted(unknown_states))}")
    for state in STATES:
        animation = states.get(state)
        if not isinstance(animation, dict):
            errors.append(f"missing state: {state}")
            continue
        unknown_animation = set(animation) - ANIMATION_KEYS
        if unknown_animation:
            errors.append(f"{state}: unknown fields: {', '.join(sorted(unknown_animation))}")
        interval = animation.get("interval_ms", 450)
        if not isinstance(interval, int) or not 90 <= interval <= 10_000:
            errors.append(f"{state}: interval_ms must be 90-10000")
        frames = animation.get("frames")
        if not isinstance(frames, list) or not 1 <= len(frames) <= 16:
            errors.append(f"{state}: frames must contain 1-16 frames")
            continue
        for frame_no, frame in enumerate(frames):
            if not isinstance(frame, list) or not 1 <= len(frame) <= 6:
                errors.append(f"{state}[{frame_no}]: frame must contain 1-6 lines")
                continue
            for line_no, line in enumerate(frame):
                if not isinstance(line, str) or any(
                    unicodedata.category(char) == "Cc" for char in line
                ):
                    errors.append(f"{state}[{frame_no}][{line_no}]: invalid control character")
                    continue
                tags = TAG.findall(line)
                unknown = set(tags) - TAGS
                if unknown or "{" in TAG.sub("", line) or "}" in TAG.sub("", line):
                    errors.append(f"{state}[{frame_no}][{line_no}]: invalid color markup")
                if width(TAG.sub("", line)) > 21:
                    errors.append(f"{state}[{frame_no}][{line_no}]: exceeds 21 columns")
    return errors


def main() -> None:
    parser = argparse.ArgumentParser(description="Validate a Bastion pet pack")
    parser.add_argument("pet_toml", type=Path)
    args = parser.parse_args()
    try:
        errors = validate(args.pet_toml)
    except (OSError, tomllib.TOMLDecodeError) as error:
        errors = [str(error)]
    if errors:
        print("pet pack invalid:", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        raise SystemExit(1)
    print(f"pet pack valid: {args.pet_toml}")


if __name__ == "__main__":
    main()
