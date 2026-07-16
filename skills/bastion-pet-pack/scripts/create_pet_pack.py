#!/usr/bin/env python3
import argparse
import json
import re
import shutil
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser(description="Scaffold a Bastion pet extension")
    parser.add_argument("id", help="lowercase publisher/name")
    parser.add_argument("name", help="display name")
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    if not re.fullmatch(r"[a-z0-9-]+/[a-z0-9-]+", args.id):
        parser.error("id must use lowercase publisher/name")
    if not 1 <= len(args.name) <= 32:
        parser.error("name must contain 1-32 characters")
    root = Path(__file__).resolve().parent.parent
    template = root / "assets" / "pet-pack"
    if args.output.exists() and any(args.output.iterdir()):
        parser.error("output directory must be empty")
    shutil.copytree(template, args.output, dirs_exist_ok=True)
    for path in (args.output / "extension.toml", args.output / "ui" / "pet.toml"):
        contents = path.read_text(encoding="utf-8")
        contents = contents.replace("example/terminal-pet", args.id)
        encoded_name = json.dumps(args.name, ensure_ascii=False)
        contents = contents.replace('name = "Terminal Pet"', f"name = {encoded_name}")
        path.write_text(contents, encoding="utf-8")
    print(args.output / "ui" / "pet.toml")


if __name__ == "__main__":
    main()
