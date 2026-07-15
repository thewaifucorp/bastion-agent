#!/usr/bin/env bash
# ============================================================================
# BASTION INSTALLER — Agnóstico, Robusto e Orientado a Wizard
# ============================================================================
# Suporta configuração via:
#   1. Wizard interativo (padrão)
#   2. Variáveis de ambiente (CI/CD ou automação)
#   3. Arquivo .env existente (preserva configurações)
#
# Uso:
#   bash <(curl -fsSL https://bastion.run/install)
#   BASTION_WIZARD=false LLM_PROVIDER=anthropic bash installer.sh
# ============================================================================

set -euo pipefail

REPO_URL="https://github.com/samurai-py/bastion.git"
if [ -f "AGENTS.md" ]; then
  INSTALL_DIR="$(pwd)"
else
  INSTALL_DIR="${BASTION_DIR:-$HOME/bastion}"
fi
WIZARD_MODE="${BASTION_WIZARD:-true}"

# ── Colors ────────────────────────────────────────────────────────
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
RESET='\033[0m'

info()    { echo -e "${CYAN}[bastion]${RESET} $*"; }
success() { echo -e "${GREEN}[bastion]${RESET} $*"; }
warn()    { echo -e "${YELLOW}[bastion]${RESET} $*"; }
error()   { echo -e "${RED}[bastion] ERROR:${RESET} $*" >&2; }
step()    { echo -e "\n${BOLD}▶ $*${RESET}"; }

# PKG-06, D-11: escalation hook — prints claude -p command on failure
_on_failure() {
  local step="${1:-unknown}"
  local err="${2:-}"
  error "Installation failed at: ${step}"
  echo ""
  echo "Paste this command into Claude Code to diagnose and resume:"
  echo ""
  printf '  claude -p "Bastion installer failed at step: %s. Error: %s. OS: %s. Diagnose and provide next steps."\n' \
    "${step}" "${err}" "$(uname -srm)"
  echo ""
  echo "(Claude Code: https://claude.ai/claude-code — or ask a human for help)"
  exit 1
}
trap '_on_failure "unexpected error" "$BASH_COMMAND"' ERR

banner() {
    echo -e "${BOLD}"
    echo "    ██████╗  █████╗ ███████╗████████╗██╗ ██████╗ ███╗   ██╗"
    echo "    ██╔══██╗██╔══██╗██╔════╝╚══██╔══╝██║██╔═══██╗████╗  ██║"
    echo "    ██████╔╝███████║███████╗   ██║   ██║██║   ██║██╔██╗ ██║"
    echo "    ██╔══██╗██╔══██║╚════██║   ██║   ██║██║   ██║██║╚██╗██║"
    echo "    ██████╔╝██║  ██║███████║   ██║   ██║╚██████╔╝██║ ╚████║"
    echo "    ╚═════╝ ╚═╝  ╚═╝╚══════╝   ╚═╝   ╚═╝ ╚═════╝ ╚═╝  ╚═══╝"
    echo -e "${RESET}"
    echo "    >> Instalador Agnóstico v3.0 | Self-Hosted Life OS"
    echo ""
}

# ── Utility Functions ─────────────────────────────────────────────
_env_get() {
  local key="$1"
  grep -E "^${key}=" .env 2>/dev/null | cut -d'=' -f2- | tr -d '"' | tr -d "'" || true
}

_env_set() {
  local key="$1"
  local val="$2"
  if grep -qE "^${key}=" .env 2>/dev/null; then
    sed -i.bak "s|^${key}=.*|${key}=${val}|" .env && rm -f .env.bak
  else
    echo "${key}=${val}" >> .env
  fi
}

_ask() {
  local prompt="$1"
  local varname="$2"
  printf "%b" "$prompt"
  read -r "$varname"
}

_ask_or_env() {
  # Usa variável de ambiente se existir, senão pergunta
  local prompt="$1"
  local varname="$2"
  local env_var="$3"
  
  if [ -n "${!env_var:-}" ]; then
    eval "$varname=\"${!env_var}\""
    info "Usando $env_var da variável de ambiente"
  elif [ "$WIZARD_MODE" = "true" ]; then
    _ask "$prompt" "$varname"
  else
    error "$env_var não definida e wizard desabilitado"
    exit 1
  fi
}

_select_or_env() {
  # Menu de seleção ou variável de ambiente
  local prompt="$1"
  local varname="$2"
  local env_var="$3"
  shift 3
  local options=("$@")
  
  if [ -n "${!env_var:-}" ]; then
    eval "$varname=\"${!env_var}\""
    info "Usando $env_var=${!env_var}"
    return
  fi
  
  if [ "$WIZARD_MODE" = "false" ]; then
    error "$env_var não definida e wizard desabilitado"
    exit 1
  fi
  
  echo ""
  echo "$prompt"
  PS3="Escolha [1-${#options[@]}]: "
  select opt in "${options[@]}"; do
    if [ -n "$opt" ]; then
      eval "$varname=\"$opt\""
      break
    fi
  done
}

# ── Plugin & Skill Installers ─────────────────────────────────────
SAGE_INSTALLED=false
COMPOSIO_INSTALLED=false
CORE_SKILLS_INSTALLED=()
CORE_SKILLS_FAILED=()

