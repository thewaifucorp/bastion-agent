#!/usr/bin/env bash
set -Eeuo pipefail

readonly REPO_URL="https://github.com/thewaifucorp/bastion-agent.git"
readonly DEFAULT_INSTALL_DIR="${XDG_DATA_HOME:-${HOME}/.local/share}/bastion"
readonly DEFAULT_BIN_DIR="${XDG_BIN_HOME:-${HOME}/.local/bin}"

INSTALL_DIR="$DEFAULT_INSTALL_DIR"
NON_INTERACTIVE=0
PREPARE_ONLY=0
NO_START=0
UPDATE=0
RELEASE_TAG=""

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
  --update            Update an existing checkout to the latest release tag
  --release TAG       Release tag to install with --update (for the host helper)
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
    --update) UPDATE=1; shift ;;
    --release) [[ $# -ge 2 ]] || die "--release requires a tag"; RELEASE_TAG="$2"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown option: $1" ;;
  esac
done

[[ -z "$RELEASE_TAG" || "$RELEASE_TAG" =~ ^v[0-9]+\.[0-9]+\.[0-9]+([-.][0-9A-Za-z.]+)?$ ]] || die "--release must be a semantic vX.Y.Z tag"

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
    if ((UPDATE)); then update_checkout; fi
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

update_checkout() {
  need git
  [[ -d "$INSTALL_DIR/.git" ]] || die "updates require a Git checkout at $INSTALL_DIR"
  git -C "$INSTALL_DIR" diff --quiet || die "refusing to update: tracked working-tree changes exist"
  git -C "$INSTALL_DIR" diff --cached --quiet || die "refusing to update: staged changes exist"
  info "Fetching Bastion releases"
  git -C "$INSTALL_DIR" fetch --tags --force origin
  local target="${RELEASE_TAG:-}"
  if [[ -z "$target" ]]; then
    target="$(git -C "$INSTALL_DIR" tag --list 'v[0-9]*' --sort=-version:refname | head -n 1)"
  fi
  [[ -n "$target" ]] || die "no Bastion release tag was found"
  git -C "$INSTALL_DIR" rev-parse --verify --quiet "refs/tags/$target^{commit}" >/dev/null \
    || die "release tag $target is not available from origin"
  UPDATE_PREVIOUS_REF="$(git -C "$INSTALL_DIR" rev-parse HEAD)"
  UPDATE_TARGET="$target"
  info "Updating Bastion to $target"
  git -C "$INSTALL_DIR" checkout --detach "$target"
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

configure_backend() {
  # Preserve an explicit previous choice on updates. The CLIs are installed in
  # every core image; only this selection decides whether Bastion uses an API
  # model or a logged-in subscription runtime for conversations.
  [[ -n "$(env_get BASTION_BACKEND_CONVERSATION)" ]] && return 0
  if ((NON_INTERACTIVE)) || [[ ! -t 0 ]]; then
    env_set BASTION_BACKEND_CONVERSATION model
    env_set BASTION_BACKEND_AUTH ""
    configure_provider
    return 0
  fi

  printf '\nConversation backend: 1) API provider  2) Claude Code subscription  3) Codex subscription  4) OpenCode subscription\nchoice [1]: '
  local choice
  read -r choice
  case "${choice:-1}" in
    1)
      env_set BASTION_BACKEND_CONVERSATION model
      env_set BASTION_BACKEND_AUTH ""
      configure_provider
      ;;
    2)
      env_set BASTION_BACKEND_CONVERSATION runtime:acpx_claude
      env_set BASTION_BACKEND_AUTH claude-subscription
      info "Run 'bastion connect claude' after startup to complete its browser login."
      ;;
    3)
      env_set BASTION_BACKEND_CONVERSATION runtime:codex_app_server
      env_set BASTION_BACKEND_AUTH codex-subscription
      info "Run 'bastion connect codex' after startup to complete its ChatGPT login."
      ;;
    4)
      env_set BASTION_BACKEND_CONVERSATION runtime:acpx_opencode
      env_set BASTION_BACKEND_AUTH opencode-subscription
      info "Run 'bastion connect opencode' after startup to complete its login."
      ;;
    *) die "invalid backend choice" ;;
  esac
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
  [[ -n "$(env_get BASTION_UPDATER_TOKEN)" ]] || env_set BASTION_UPDATER_TOKEN "$(random_secret)"
  env_set BASTION_UID "$(id -u)"
  env_set BASTION_GID "$(id -g)"

  # Kill port footgun: docker-compose.yml now hardcodes BASTION_WEBHOOK_ADDR to
  # 0.0.0.0:8080 in its `environment:` block, which overrides env_file — but a
  # stale entry left in .env from an older install is confusing to read next
  # to BASTION_HTTP_PORT (the only port that's actually configurable). Strip it.
  if grep -q '^BASTION_WEBHOOK_ADDR=' "$INSTALL_DIR/.env" 2>/dev/null; then
    sed -i '/^BASTION_WEBHOOK_ADDR=/d' "$INSTALL_DIR/.env"
    info "Removed stale BASTION_WEBHOOK_ADDR from .env (docker-compose.yml now hardcodes it)"
  fi

  configure_backend
  info "Configuration prepared (internal secrets were not printed)."
}

