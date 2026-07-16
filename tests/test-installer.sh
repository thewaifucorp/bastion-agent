#!/usr/bin/env bash
set -Eeuo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
INSTALLER="$ROOT/installer.sh"

bash -n "$INSTALLER"
"$INSTALLER" --help >/dev/null

if grep -Eqi 'openclaw|clawhub|evolution.?api|pokedev|bastion\.json|samurai-py' "$INSTALLER"; then
  echo "installer contains a removed legacy integration" >&2
  exit 1
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
cp "$ROOT/.env.example" "$ROOT/Cargo.toml" "$ROOT/docker-compose.yml" "$ROOT/bastion.toml" "$tmp/"

GEMINI_API_KEY=test-only-key "$INSTALLER" --dir "$tmp" --prepare-only --non-interactive >/dev/null

grep -q '^GEMINI_API_KEY=test-only-key$' "$tmp/.env"
grep -Eq '^APP_JWT_SECRET=.{64}$' "$tmp/.env"
grep -Eq '^BASTION_INFER_TOKEN=.{64}$' "$tmp/.env"
grep -Eq '^BASTION_BOOTSTRAP_TOKEN=.{64}$' "$tmp/.env"
[[ "$(stat -c '%a' "$tmp/.env" 2>/dev/null || stat -f '%Lp' "$tmp/.env")" == 600 ]]

first_jwt="$(sed -n 's/^APP_JWT_SECRET=//p' "$tmp/.env")"
first_infer="$(sed -n 's/^BASTION_INFER_TOKEN=//p' "$tmp/.env")"
"$INSTALLER" --dir "$tmp" --prepare-only --non-interactive >/dev/null
[[ "$(sed -n 's/^APP_JWT_SECRET=//p' "$tmp/.env")" == "$first_jwt" ]]
[[ "$(sed -n 's/^BASTION_INFER_TOKEN=//p' "$tmp/.env")" == "$first_infer" ]]

echo "installer smoke tests passed"