install_plugin() {
  local plugin="$1"
  local force="${2:-}"
  local plugin_id_override="${3:-}"
  info "Instalando plugin ${plugin}..."

  # Usa override se fornecido, senão extrai do nome do pacote
  local plugin_id
  if [ -n "$plugin_id_override" ]; then
    plugin_id="$plugin_id_override"
  else
    plugin_id=$(echo "$plugin" | sed 's|.*/||')
  fi

  local ext_dir="$HOME/.bastion/extensions/${plugin_id}"

  # Skip se já instalado
  if [ -d "$ext_dir" ]; then
    success "Plugin ${plugin_id} já instalado — pulando."
    return 0
  fi

  # Instala via clawhub
  if clawhub install "$plugin" 2>/dev/null; then
    success "Plugin ${plugin} instalado."
  elif [ "$force" = "--force" ]; then
    warn "Scanner detectou padrão suspeito em ${plugin} — forçando instalação (falso positivo conhecido)."
    if clawhub install "$plugin" --force; then
      success "Plugin ${plugin} instalado."
    else
      warn "Falha ao instalar plugin ${plugin}. Continuando sem ele."
    fi
  else
    warn "Falha ao instalar plugin ${plugin}. Continuando sem ele."
  fi
}

install_skill() {
  local skill="$1"
  info "Instalando skill ${skill}..."
  if clawhub install "$skill"; then
    success "Skill ${skill} instalada."
    CORE_SKILLS_INSTALLED+=("$skill")
  else
    warn "Falha ao instalar skill ${skill}. Continuando..."
    CORE_SKILLS_FAILED+=("$skill")
  fi
}

# Idempotency: exit 0 if Bastion is already installed and running
_check_already_installed() {
  if [ -f .env ] && command -v docker &>/dev/null && docker compose ps --quiet 2>/dev/null | grep -q "bastion"; then
    success "Bastion is already installed and running."
    info "Run 'docker compose restart' to restart, or 'docker compose down' to stop."
    exit 0
  fi
}

# PKG-05, D-10: Docker auto-install with sudo confirmation
_install_docker() {
  echo ""
  warn "Docker not found. Install automatically? (requires sudo)"
  read -rp "[bastion] Install Docker now? [y/N] " _install_choice
  if [ "${_install_choice:-n}" != "y" ] && [ "${_install_choice:-n}" != "Y" ]; then
    _on_failure "docker-install" "User declined Docker installation. Install manually: https://docs.docker.com/get-docker/"
  fi
  info "Downloading and running Docker official install script..."
  curl -fsSL https://get.docker.com | sudo sh
  # Add current user to docker group (avoids sudo for docker commands)
  if getent group docker &>/dev/null; then
    sudo usermod -aG docker "$USER" 2>/dev/null || true
    warn "Added $USER to docker group. You may need to log out/in for it to take effect."
  fi
  success "Docker installed."
}

check_docker() {
  if command -v docker &>/dev/null; then
    success "Docker encontrado: $(docker --version | cut -d' ' -f3 | tr -d ',')"
  else
    _install_docker
  fi
}

check_docker_compose() {
  if docker compose version &>/dev/null 2>&1; then
    success "Docker Compose encontrado (plugin)"
  else
    _on_failure "docker-compose-check" "docker compose plugin not found. Install: https://docs.docker.com/compose/install/"
  fi
}

# ── 1. Check prerequisites ────────────────────────────────────────
banner

# --dry-run: validate dependencies and print steps only (no actual install)
if [ "${1:-}" = "--dry-run" ]; then
  info "Dry run — checking dependencies and printing steps only"
  check_docker
  check_docker_compose
  success "Dry run complete."
  exit 0
fi

_check_already_installed

step "Verificando pré-requisitos..."

check_docker
check_docker_compose

if ! docker info &>/dev/null 2>&1; then
  error "Docker daemon não está rodando. Inicie o Docker e tente novamente."
  exit 1
fi
success "Docker daemon está ativo."

# ── Verificar Node.js e CLIs ──────────────────────────────────────
check_node() {
  if ! command -v node &>/dev/null; then
    warn "Node.js não encontrado. Instalando via NodeSource..."
    curl -fsSL https://deb.nodesource.com/setup_lts.x | sudo -E bash -
    sudo apt-get install -y nodejs
    success "Node.js instalado: $(node --version)"
  else
    success "Node.js encontrado: $(node --version)"
  fi
}

check_clawhub() {
  if ! command -v clawhub &>/dev/null; then
    info "Instalando clawhub CLI..."
    npm install -g clawhub@latest
    success "clawhub instalado."
  else
    success "clawhub encontrado: $(clawhub --version 2>/dev/null || echo 'ok')"
  fi
}

check_node
check_clawhub

# ── 2. Clone or update repository ────────────────────────────────
step "Setting up Bastion directory..."

if [ -d "$INSTALL_DIR/.git" ]; then
  warn "Repository already exists at $INSTALL_DIR — skipping clone."
else
  info "Cloning Bastion into $INSTALL_DIR ..."
  git clone "$REPO_URL" "$INSTALL_DIR"
  success "Repository cloned successfully."
fi