run_compose() {
  local publish_host http_port client_host client_url
  need docker
  docker compose version >/dev/null 2>&1 || die "Docker Compose v2 is required"
  (cd "$INSTALL_DIR" && docker compose config --quiet)
  info "Building Bastion images"
  (cd "$INSTALL_DIR" && docker compose build --pull)
  install_cli
  if ((NO_START)); then
    info "Build complete; services were not started."
  else
    info "Starting Bastion"
    (cd "$INSTALL_DIR" && docker compose up -d --force-recreate)
    ensure_updater
    publish_host="$(env_get BASTION_PUBLISH_HOST)"
    http_port="$(env_get BASTION_HTTP_PORT)"
    client_url="$(env_get BASTION_URL)"
    publish_host="${publish_host:-127.0.0.1}"
    http_port="${http_port:-8080}"
    case "$publish_host" in
      0.0.0.0|::) publish_host=127.0.0.1 ;;
    esac
    client_host="$publish_host"
    [[ "$client_host" == *:* ]] && client_host="[$client_host]"
    client_url="${client_url:-http://${client_host}:${http_port}}"
    info "Bastion is starting at $client_url"
    info "Check readiness with: docker compose -f '$INSTALL_DIR/docker-compose.yml' ps"
  fi
}

ensure_updater() {
  local runtime_bin socket pid_file old_pid token
  runtime_bin="$INSTALL_DIR/.bastion/bin/bastion"
  socket="$INSTALL_DIR/.bastion/updater.sock"
  pid_file="$INSTALL_DIR/.bastion/updater.pid"
  token="$(env_get BASTION_UPDATER_TOKEN)"
  [[ -x "$runtime_bin" && -n "$token" ]] || die "host updater prerequisites are missing"
  if [[ -f "$pid_file" ]]; then
    old_pid="$(cat "$pid_file" 2>/dev/null || true)"
    [[ -z "$old_pid" ]] || kill "$old_pid" 2>/dev/null || true
  fi
  rm -f "$socket"
  (
    umask 077
    cd "$INSTALL_DIR"
    nohup "$runtime_bin" updater serve --socket "$socket" --token "$token" \
      >> "$INSTALL_DIR/.bastion/updater.log" 2>&1 &
    echo $! > "$pid_file"
  )
  info "Enabled authenticated channel updates. Use /update apply from a trusted channel."
}

