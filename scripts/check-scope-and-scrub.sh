#!/usr/bin/env bash
#
# check-scope-and-scrub.sh — Loop 3-D (docs/revamp/C3-cloud-ready-design.md),
# acceptance criterion 5 + decision #18 (docs/revamp/BACKLOG.md).
#
# Two independent guards, run together because both gate what may exist in
# this PUBLIC repo:
#
#   1. Cloud-concept scope guard — the Core must never gain a billing,
#      marketplace, tenancy, or control-plane SYMBOL (a `struct`/`enum`/
#      `trait`/`fn`/`mod`/`const`/`static`/`type` DECLARATION, not a prose
#      mention). Design docs are explicitly allowed to name these concepts
#      when documenting them as OUT OF SCOPE (e.g. C3-cloud-ready-design.md's
#      own "Fora de escopo" section) — this guard only scans compiled `.rs`
#      code (src/, crates/*/src/, tests/, examples/*/src/), never docs/.
#      Comment-only lines (`//`, `///`, `//!`) are excluded — a code comment
#      explaining why the kernel does NOT have a concept (this script's own
#      doc comments included) is not a violation.
#
#   2. Corporate-name scrub guard (decision #18) — a small, deliberately
#      short, low-collision blocklist of proprietary names scrubbed from
#      this repo's public tip on 2026-06-30. Scans EVERY tracked file (not
#      just code) — a leaked name in a doc, a comment, or a config file is
#      equally a leak. `Bastion Cloud` is NOT on this list — it stays
#      (decision: only the closed-source corporate/product names leave).
#
# Exit 0 on success, exit 1 on any violation (prints every violation found).
# Implemented as a single Python pass (like scripts/dump-public-api.sh) —
# a shell line-by-line loop over every tracked file is both slow and fragile
# for this repo's size.

set -uo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null || { cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd; })"
cd "$REPO_ROOT"

python3 - <<'PYEOF'
import os
import re
import subprocess
import sys

fail = False

# ── Guard 1: cloud-concept scope (code only, declarations only) ─────────────

CODE_DIRS = ["src", "crates", "tests", "examples"]
BLOCKED_ROOTS = r"(?:billing|marketplace|tenant|tenancy|controlplane|control_plane)"
DECL_RE = re.compile(
    r'^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+|unsafe\s+|const\s+)*'
    r'(?:struct|enum|trait|fn|mod|const|static|type)\s+'
    r'([A-Za-z_][A-Za-z0-9_]*)',
)
root_re = re.compile(BLOCKED_ROOTS, re.IGNORECASE)

print(f"check-scope-and-scrub: guard 1 — cloud-concept symbols in code ({BLOCKED_ROOTS})")

rs_files = []
for base in CODE_DIRS:
    if not os.path.isdir(base):
        continue
    for dirpath, _, files in os.walk(base):
        for fname in files:
            if fname.endswith(".rs"):
                rs_files.append(os.path.join(dirpath, fname))

for path in sorted(rs_files):
    try:
        with open(path, encoding="utf-8", errors="replace") as fh:
            lines = fh.readlines()
    except OSError:
        continue
    for lineno, line in enumerate(lines, start=1):
        stripped = line.strip()
        if stripped.startswith("//"):
            continue
        m = DECL_RE.match(line)
        if m and root_re.search(m.group(1)):
            print(
                f"check-scope-and-scrub: FORBIDDEN — cloud-concept symbol in "
                f"{path}:{lineno}: {stripped}",
                file=sys.stderr,
            )
            fail = True

# ── Guard 2: corporate-name scrub (whole repo, tracked files) ───────────────

print("check-scope-and-scrub: guard 2 — corporate-name scrub")
# Deliberately short and specific — see header. Extend only with an actual
# leaked proprietary name, never a generic English word (false positives on
# common words would make this check impossible to keep green).
# `waifucorp`/`thewaifucorp` are the project's PUBLIC identity — the GitHub org
# that hosts this repo, the README attribution, and waifucorp.org — not leaked
# closed-source names, so they stay (same rationale as `Bastion Cloud`, header).
BLOCKED_NAMES = ["katsui"]
# This script itself must name the blocked strings to check for them — the
# ONE deliberate, self-referential exception, not a leak.
EXEMPT_FILES = {"scripts/check-scope-and-scrub.sh"}

try:
    tracked = subprocess.run(
        ["git", "ls-files"], capture_output=True, text=True, check=True
    ).stdout.splitlines()
except (subprocess.CalledProcessError, FileNotFoundError):
    tracked = []
    for dirpath, dirnames, files in os.walk("."):
        dirnames[:] = [d for d in dirnames if d not in (".git", "target", ".aag")]
        for fname in files:
            tracked.append(os.path.join(dirpath, fname))

for name in BLOCKED_NAMES:
    hits = []
    for relpath in tracked:
        if relpath in EXEMPT_FILES:
            continue
        if not os.path.isfile(relpath):
            continue
        try:
            with open(relpath, encoding="utf-8", errors="replace") as fh:
                content = fh.read()
        except OSError:
            continue
        if name in content.lower():
            hits.append(relpath)
    if hits:
        print(
            f"check-scope-and-scrub: FORBIDDEN — corporate name '{name}' found in:",
            file=sys.stderr,
        )
        for h in hits:
            print(f"  {h}", file=sys.stderr)
        fail = True

print("---")
if fail:
    print("check-scope-and-scrub: FAIL — see violations above.", file=sys.stderr)
    sys.exit(1)
else:
    print(
        "check-scope-and-scrub: PASS — no cloud-concept symbols in code, "
        "no corporate names in tracked files."
    )
    sys.exit(0)
PYEOF