cd "$INSTALL_DIR"

# Stop any running containers before reconfiguring
if [ -f "docker-compose.yml" ]; then
  info "Deseja realizar uma instalação limpa (apagar volumes e configurações antigas)? [s/N]"
  _ask "Confirma limpeza profunda? " clean_confirm
  if [[ "$clean_confirm" =~ ^[sS]$ ]]; then
    step "Limpando instalação anterior..."
    docker compose down -v --remove-orphans 2>/dev/null || true
    # Limpar config gerado pelo Bastion (preserva repo root)
    info "Removendo configuração antiga..."
    docker run --rm -v "$(pwd):/app" alpine sh -c \
      "chown -R $(id -u):$(id -g) /app/config 2>/dev/null; rm -rf /app/config /app/.env"
    # Remover plugins instalados localmente
    rm -rf "$HOME/.bastion/extensions"
    success "Limpeza concluída."
  else
    docker compose down --remove-orphans 2>/dev/null || true
  fi
fi

# ── 3. Copy .env.example → .env (idempotent) ─────────────────────
step "Configuring environment..."

if [ -f ".env" ]; then
  warn ".env already exists — preserving your configuration."
else
  cp .env.example .env
  success ".env created from .env.example."
fi

# ── 4. Configuração Dinâmica: LLM Provider ───────────────────────
step "Configurando LLM Provider..."

EXISTING_LLM=$(_env_get "OPENROUTER_API_KEY")$(_env_get "ANTHROPIC_API_KEY")$(_env_get "OPENAI_API_KEY")$(_env_get "GEMINI_API_KEY")$(_env_get "GROQ_API_KEY")

if [ -z "$EXISTING_LLM" ]; then
  _select_or_env "Qual LLM você quer usar?" llm_choice LLM_PROVIDER \
    "OpenRouter (recomendado — modelos gratuitos)" \
    "Groq (gratuito, rápido)" \
    "Google Gemini (gratuito)" \
    "Anthropic Claude (pago, melhor qualidade)" \
    "OpenAI GPT (pago, popular)"

  case "$llm_choice" in
    *OpenRouter*)
      info "Crie sua chave gratuita em: https://openrouter.ai/keys"
      _ask_or_env "$(echo -e "${CYAN}Cole sua OPENROUTER_API_KEY: ${RESET}")" llm_key OPENROUTER_API_KEY
      _env_set "OPENROUTER_API_KEY" "$llm_key"
      
      # Permite escolher o modelo do OpenRouter
      if [ "$WIZARD_MODE" = "true" ] && [ -n "$llm_key" ]; then
        info "Buscando modelos disponíveis na OpenRouter..."
        
        # Busca modelos, filtra os que possuem os nomes fornecidos ou os top 10 por uso se não houver filtro
        OR_MODELS_JSON=$(curl -s "https://openrouter.ai/api/v1/models")
        
        # Se falhar, usa os básicos como fallback
        if [ $? -eq 0 ] && [ -n "$OR_MODELS_JSON" ]; then
          # Seleciona os 15 modelos mais relevantes (simplificado)
          mapfile -t OR_OPTIONS < <(echo "$OR_MODELS_JSON" | jq -r '.data[0:15] | .[] | "\(.id) (\(.name))"')
          OR_OPTIONS+=("Outro (digitar manualmente)")
          
          _select_or_env "Qual modelo do OpenRouter você quer usar?" or_model_select OPENROUTER_MODEL "${OR_OPTIONS[@]}"
          
          if [[ "$or_model_select" == *"Outro"* ]]; then
             _ask "$(echo -e "${CYAN}Digite o ID completo do modelo (ex: google/gemini-2.0-flash-lite-001): ${RESET}")" or_custom_id
             _env_set "OPENROUTER_MODEL" "$or_custom_id"
          else
             # Extrai apenas o ID (parte antes do espaço e parênteses)
             selected_id=$(echo "$or_model_select" | cut -d' ' -f1)
             _env_set "OPENROUTER_MODEL" "$selected_id"
          fi
        else
          warn "Falha ao buscar modelos dinamicamente. Usando padrões."
          _select_or_env "Qual modelo do OpenRouter?" model_choice OPENROUTER_MODEL \
            "openai/gpt-oss-20b:free" \
            "meta-llama/llama-3.3-70b-instruct:free" \
            "google/gemini-2.0-flash-lite-001"
          _env_set "OPENROUTER_MODEL" "$(echo "$model_choice" | cut -d' ' -f1)"
        fi
      fi
      success "OpenRouter configurado."
      ;;
    *Groq*)
      info "Crie sua chave gratuita em: https://console.groq.com"
      _ask_or_env "$(echo -e "${CYAN}Cole sua GROQ_API_KEY: ${RESET}")" llm_key GROQ_API_KEY
      _env_set "GROQ_API_KEY" "$llm_key"
      success "Groq configurado."
      ;;
    *Gemini*)
      info "Crie sua chave em: https://aistudio.google.com/app/apikey"
      _ask_or_env "$(echo -e "${CYAN}Cole sua GEMINI_API_KEY: ${RESET}")" llm_key GEMINI_API_KEY
      _env_set "GEMINI_API_KEY" "$llm_key"
      success "Gemini configurado."
      ;;
    *Claude*)
      info "Crie sua chave em: https://console.anthropic.com"
      _ask_or_env "$(echo -e "${CYAN}Cole sua ANTHROPIC_API_KEY: ${RESET}")" llm_key ANTHROPIC_API_KEY
      _env_set "ANTHROPIC_API_KEY" "$llm_key"
      success "Anthropic configurado."
      ;;
    *OpenAI*)
      info "Crie sua chave em: https://platform.openai.com/api-keys"
      _ask_or_env "$(echo -e "${CYAN}Cole sua OPENAI_API_KEY: ${RESET}")" llm_key OPENAI_API_KEY
      _env_set "OPENAI_API_KEY" "$llm_key"
      success "OpenAI configurado."
      ;;
    *)
      warn "Opção inválida. Configure manualmente em .env depois."
      ;;
  esac
