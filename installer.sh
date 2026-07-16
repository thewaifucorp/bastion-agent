#!/usr/bin/env bash
set -Eeuo pipefail

readonly REPO_URL="https://github.com/thewaifucorp/bastion-agent.git"
readonly DEFAULT_INSTALL_DIR="${XDG_DATA_HOME:-${HOME}/.local/share}/bastion"

INSTALL_DIR="$DEFAULT_INSTALL_DIR"
NON_INTERACTIVE=0
PREPARE_ONLY=0
NO_START=0

info() { printf '\033[1;36m◈\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33mwarning:\033[0m %s\n' "$*" >&2; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

usage() {
  cat <<'EOF'
Bastion installer

Usage: ./installer.sh [options]

  --dir PATH          Install or update in PATH
  --non-interactive   Never prompt; provider keys must already be exported
  --prepare-only      Prepare .env without requiring or starting Docker
  --no-start          Validate and build, but do not start services
  -h, --help          Show this help

The installer preserves an existing .env and generates missing internal secrets.
EOF
}

while (($#)); do
  case "$1" in
    --dir) [[ $# -ge 2 ]] || die "--dir requires a path"; INSTALL_DIR="$2"; shift 2 ;;
    --non-interactive) NON_INTERACTIVE=1; shift ;;
    --prepare-only) PREPARE_ONLY=1; shift ;;
    --no-start) NO_START=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown option: $1" ;;
  esac
done

need() { command -v "$1" >/dev/null 2>&1 || die "required command not found: $1"; }

script_dir() {
  local source="${BASH_SOURCE[0]}"
  if [[ -f "$source" ]]; then
    (cd "$(dirname "$source")" && pwd -P)
  fi
}

install_or_update_repo() {
  local local_dir
  local_dir="$(script_dir || true)"

  if [[ -f "$INSTALL_DIR/Cargo.toml" && -f "$INSTALL_DIR/docker-compose.yml" ]]; then
    info "Using existing checkout: $INSTALL_DIR"
    return
  fi
  if [[ -n "$local_dir" && -f "$local_dir/Cargo.toml" && -f "$local_dir/docker-compose.yml" ]]; then
    INSTALL_DIR="$local_dir"
    info "Using installer checkout: $INSTALL_DIR"
    return
  fi

  need git
  if [[ -d "$INSTALL_DIR/.git" ]]; then
    info "Updating $INSTALL_DIR"
    git -C "$INSTALL_DIR" pull --ff-only
  elif [[ -e "$INSTALL_DIR" ]]; then
    die "$INSTALL_DIR exists but is not a Bastion checkout"
  else
    info "Cloning Bastion into $INSTALL_DIR"
    mkdir -p "$(dirname "$INSTALL_DIR")"
    git clone --depth 1 "$REPO_URL" "$INSTALL_DIR"
  fi
}

env_get() {
  local key="$1"
  [[ -f "$INSTALL_DIR/.env" ]] || return 0
  sed -n "s/^${key}=//p" "$INSTALL_DIR/.env" | tail -n 1
}

env_set() {
  local key="$1" value="$2" file="$INSTALL_DIR/.env" tmp
  tmp="${file}.tmp"
  awk -v key="$key" -v value="$value" '
    BEGIN { found=0 }
    index($0, key "=")==1 { if (!found) print key "=" value; found=1; next }
    { print }
    END { if (!found) print key "=" value }
  ' "$file" > "$tmp"
  chmod 600 "$tmp"
  mv "$tmp" "$file"
}

random_secret() {
  if command -v openssl >/dev/null 2>&1; then
    openssl rand -hex 32
  else
    od -An -N32 -tx1 /dev/urandom | tr -d ' \n'
  fi
}

copy_exported_secret() {
  local key="$1" value="${!1:-}"
  if [[ -n "$value" && -z "$(env_get "$key")" ]]; then
    env_set "$key" "$value"
  fi
}

configure_provider() {
  local keys=(ANTHROPIC_API_KEY OPENAI_API_KEY GEMINI_API_KEY OPENROUTER_API_KEY)
  local key
  for key in "${keys[@]}"; do copy_exported_secret "$key"; done
  for key in "${keys[@]}"; do
    [[ -n "$(env_get "$key")" ]] && return 0
  done

  if ((NON_INTERACTIVE)) || [[ ! -t 0 ]]; then
    warn "No provider key configured. Add one to $INSTALL_DIR/.env before starting Bastion."
    return 0
  fi

  printf '\nLLM provider: 1) Gemini  2) Anthropic  3) OpenAI  4) OpenRouter\nchoice [1]: '
  local choice provider_key model secret
  read -r choice
  case "${choice:-1}" in
    1) provider_key=GEMINI_API_KEY; model=gemini-2.5-flash ;;
    2) provider_key=ANTHROPIC_API_KEY; model=claude-sonnet-4-5 ;;
    3) provider_key=OPENAI_API_KEY; model=gpt-4.1 ;;
    4) provider_key=OPENROUTER_API_KEY; model=anthropic/claude-sonnet-4.5 ;;
    *) die "invalid provider choice" ;;
  esac
  printf '%s: ' "$provider_key"
  read -rs secret
  printf '\n'
  [[ -n "$secret" ]] || die "provider key cannot be empty"
  env_set "$provider_key" "$secret"
  env_set BASTION__AGENT__DEFAULT_MODEL "$model"
}

prepare_environment() {
  [[ -f "$INSTALL_DIR/.env.example" ]] || die "missing .env.example in $INSTALL_DIR"
  if [[ ! -f "$INSTALL_DIR/.env" ]]; then
    cp "$INSTALL_DIR/.env.example" "$INSTALL_DIR/.env"
    chmod 600 "$INSTALL_DIR/.env"
    info "Created .env"
  else
    chmod 600 "$INSTALL_DIR/.env"
    info "Preserving existing .env"
  fi

  [[ -n "$(env_get APP_JWT_SECRET)" ]] || env_set APP_JWT_SECRET "$(random_secret)"
  [[ -n "$(env_get BASTION_INFER_TOKEN)" ]] || env_set BASTION_INFER_TOKEN "$(random_secret)"
  [[ -n "$(env_get BASTION_BOOTSTRAP_TOKEN)" ]] || env_set BASTION_BOOTSTRAP_TOKEN "$(random_secret)"
  env_set BASTION_UID "$(id -u)"
  env_set BASTION_GID "$(id -g)"
  configure_provider
  info "Configuration prepared (internal secrets were not printed)."
}

run_compose() {
  need docker
  docker compose version >/dev/null 2>&1 || die "Docker Compose v2 is required"
  (cd "$INSTALL_DIR" && docker compose config --quiet)
  info "Building Bastion images"
  (cd "$INSTALL_DIR" && docker compose build --pull)
  if ((NO_START)); then
    info "Build complete; services were not started."
  else
    info "Starting Bastion"
    (cd "$INSTALL_DIR" && docker compose up -d --force-recreate)
    info "Bastion is starting at http://127.0.0.1:8080"
    info "Check readiness with: docker compose -f '$INSTALL_DIR/docker-compose.yml' ps"
  fi
}

main() {
  install_or_update_repo
  prepare_environment
  if ((PREPARE_ONLY)); then
    info "Preparation complete: $INSTALL_DIR"
  else
    run_compose
  fi
}

main
