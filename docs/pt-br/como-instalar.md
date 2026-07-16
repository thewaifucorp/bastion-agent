# Guia do Instalador Bastion

## Visão Geral

O instalador do Bastion é agnóstico e suporta três modos de configuração:

1. **Wizard Interativo** (padrão) — perguntas e respostas no terminal
2. **Variáveis de Ambiente** — para automação e CI/CD
3. **Arquivo .env Existente** — preserva configurações anteriores

## Uso Básico

### Instalação Interativa (Recomendado)

```bash
bash <(curl -fsSL https://bastion.run/install)
```

O instalador vai guiar você por:
- Verificação de Docker
- Escolha do LLM provider (OpenRouter, Anthropic, OpenAI, Gemini, Groq)
- Escolha do modelo (quando aplicável)
- Configuração do canal (Telegram, WhatsApp, Discord, Slack)
- Geração automática de configurações

### Instalação Automatizada (CI/CD)

```bash
export BASTION_WIZARD=false
export LLM_PROVIDER="OpenRouter (recomendado — modelos gratuitos)"
export OPENROUTER_API_KEY="sk-or-..."
export OPENROUTER_MODEL="openai/gpt-oss-20b:free"
export PRIMARY_CHANNEL="Telegram"
export TELEGRAM_BOT_TOKEN="123456:ABC..."
export TELEGRAM_USER_ID="987654321"

bash installer.sh
```

## Variáveis de Ambiente Suportadas

### LLM Providers

| Variável | Descrição | Exemplo |
|----------|-----------|---------|
| `LLM_PROVIDER` | Provider escolhido | `"OpenRouter (recomendado — modelos gratuitos)"` |
| `OPENROUTER_API_KEY` | Chave da OpenRouter | `sk-or-v1-...` |
| `OPENROUTER_MODEL` | Modelo específico | `openai/gpt-oss-20b:free` |
| `ANTHROPIC_API_KEY` | Chave da Anthropic | `sk-ant-...` |
| `OPENAI_API_KEY` | Chave da OpenAI | `sk-...` |
| `GEMINI_API_KEY` | Chave do Google | `AIza...` |
| `GROQ_API_KEY` | Chave da Groq | `gsk_...` |

### Canais de Mensagens

| Variável | Descrição | Exemplo |
|----------|-----------|---------|
| `PRIMARY_CHANNEL` | Canal escolhido | `Telegram`, `Discord`, `Slack` |
| `TELEGRAM_BOT_TOKEN` | Token do bot Telegram | `123456:ABC-DEF...` |
| `TELEGRAM_USER_ID` | Seu user ID numérico | `987654321` |
| `DISCORD_BOT_TOKEN` | Token do bot Discord | `MTk4...` |
| `DISCORD_USER_ID` | Seu user ID Discord | `123456789012345678` |
| `SLACK_BOT_TOKEN` | Token do bot Slack | `xoxb-...` |
| `SLACK_USER_ID` | Seu user ID Slack | `U01234567` |
| `WHATSAPP_API_URL` | URL da Evolution API | `https://api.example.com` |
| `WHATSAPP_API_KEY` | Chave da Evolution API | `abc123...` |
| `WHATSAPP_NUMBER` | Seu número com DDI | `5521999999999` |

### Controle do Instalador

| Variável | Descrição | Padrão |
|----------|-----------|--------|
| `BASTION_WIZARD` | Habilita modo wizard | `true` |
| `BASTION_DIR` | Diretório de instalação | `$HOME/bastion` |

## Arquitetura do Instalador

### Fluxo de Execução

1. **Banner e Verificação de Dependências**
   - Verifica Docker e Docker Compose
   - Oferece instalação automática se ausente

2. **Preparação de Diretórios**
   - Clona ou atualiza o repositório
   - Preserva configurações existentes

3. **Configuração de .env**
   - Copia `.env.example` se não existir
   - Preserva valores existentes

4. **Configuração de LLM**
   - Wizard interativo ou variáveis de ambiente
   - Suporta múltiplos providers
   - Permite escolha de modelo (OpenRouter)