else
  success "LLM já configurado no .env."
fi

# ── 5. Configuração Dinâmica: Canal de Mensagens ─────────────────
step "Configurando canal de mensagens..."

PRIMARY_CHANNEL=$(_env_get "PRIMARY_CHANNEL")

if [ -z "$PRIMARY_CHANNEL" ]; then
  _select_or_env "Qual canal você quer configurar?" channel_choice PRIMARY_CHANNEL \
    "Telegram" \
    "WhatsApp (via Evolution API)" \
    "Discord" \
    "Slack" \
    "Pular (configurar depois)"

  case "$channel_choice" in
    Telegram)
      info "Crie um bot no Telegram: abra @BotFather e use /newbot"
      _ask_or_env "$(echo -e "${CYAN}Cole seu TELEGRAM_BOT_TOKEN: ${RESET}")" tg_token TELEGRAM_BOT_TOKEN
      if [ -n "$tg_token" ]; then
        _env_set "TELEGRAM_BOT_TOKEN" "$tg_token"
        info "Obtenha seu Telegram user ID: envie uma mensagem para @userinfobot"
        _ask_or_env "$(echo -e "${CYAN}Cole seu Telegram user ID: ${RESET}")" tg_user_id TELEGRAM_USER_ID
        _env_set "TELEGRAM_USER_ID" "$tg_user_id"
        _env_set "PRIMARY_CHANNEL" "telegram"
        success "Telegram configurado."
      fi
      ;;
    "WhatsApp (via Evolution API)")
      info "Configure Evolution API: https://doc.evolution-api.com/v2/pt/get-started/introduction"
      _ask_or_env "$(echo -e "${CYAN}Cole a URL da sua Evolution API: ${RESET}")" wa_url WHATSAPP_API_URL
      _ask_or_env "$(echo -e "${CYAN}Cole sua Evolution API Key: ${RESET}")" wa_key WHATSAPP_API_KEY
      _ask_or_env "$(echo -e "${CYAN}Cole seu número WhatsApp (com DDI, ex: 5521999999999): ${RESET}")" wa_number WHATSAPP_NUMBER
      if [ -n "$wa_url" ] && [ -n "$wa_key" ] && [ -n "$wa_number" ]; then
        _env_set "WHATSAPP_API_URL" "$wa_url"
        _env_set "WHATSAPP_API_KEY" "$wa_key"
        _env_set "WHATSAPP_NUMBER" "$wa_number"
        _env_set "PRIMARY_CHANNEL" "whatsapp"
        success "WhatsApp configurado."
      fi
      ;;
    Discord)
      info "Crie um bot no Discord: https://discord.com/developers/applications"
      _ask_or_env "$(echo -e "${CYAN}Cole seu DISCORD_BOT_TOKEN: ${RESET}")" dc_token DISCORD_BOT_TOKEN
      _ask_or_env "$(echo -e "${CYAN}Cole seu Discord user ID: ${RESET}")" dc_user_id DISCORD_USER_ID
      if [ -n "$dc_token" ] && [ -n "$dc_user_id" ]; then
        _env_set "DISCORD_BOT_TOKEN" "$dc_token"
        _env_set "DISCORD_USER_ID" "$dc_user_id"
        _env_set "PRIMARY_CHANNEL" "discord"
        success "Discord configurado."
      fi
      ;;
    Slack)
      info "Configure Slack App: https://api.slack.com/apps"
      _ask_or_env "$(echo -e "${CYAN}Cole seu SLACK_BOT_TOKEN: ${RESET}")" slack_token SLACK_BOT_TOKEN
      _ask_or_env "$(echo -e "${CYAN}Cole seu Slack user ID: ${RESET}")" slack_user_id SLACK_USER_ID
      if [ -n "$slack_token" ] && [ -n "$slack_user_id" ]; then
        _env_set "SLACK_BOT_TOKEN" "$slack_token"
        _env_set "SLACK_USER_ID" "$slack_user_id"
        _env_set "PRIMARY_CHANNEL" "slack"
        success "Slack configurado."
      fi
      ;;
    "Pular (configurar depois)")
      warn "Nenhum canal configurado. Configure em .env depois."
      ;;
    *)
      warn "Opção inválida. Configure manualmente em .env depois."
      ;;
  esac
else
  success "Canal já configurado: $PRIMARY_CHANNEL"
fi

