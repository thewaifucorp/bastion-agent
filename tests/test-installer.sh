#!/usr/bin/env bash
# Test suite para o instalador do Bastion
# Valida diferentes cenários de instalação

set -uo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
RESET='\033[0m'

TESTS_PASSED=0
TESTS_FAILED=0

test_pass() {
  echo -e "${GREEN}✓${RESET} $1"
  ((TESTS_PASSED++)) || true
}

test_fail() {
  echo -e "${RED}✗${RESET} $1"
  ((TESTS_FAILED++)) || true
}

test_info() {
  echo -e "${CYAN}→${RESET} $1"
}

# ── Test 1: Syntax Check ──────────────────────────────────────────
test_info "Test 1: Verificando sintaxe do bash..."
if bash -n ./installer.sh 2>/dev/null; then
  test_pass "Sintaxe do bash válida"
else
  test_fail "Erro de sintaxe no installer.sh"
fi

# ── Test 2: Required Functions ────────────────────────────────────
test_info "Test 2: Verificando funções obrigatórias..."
required_functions=(
  "banner"
  "_env_get"
  "_env_set"
  "_ask"
  "_ask_or_env"
  "_select_or_env"
  "check_docker"
  "check_docker_compose"
)

for func in "${required_functions[@]}"; do
  if grep -q "^${func}()" ./installer.sh; then
    test_pass "Função $func encontrada"
  else
    test_fail "Função $func não encontrada"
  fi
done

# ── Test 3: Environment Variables ─────────────────────────────────
test_info "Test 3: Verificando variáveis de ambiente suportadas..."
required_vars=(
  "BASTION_WIZARD"
  "BASTION_DIR"
  "LLM_PROVIDER"
  "PRIMARY_CHANNEL"
  "OPENROUTER_API_KEY"
  "ANTHROPIC_API_KEY"
  "OPENAI_API_KEY"
  "GEMINI_API_KEY"
  "GROQ_API_KEY"
  "TELEGRAM_BOT_TOKEN"
  "TELEGRAM_USER_ID"
)

for var in "${required_vars[@]}"; do
  if grep -q "$var" ./installer.sh; then
    test_pass "Variável $var suportada"
  else
    test_fail "Variável $var não encontrada"
  fi
done

# ── Test 4: Configuration Files ───────────────────────────────────
test_info "Test 4: Verificando geração de arquivos de configuração..."
config_files=(
  "openclaw.json"
  "telegram.json"
  "discord.json"
  "slack.json"
  "whatsapp.json"
)

for file in "${config_files[@]}"; do
  if grep -q "$file" ./installer.sh; then
    test_pass "Geração de $file implementada"
  else
    test_fail "Geração de $file não encontrada"
  fi
done

# ── Test 5: Security Checks ───────────────────────────────────────
test_info "Test 5: Verificando implementação de segurança..."

if grep -q "dmPolicy.*allowlist" ./installer.sh; then
  test_pass "dmPolicy allowlist implementado"
else
  test_fail "dmPolicy allowlist não encontrado"
fi

if grep -q "authorized_user_ids" ./installer.sh; then
  test_pass "Pré-autorização de user_id implementada"
else
  test_fail "Pré-autorização de user_id não encontrada"
fi

if grep -q "imutável pelo agente" ./installer.sh; then
  test_pass "Comentário de segurança no USER.md presente"
else
  test_fail "Comentário de segurança no USER.md ausente"
fi

# ── Test 6: Docker Integration ────────────────────────────────────
test_info "Test 6: Verificando integração com Docker..."

if grep -q "docker compose pull" ./installer.sh; then
  test_pass "Pull de imagem Docker implementado"
else
  test_fail "Pull de imagem Docker não encontrado"
fi

if grep -q "docker compose up.*force-recreate" ./installer.sh; then
  test_pass "Recreação forçada de containers implementada"
else
  test_fail "Recreação forçada de containers não encontrada"
fi

if grep -q "chown -R 1000:1000" ./installer.sh; then
  test_pass "Correção de permissões implementada"
else
  test_fail "Correção de permissões não encontrada"
fi

# ── Test 7: Workspace Sync ────────────────────────────────────────
test_info "Test 7: Verificando sincronização de workspace..."

required_files=(
  "SOUL.md"
  "USER.md"
  "AGENTS.md"
  "HEARTBEAT.md"
)

for file in "${required_files[@]}"; do
  if grep -q "$file" ./installer.sh; then
    test_pass "Sincronização de $file implementada"
  else
    test_fail "Sincronização de $file não encontrada"
  fi
done

# ── Test 8: Error Handling ────────────────────────────────────────
test_info "Test 8: Verificando tratamento de erros..."

if grep -q "set -euo pipefail" ./installer.sh; then
  test_pass "Modo strict do bash habilitado"
else
  test_fail "Modo strict do bash não encontrado"
fi

if grep -q "error()" ./installer.sh; then
  test_pass "Função de erro implementada"
else
  test_fail "Função de erro não encontrada"
fi

# ── Test 9: LLM Provider Detection ────────────────────────────────
test_info "Test 9: Verificando detecção de LLM providers..."

providers=(
  "openrouter"
  "anthropic"
  "openai"
  "google-gemini"
  "groq"
)

for provider in "${providers[@]}"; do
  if grep -q "PROVIDER_ID=\"$provider\"" ./installer.sh; then
    test_pass "Provider $provider suportado"
  else
    test_fail "Provider $provider não encontrado"
  fi
done

# ── Test 10: Channel Configuration ────────────────────────────────
test_info "Test 10: Verificando configuração de canais..."

channels=(
  "telegram"
  "whatsapp"
  "discord"
  "slack"
)

for channel in "${channels[@]}"; do
  if grep -q "PRIMARY_CHANNEL.*$channel" ./installer.sh; then
    test_pass "Canal $channel suportado"
  else
    test_fail "Canal $channel não encontrado"
  fi
done

# ── Summary ───────────────────────────────────────────────────────
echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo -e "  ${GREEN}Passed:${RESET} $TESTS_PASSED"
echo -e "  ${RED}Failed:${RESET} $TESTS_FAILED"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

if [ $TESTS_FAILED -eq 0 ]; then
  echo -e "${GREEN}✓ Todos os testes passaram!${RESET}"
  exit 0
else
  echo -e "${RED}✗ Alguns testes falharam.${RESET}"
  exit 1
fi