5. **Configuração de Canal**
   - Wizard interativo ou variáveis de ambiente
   - Suporta múltiplos canais
   - Configura allowlist automaticamente

6. **Geração de Configurações**
   - `config/openclaw.json` — configuração do core
   - `config/channels/*.json` — configuração de canais
   - Aplica fixes para problemas conhecidos do OpenClaw

7. **Preparação do Workspace**
   - Sincroniza SOUL.md, USER.md, AGENTS.md
   - Copia skills para o workspace
   - Pré-autoriza user_id no USER.md

8. **Inicialização**
   - Pull da imagem Docker
   - Força recreação de containers
   - Verifica status de saúde

## Correções Aplicadas

O instalador aplica automaticamente correções para problemas conhecidos:

### 1. Problema de Autenticação do Telegram

**Sintoma:** Bot não responde, loop de pairing code

**Correção:** Usa `dmPolicy: "allowlist"` com user_id pré-configurado, evitando o fluxo de pairing manual que falha

### 2. Configuração de Gateway

**Sintoma:** Container não aceita conexões

**Correção:** Força `host: "0.0.0.0"` e `port: 18789` no openclaw.json

### 3. Permissões de Arquivos

**Sintoma:** Erros de permissão no container

**Correção:** Ajusta ownership para `1000:1000` (usuário padrão do container)

### 4. Sincronização de Contexto

**Sintoma:** Bastion não carrega SOUL.md ou skills

**Correção:** Copia arquivos para `config/workspace/` onde o OpenClaw os lê

## Guardrails de Segurança

O instalador respeita os guardrails definidos em `AGENTS.md`:

1. **Whitelist Imutável**
   - O campo `authorized_user_ids` em USER.md é gerenciado apenas pelo installer
   - O agente nunca pode modificar este campo
   - Comentário explícito no arquivo gerado

2. **Pré-autorização Segura**
   - User ID é adicionado durante a instalação
   - Evita necessidade de autenticação manual inicial
   - Mantém segurança via TOTP após onboarding

## Troubleshooting

### Container não inicia

```bash
cd ~/bastion
docker compose logs -f
```

Verifique se:
- As chaves de API estão corretas no `.env`
- O Docker tem recursos suficientes (memória, CPU)
- Não há conflito de portas (18789, 443, 80)

### Bot não responde

1. Verifique se o user_id está correto:
   ```bash
   grep authorized_user_ids ~/bastion/USER.md
   ```

2. Verifique os logs do canal:
   ```bash
   docker compose logs openclaw | grep -i telegram
   ```

3. Teste a conexão do bot:
   - Telegram: envie `/start`
   - Outros: envie qualquer mensagem

### Reconfigurar do Zero

```bash
cd ~/bastion
docker compose down -v
rm -rf config/
bash installer.sh
```

## Exemplos de Uso

### Instalação com OpenRouter (Gratuito)

```bash
export BASTION_WIZARD=false
export OPENROUTER_API_KEY="sk-or-v1-..."
export OPENROUTER_MODEL="openai/gpt-oss-20b:free"
export TELEGRAM_BOT_TOKEN="123456:ABC..."
export TELEGRAM_USER_ID="987654321"
bash <(curl -fsSL https://bastion.run/install)
```

### Instalação com Claude (Pago)

```bash
export BASTION_WIZARD=false
export ANTHROPIC_API_KEY="sk-ant-..."
export TELEGRAM_BOT_TOKEN="123456:ABC..."
export TELEGRAM_USER_ID="987654321"
bash <(curl -fsSL https://bastion.run/install)
```

### Instalação com WhatsApp

```bash
export BASTION_WIZARD=false
export GROQ_API_KEY="gsk_..."
export WHATSAPP_API_URL="https://evolution.example.com"
export WHATSAPP_API_KEY="abc123..."
export WHATSAPP_NUMBER="5521999999999"
bash <(curl -fsSL https://bastion.run/install)
```

## Próximos Passos

Após a instalação:

1. Envie `/start` para seu bot
2. Complete o onboarding (nome, personas, TOTP)
3. Comece a usar o Bastion!

Para mais informações:
- [Getting Started](getting-started.md)
- [Guia de Segurança](security.md)
- [Personas](personas.md)