# ── 6. Detectar LLM Provider e Gerar Configuração ────────────────
step "Detectando LLM provider..."

ANTHROPIC_KEY=$(_env_get "ANTHROPIC_API_KEY")
OPENAI_KEY=$(_env_get "OPENAI_API_KEY")
GEMINI_KEY=$(_env_get "GEMINI_API_KEY")
GROQ_KEY=$(_env_get "GROQ_API_KEY")
OPENROUTER_KEY=$(_env_get "OPENROUTER_API_KEY")
OPENROUTER_MODEL=$(_env_get "OPENROUTER_MODEL")
PRIMARY_CHANNEL=$(_env_get "PRIMARY_CHANNEL")

if [ -n "$OPENROUTER_KEY" ]; then
  PROVIDER_ID="openrouter"
  PROVIDER_BASE_URL="https://openrouter.ai/api/v1"
  PROVIDER_API_KEY="$OPENROUTER_KEY"
  MODEL_ID="${OPENROUTER_MODEL:-openai/gpt-oss-20b:free}"
  MODEL_NAME="OpenRouter: ${MODEL_ID}"
  PROVIDER_HEADERS='"headers": { "HTTP-Referer": "https://github.com/samurai-py/bastion", "X-Title": "Bastion" },'
  success "Usando OpenRouter ($MODEL_ID)"
elif [ -n "$ANTHROPIC_KEY" ]; then
  PROVIDER_ID="anthropic"
  PROVIDER_BASE_URL="https://api.anthropic.com"
  PROVIDER_API_KEY="$ANTHROPIC_KEY"
  MODEL_ID="claude-sonnet-4-5"
  MODEL_NAME="Claude Sonnet 4.5"
  PROVIDER_HEADERS=""
  success "Usando Anthropic (Claude)"
elif [ -n "$OPENAI_KEY" ]; then
  PROVIDER_ID="openai"
  PROVIDER_BASE_URL="https://api.openai.com/v1"
  PROVIDER_API_KEY="$OPENAI_KEY"
  MODEL_ID="gpt-4o"
  MODEL_NAME="GPT-4o"
  PROVIDER_HEADERS=""
  success "Usando OpenAI (GPT-4o)"
elif [ -n "$GEMINI_KEY" ]; then
  PROVIDER_ID="google-gemini"
  PROVIDER_BASE_URL="https://generativelanguage.googleapis.com/v1beta/openai"
  PROVIDER_API_KEY="$GEMINI_KEY"
  MODEL_ID="gemini-2.0-flash"
  MODEL_NAME="Gemini 2.0 Flash"
  PROVIDER_HEADERS=""
  success "Usando Google Gemini"
elif [ -n "$GROQ_KEY" ]; then
  PROVIDER_ID="groq"
  PROVIDER_BASE_URL="https://api.groq.com/openai/v1"
  PROVIDER_API_KEY="$GROQ_KEY"
  MODEL_ID="llama-3.3-70b-versatile"
  MODEL_NAME="Llama 3.3 70B (Groq)"
  PROVIDER_HEADERS=""
  success "Usando Groq (Llama 3.3)"
else
  error "Nenhuma API key de LLM encontrada. Configure em .env e rode novamente."
  exit 1
fi

# ── 6.5. Configurar Composio ─────────────────────────────────────
step "Configurando Composio..."

EXISTING_COMPOSIO=$(_env_get "COMPOSIO_CONSUMER_KEY")

if [ -z "$EXISTING_COMPOSIO" ]; then
  info "O Bastion usa o Composio para integrar com 850+ apps (Gmail, Calendar, GitHub, etc.)"
  info "Crie sua chave gratuita em: https://dashboard.composio.dev"
  _ask_or_env "$(echo -e "${CYAN}Cole sua COMPOSIO_CONSUMER_KEY (começa com ck_): ${RESET}")" composio_key COMPOSIO_CONSUMER_KEY
  if [ -n "$composio_key" ]; then
    _env_set "COMPOSIO_CONSUMER_KEY" "$composio_key"
    success "Composio configurado."
  else
    warn "Composio não configurado. Algumas integrações não estarão disponíveis."
  fi
else
  success "Composio já configurado no .env."
fi

# ── 6.6. Instalar Plugins ─────────────────────────────────────────
step "Instalando plugins..."

install_plugin "@gendigital/sage-bastion" --force
SAGE_INSTALLED=true
install_plugin "@composio/bastion-plugin" --force composio
COMPOSIO_INSTALLED=true

# Configurar consumer key no Bastion se disponível
COMPOSIO_KEY=$(_env_get "COMPOSIO_CONSUMER_KEY")
if [ -n "$COMPOSIO_KEY" ]; then
  info "Composio consumer key configurada no .env."
fi

success "Scanner de segurança Sage ativo."

# ── 7. Gerar bastion.json com Configuração Robusta ───────────────
step "Gerando configuração Bastion..."

CONFIG_DIR="$INSTALL_DIR/config"
mkdir -p "$CONFIG_DIR"

