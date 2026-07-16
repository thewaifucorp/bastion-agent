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

if grep -Eq '^[[:space:]]*container_name:' "$ROOT/docker-compose.yml"; then
  echo "Compose services must use project-scoped container names" >&2
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

mkdir -p "$tmp/mock-bin" "$tmp/home"
cat > "$tmp/mock-bin/docker" <<'EOF'
#!/usr/bin/env bash
set -Eeuo pipefail
case "${1:-}" in
  compose)
    case "${2:-}" in
      version|build|up) exit 0 ;;
      config)
        if [[ "${3:-}" == "--format" && "${4:-}" == "json" ]]; then
          printf '%s\n' '{' '  "services": {' '    "core": {' \
            '      "image": "bastion-core:latest"' '    }' '  }' '}'
        fi
        ;;
      images) exit 0 ;;
      *) exit 1 ;;
    esac
    ;;
  image) [[ "${2:-}" == "inspect" ]] ;;
  create) printf '%s\n' fake-container ;;
  cp)
    printf '#!/usr/bin/env bash\nprintf "mock bastion 0.1.1\\n"\n' > "$3"
    ;;
  rm) exit 0 ;;
  *) exit 1 ;;
esac
EOF
chmod 755 "$tmp/mock-bin/docker"

HOME="$tmp/home" PATH="$tmp/mock-bin:$PATH" GEMINI_API_KEY=test-only-key \
  "$INSTALLER" --dir "$tmp" --no-start --non-interactive >/dev/null

[[ -x "$tmp/.bastion/bin/bastion" ]]
[[ -x "$tmp/home/.local/bin/bastion" ]]
[[ "$(HOME="$tmp/home" "$tmp/home/.local/bin/bastion")" == "mock bastion 0.1.1" ]]

echo "installer smoke tests passed"