wait_for_core_health() {
  local attempt
  for attempt in $(seq 1 30); do
    if (cd "$INSTALL_DIR" && docker compose exec -T core curl --fail --silent http://127.0.0.1:8080/healthz >/dev/null 2>&1); then
      return 0
    fi
    sleep 2
  done
  return 1
}

rollback_update() {
  [[ -n "${UPDATE_PREVIOUS_REF:-}" ]] || return 1
  warn "Updated release did not become healthy; rolling back"
  git -C "$INSTALL_DIR" checkout --detach "$UPDATE_PREVIOUS_REF"
  run_compose
  wait_for_core_health || die "rollback deployment also failed health checks"
}

install_cli() {
  local image container runtime_dir runtime_bin launcher tmp_launcher
  image="$(
    cd "$INSTALL_DIR"
    docker compose config --format json | awk '
      /^    "core": \{/ { in_core = 1; next }
      in_core && /^    "[^"]+": \{/ { in_core = 0 }
      in_core && /^      "image": "/ {
        value = $0
        sub(/^      "image": "/, "", value)
        sub(/",?$/, "", value)
        print value
      }
    '
  )"
  [[ -n "$image" ]] || die "could not resolve the Bastion core image from Compose configuration"
  docker image inspect "$image" >/dev/null 2>&1 \
    || die "built Bastion core image is unavailable: $image"

  runtime_dir="$INSTALL_DIR/.bastion/bin"
  runtime_bin="$runtime_dir/bastion"
  mkdir -p "$runtime_dir" "$DEFAULT_BIN_DIR"

  container="$(docker create "$image")"
  [[ -n "$container" ]] || die "could not create a temporary container for CLI installation"
  if ! docker cp "$container:/usr/local/bin/bastion" "$runtime_bin.tmp"; then
    docker rm "$container" >/dev/null 2>&1 || true
    die "could not extract the Bastion CLI from the built image"
  fi
  docker rm "$container" >/dev/null
  chmod 755 "$runtime_bin.tmp"
  mv "$runtime_bin.tmp" "$runtime_bin"

  launcher="$DEFAULT_BIN_DIR/bastion"
  tmp_launcher="$launcher.tmp"
  printf '#!/usr/bin/env bash\nset -Eeuo pipefail\ncd %q\nexec %q "$@"\n' \
    "$INSTALL_DIR" "$runtime_bin" > "$tmp_launcher"
  chmod 755 "$tmp_launcher"
  mv "$tmp_launcher" "$launcher"
  info "Installed CLI: $launcher"
  case ":$PATH:" in
    *":$DEFAULT_BIN_DIR:"*) ;;
    *) warn "Add $DEFAULT_BIN_DIR to PATH to run: bastion" ;;
  esac
  install_completions "$launcher" || warn "shell completion setup skipped"
}

# Fase 3.6: best-effort shell completions — bash gets installed automatically
# (a missing/unwritable completions dir is a warning, never a hard failure:
# the CLI itself is already fully installed by this point). zsh/fish are
# printed as one-line instructions instead of guessing the user's fpath/
# fish config layout, which varies too much to install into safely.
install_completions() {
  local launcher="$1"
  local bash_comp_dir="${XDG_DATA_HOME:-$HOME/.local/share}/bash-completion/completions"

  if mkdir -p "$bash_comp_dir" 2>/dev/null \
      && "$launcher" completions bash > "$bash_comp_dir/bastion.tmp" 2>/dev/null; then
    mv "$bash_comp_dir/bastion.tmp" "$bash_comp_dir/bastion"
    info "Installed bash completions: $bash_comp_dir/bastion"
  else
    rm -f "$bash_comp_dir/bastion.tmp" 2>/dev/null || true
    warn "could not install bash completions in $bash_comp_dir"
  fi

  info "For zsh: $launcher completions zsh > \"\${fpath[1]}/_bastion\""
  info "For fish: $launcher completions fish > ~/.config/fish/completions/bastion.fish"
}

main() {
  install_or_update_repo
  prepare_environment
  if ((PREPARE_ONLY)); then
    info "Preparation complete: $INSTALL_DIR"
  else
    run_compose
    if ((UPDATE)) && ! wait_for_core_health; then
      rollback_update
      die "update to ${UPDATE_TARGET:-requested release} failed health checks and was rolled back"
    fi
  fi
}

main