# Ler configurações de canais do .env
TELEGRAM_BOT_TOKEN=$(_env_get "TELEGRAM_BOT_TOKEN")
TELEGRAM_USER_ID=$(_env_get "TELEGRAM_USER_ID")
DISCORD_BOT_TOKEN=$(_env_get "DISCORD_BOT_TOKEN")
DISCORD_USER_ID=$(_env_get "DISCORD_USER_ID")
SLACK_BOT_TOKEN=$(_env_get "SLACK_BOT_TOKEN")
SLACK_USER_ID=$(_env_get "SLACK_USER_ID")
WA_URL=$(_env_get "WHATSAPP_API_URL")
WA_KEY=$(_env_get "WHATSAPP_API_KEY")
WA_NUMBER=$(_env_get "WHATSAPP_NUMBER")

# Construir seção de channels dinamicamente
CHANNEL_ENTRIES=()

if [ -n "$TELEGRAM_BOT_TOKEN" ]; then
  TG_ENTRY="\"telegram\": { \"enabled\": true, \"botToken\": \"${TELEGRAM_BOT_TOKEN}\", \"dmPolicy\": \"allowlist\""
  [ -n "$TELEGRAM_USER_ID" ] && TG_ENTRY="${TG_ENTRY}, \"allowFrom\": [\"tg:${TELEGRAM_USER_ID}\"]"
  CHANNEL_ENTRIES+=("    ${TG_ENTRY} }")
fi

if [ -n "$DISCORD_BOT_TOKEN" ]; then
  DC_ENTRY="\"discord\": { \"enabled\": true, \"token\": \"${DISCORD_BOT_TOKEN}\", \"dmPolicy\": \"allowlist\""
  [ -n "$DISCORD_USER_ID" ] && DC_ENTRY="${DC_ENTRY}, \"allowFrom\": [\"${DISCORD_USER_ID}\"]"
  CHANNEL_ENTRIES+=("    ${DC_ENTRY} }")
fi

if [ -n "$SLACK_BOT_TOKEN" ]; then
  SL_ENTRY="\"slack\": { \"enabled\": true, \"botToken\": \"${SLACK_BOT_TOKEN}\", \"dmPolicy\": \"allowlist\""
  [ -n "$SLACK_USER_ID" ] && SL_ENTRY="${SL_ENTRY}, \"allowFrom\": [\"${SLACK_USER_ID}\"]"
  CHANNEL_ENTRIES+=("    ${SL_ENTRY} }")
fi

if [ -n "$WA_URL" ] && [ -n "$WA_KEY" ]; then
  WA_ENTRY="\"whatsapp\": { \"enabled\": true, \"apiUrl\": \"${WA_URL}\", \"apiKey\": \"${WA_KEY}\", \"dmPolicy\": \"allowlist\""
  [ -n "$WA_NUMBER" ] && WA_ENTRY="${WA_ENTRY}, \"allowFrom\": [\"+${WA_NUMBER}\"]"
  CHANNEL_ENTRIES+=("    ${WA_ENTRY} }")
fi

# Montar seção channels com vírgulas entre entradas
if [ ${#CHANNEL_ENTRIES[@]} -gt 0 ]; then
  CHANNELS_BODY=""
  for i in "${!CHANNEL_ENTRIES[@]}"; do
    [ "$i" -gt 0 ] && CHANNELS_BODY="${CHANNELS_BODY},"$'\n'
    CHANNELS_BODY="${CHANNELS_BODY}${CHANNEL_ENTRIES[$i]}"
  done
  CHANNELS_SECTION=",
  \"channels\": {
${CHANNELS_BODY}
  }"
else
  CHANNELS_SECTION=""
fi

# Gerar bastion.json com channels integrados
cat > "$CONFIG_DIR/bastion.json" <<EOF
{
  "agents": {
    "defaults": {
      "maxConcurrent": 4,
      "subagents": { "maxConcurrent": 8 },
      "compaction": { "mode": "safeguard" },
      "model": { "primary": "${PROVIDER_ID}/${MODEL_ID}" },
      "models": { "${PROVIDER_ID}/${MODEL_ID}": { "alias": "${PROVIDER_ID}" } }
    }
  },
  "gateway": {
    "mode": "local",
    "auth": { "mode": "none" }
  }${CHANNELS_SECTION},
  "models": {
    "mode": "merge",
    "providers": {
      "${PROVIDER_ID}": {
        "baseUrl": "${PROVIDER_BASE_URL}",
        "api": "openai-completions",
        "apiKey": "${PROVIDER_API_KEY}",
        ${PROVIDER_HEADERS}
        "models": [
          {
            "id": "${MODEL_ID}",
            "name": "${MODEL_NAME}",
            "contextWindow": 128000,
            "maxTokens": 8192,
            "input": ["text"],
            "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
            "reasoning": false
          }
        ]
      }
    }
  }
}
EOF

success "Bastion configurado com ${MODEL_NAME}"

# ── 7.5. Instalar Skills Core ─────────────────────────────────────
step "Instalando skills core..."

install_skill "mcporter"
install_skill "https://clawhub.ai/ivangdavila/stock-images"

# Install output-validator dependencies
if command -v pip3 &>/dev/null; then
  pip3 install jsonschema click pyyaml --quiet 2>/dev/null || true
fi

# ── 8. Preparar USER.md e diretórios necessários ────────────────
step "Preparando ambiente..."

# Detectar UID/GID do usuário atual e persistir no .env
# O container irá rodar como este usuário para evitar arquivos root
BASTION_UID=$(id -u)
BASTION_GID=$(id -g)
_env_set "BASTION_UID" "$BASTION_UID"
_env_set "BASTION_GID" "$BASTION_GID"

# Gerar APP_JWT_SECRET se ausente — necessário pro canal webhook/cockpit (WR-01:
# o daemon recusa subir o webhook sem isso). Idempotente: não sobrescreve valor existente.
if [ -z "$(_env_get APP_JWT_SECRET)" ]; then
  _env_set "APP_JWT_SECRET" "$(openssl rand -hex 32)"
  info "APP_JWT_SECRET gerado."
fi

# Bind address do webhook/cockpit dentro do container — só define se ausente.
if [ -z "$(_env_get BASTION_WEBHOOK_ADDR)" ]; then
  _env_set "BASTION_WEBHOOK_ADDR" "0.0.0.0:8080"
fi

# Detectar timezone do sistema
SYSTEM_TZ=$(timedatectl show --property=Timezone --value 2>/dev/null \
  || cat /etc/timezone 2>/dev/null \
  || echo "UTC")
_env_set "TIMEZONE" "$SYSTEM_TZ"
info "Timezone detectado: $SYSTEM_TZ (ajuste TIMEZONE no .env se necessário)"

# Criar diretórios antes do Docker — garante ownership do usuário atual (sem sudo)
mkdir -p "$INSTALL_DIR/config/workspace"
mkdir -p "$INSTALL_DIR/config/identity"
mkdir -p "$INSTALL_DIR/personas"

# Pré-autorizar o user_id no USER.md (repo root, bind-mounted no container)
PRIMARY_CHANNEL=$(_env_get "PRIMARY_CHANNEL")
USER_ID=""
case "$PRIMARY_CHANNEL" in
  telegram) USER_ID=$(_env_get "TELEGRAM_USER_ID") ;;
  whatsapp) USER_ID=$(_env_get "WHATSAPP_NUMBER") ;;
  discord)  USER_ID=$(_env_get "DISCORD_USER_ID") ;;
  slack)    USER_ID=$(_env_get "SLACK_USER_ID") ;;
