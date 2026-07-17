#!/usr/bin/env sh
# Bastion one-line installer — served at https://bastion.run
#
#   curl -sSf https://bastion.run | sh
#
# Clones bastion-agent at the latest published release (falling back to the
# default branch if none exists yet) and hands off to installer.sh. Nothing
# here needs root; everything lands under your user.
set -eu

REPO="https://github.com/thewaifucorp/bastion-agent.git"
API="https://api.github.com/repos/thewaifucorp/bastion-agent/releases/latest"
SRC_DIR="${BASTION_SRC_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/bastion/src}"

info() { printf '\033[1;34m::\033[0m %s\n' "$1"; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$1" >&2; exit 1; }

command -v git >/dev/null 2>&1 || die "git is required — install it and re-run."
command -v docker >/dev/null 2>&1 || die "docker is required — see https://docs.docker.com/engine/install/"
docker compose version >/dev/null 2>&1 || die "docker compose v2 is required (the 'docker compose' plugin)."

# Resolve the latest release tag; fall back to the default branch.
REF=""
if command -v curl >/dev/null 2>&1; then
  REF="$(curl -fsSL "$API" 2>/dev/null | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n1 || true)"
fi
if [ -n "$REF" ]; then
  info "Latest release: $REF"
else
  info "No published release found — using the default branch."
fi

if [ -d "$SRC_DIR/.git" ]; then
  info "Updating existing checkout at $SRC_DIR"
  git -C "$SRC_DIR" fetch --tags --quiet
  [ -n "$REF" ] && git -C "$SRC_DIR" checkout --quiet "$REF"
  [ -n "$REF" ] || git -C "$SRC_DIR" pull --quiet --ff-only
else
  info "Cloning into $SRC_DIR"
  mkdir -p "$(dirname "$SRC_DIR")"
  if [ -n "$REF" ]; then
    git clone --quiet --depth 1 --branch "$REF" "$REPO" "$SRC_DIR"
  else
    git clone --quiet --depth 1 "$REPO" "$SRC_DIR"
  fi
fi

cd "$SRC_DIR"
info "Running installer…"
# When piped (curl | sh) this script has no stdin, so give the installer the
# terminal directly for its prompts. Falls back to non-interactive if there is
# no controlling TTY (e.g. CI).
if [ -r /dev/tty ]; then
  exec ./installer.sh </dev/tty
else
  exec ./installer.sh --non-interactive
fi