esac

if [ -n "$USER_ID" ]; then
  cat > "$INSTALL_DIR/USER.md" <<EOF
---
name: ""
language: "pt-BR"
timezone: "${SYSTEM_TZ}"
authorized_user_ids:
  - "${USER_ID}"
personas: []
totp_configured: false
user_bio: ""
pain_points_and_goals: ""
onboarding_completed_at: ""
---

<!-- This file is auto-generated by bastion/onboarding skill. -->
<!-- authorized_user_ids is managed exclusively by the installer — never modified by the agent. -->
EOF
  success "User ID ${USER_ID} pré-autorizado no USER.md."
fi

success "Ambiente preparado."

# ── 9. Iniciar Bastion com Healthcheck ──────────────────────────
step "Iniciando Bastion..."

cd "$INSTALL_DIR"

# Diretórios já criados com ownership correto via mkdir-p acima — nenhum chown necessário

# Pull da imagem mais recente para evitar cache corrompido.
# --ignore-buildable: os 4 serviços bastion-* têm `build:` (sem imagem em registry
# nenhum) — sem essa flag, `pull` tenta baixá-los e falha, e o `set -e` aborta antes
# de chegar no `up` (que é quem builda). Só `busybox:stable` (volume-init) é pull de
# verdade; o resto é construído localmente pelo `up` logo abaixo.
docker compose pull --quiet --ignore-buildable

# Força recreação para aplicar novas configurações
docker compose up -d --force-recreate --remove-orphans

# Aguardar o container ficar saudável
info "Aguardando o Bastion inicializar..."
sleep 5

if docker ps --filter "name=bastion" --format "{{.Status}}" | grep -q "Up"; then
  success "Bastion está rodando!"
else
  warn "Container iniciado mas pode estar com problemas. Verifique os logs:"
  echo "  cd $INSTALL_DIR && docker compose logs -f"
fi

# ── 10. Verificação Final ────────────────────────────────────────
step "Verificando instalação..."

VALIDATION_FAILED=false

# Verificar se o .env tem as variáveis necessárias
if [ ! -f "$INSTALL_DIR/.env" ]; then
  error "Arquivo .env não encontrado"
  VALIDATION_FAILED=true
fi

# Verificar se tem pelo menos um LLM configurado
LLM_FOUND=false
for key in OPENROUTER_API_KEY ANTHROPIC_API_KEY OPENAI_API_KEY GEMINI_API_KEY GROQ_API_KEY; do
  val=$(_env_get "$key")
  if [ -n "$val" ]; then
    LLM_FOUND=true
    break
  fi
done

if [ "$LLM_FOUND" = false ]; then
  warn "Nenhum LLM configurado no .env"
  VALIDATION_FAILED=true
fi

# Verificar se tem pelo menos um canal configurado
CHANNEL_FOUND=false
for key in TELEGRAM_BOT_TOKEN DISCORD_BOT_TOKEN SLACK_BOT_TOKEN WHATSAPP_API_URL; do
  val=$(_env_get "$key")
  if [ -n "$val" ]; then
    CHANNEL_FOUND=true
    break
  fi
done

if [ "$CHANNEL_FOUND" = false ]; then
  warn "Nenhum canal configurado no .env"
  VALIDATION_FAILED=true
fi

# Verificar se o bastion.json foi criado
if [ ! -f "$CONFIG_DIR/bastion.json" ]; then
  error "Arquivo bastion.json não foi criado"
  VALIDATION_FAILED=true
else
  # Verificar se tem a seção channels no bastion.json
  if ! grep -q '"channels"' "$CONFIG_DIR/bastion.json"; then
    warn "Seção 'channels' não encontrada no bastion.json"
    VALIDATION_FAILED=true
  fi

  # Verificar se tem a seção models
  if ! grep -q '"models"' "$CONFIG_DIR/bastion.json"; then
    error "Seção 'models' não encontrada no bastion.json"
    VALIDATION_FAILED=true
  fi
fi

# Verificar arquivos essenciais no repo root (fonte de verdade, bind-mounted)
for f in SOUL.md USER.md AGENTS.md; do
  if [ ! -f "$INSTALL_DIR/$f" ]; then
    warn "Arquivo $f não encontrado"
    VALIDATION_FAILED=true
  fi
done

if [ ! -d "$INSTALL_DIR/skills" ]; then
  warn "Pasta skills não encontrada"
  VALIDATION_FAILED=true
fi

# Verificar se o container está rodando
if docker ps --filter "name=bastion-core" --format "{{.Status}}" | grep -q "Up"; then
  success "Container Bastion está rodando"

  # Verificar se os bind mounts estão corretos dentro do container
  if docker exec bastion-core test -f /home/node/.bastion/workspace/SOUL.md 2>/dev/null; then
    success "SOUL.md acessível no container"
  else
    warn "SOUL.md não encontrado dentro do container"
    VALIDATION_FAILED=true
  fi

  if docker exec bastion-core test -d /home/node/.bastion/workspace/skills 2>/dev/null; then
    success "Skills acessíveis no container"
  else
    warn "Pasta skills não encontrada dentro do container"
    VALIDATION_FAILED=true
  fi

  # Verificar se o Telegram conectou (se configurado)
  if [ -n "$(_env_get TELEGRAM_BOT_TOKEN)" ]; then
    sleep 3
    if docker compose -f "$INSTALL_DIR/docker-compose.yml" logs bastion-core 2>&1 | grep -q "starting provider"; then
      success "Telegram conectado com sucesso"
    else
      warn "Telegram pode não ter conectado. Verifique os logs."
    fi
  fi
else
  error "Container Bastion não está rodando"
  VALIDATION_FAILED=true
fi

if [ "$VALIDATION_FAILED" = true ]; then
  echo ""
  warn "Algumas verificações falharam. Revise a configuração."
  echo ""
fi

# ── 11. Resumo Final ──────────────────────────────────────────────
step "Instalação concluída!"

echo ""
echo -e "${BOLD}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RESET}"
echo -e "  ${GREEN}✓${RESET} Modelo:  ${BOLD}${MODEL_NAME}${RESET}"
[ -n "$PRIMARY_CHANNEL" ] && echo -e "  ${GREEN}✓${RESET} Canal:   ${BOLD}${PRIMARY_CHANNEL}${RESET}"
[ -n "$USER_ID" ] && echo -e "  ${GREEN}✓${RESET} User ID: ${BOLD}${USER_ID}${RESET}"
echo ""
echo -e "  ${CYAN}Próximos passos:${RESET}"
case "$PRIMARY_CHANNEL" in
  telegram) echo -e "    1. Abra o Telegram e envie ${BOLD}/start${RESET} para seu bot" ;;
  whatsapp) echo -e "    1. Envie uma mensagem para seu número WhatsApp" ;;
  discord) echo -e "    1. Envie uma DM para seu bot no Discord" ;;
  slack) echo -e "    1. Envie uma DM para seu bot no Slack" ;;
  *) echo -e "    1. Configure um canal em ${BOLD}.env${RESET} e rode novamente" ;;
esac
echo -e "    2. Complete o onboarding (nome, personas, TOTP)"
echo -e "    3. Comece a usar o Bastion!"
echo ""
if [ ${#CORE_SKILLS_FAILED[@]} -gt 0 ]; then
  warn "Skills core que falharam na instalação (instale manualmente com 'clawhub install'):"
  for s in "${CORE_SKILLS_FAILED[@]}"; do
    echo -e "    ${YELLOW}✗${RESET} ${s}"
  done
  echo ""
fi
echo -e "  ${CYAN}Comandos úteis:${RESET}"
echo -e "    Ver logs:      ${BOLD}cd $INSTALL_DIR && docker compose logs -f${RESET}"
echo -e "    Reiniciar:     ${BOLD}cd $INSTALL_DIR && docker compose restart${RESET}"
echo -e "    Parar:         ${BOLD}cd $INSTALL_DIR && docker compose down${RESET}"
echo -e "    Reconfigurar:  ${BOLD}bash $INSTALL_DIR/installer.sh${RESET}"
echo -e "${BOLD}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RESET}"
echo ""
