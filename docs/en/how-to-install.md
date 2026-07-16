# Bastion Installer Guide

## Overview

The Bastion installer is provider-agnostic and supports three configuration modes:

1. **Interactive Wizard** (default) — question and answer in the terminal
2. **Environment Variables** — for automation and CI/CD
3. **Existing .env File** — preserves previous settings

## Basic Usage

### Interactive Installation (Recommended)

```bash
bash <(curl -fsSL https://bastion.run/install)
```

The installer will guide you through:
- Docker verification
- LLM provider selection (OpenRouter, Anthropic, OpenAI, Gemini, Groq)
- Model selection (when applicable)
- Channel configuration (Telegram, WhatsApp, Discord, Slack)
- Automatic configuration generation

### Automated Installation (CI/CD)

```bash
export BASTION_WIZARD=false
export OPENROUTER_API_KEY="sk-or-..."
export OPENROUTER_MODEL="openai/gpt-oss-20b:free"
export PRIMARY_CHANNEL="Telegram"
export TELEGRAM_BOT_TOKEN="123456:ABC..."
export TELEGRAM_USER_ID="987654321"

bash installer.sh
```

## Supported Environment Variables

### LLM Providers

| Variable | Description | Example |
|----------|-------------|---------|
| `OPENROUTER_API_KEY` | OpenRouter API key | `sk-or-v1-...` |
| `OPENROUTER_MODEL` | Specific model | `openai/gpt-oss-20b:free` |
| `ANTHROPIC_API_KEY` | Anthropic API key | `sk-ant-...` |
| `OPENAI_API_KEY` | OpenAI API key | `sk-...` |
| `GEMINI_API_KEY` | Google API key | `AIza...` |
| `GROQ_API_KEY` | Groq API key | `gsk_...` |

### Messaging Channels

| Variable | Description | Example |
|----------|-------------|---------|
| `PRIMARY_CHANNEL` | Selected channel | `Telegram`, `Discord`, `Slack` |
| `TELEGRAM_BOT_TOKEN` | Telegram bot token | `123456:ABC-DEF...` |
| `TELEGRAM_USER_ID` | Your numeric user ID | `987654321` |
| `DISCORD_BOT_TOKEN` | Discord bot token | `MTk4...` |
| `DISCORD_USER_ID` | Your Discord user ID | `123456789012345678` |
| `SLACK_BOT_TOKEN` | Slack bot token | `xoxb-...` |
| `SLACK_USER_ID` | Your Slack user ID | `U01234567` |
| `WHATSAPP_API_URL` | Evolution API URL | `https://api.example.com` |
| `WHATSAPP_API_KEY` | Evolution API key | `abc123...` |
| `WHATSAPP_NUMBER` | Your number with country code | `15551234567` |

### Installer Control

| Variable | Description | Default |
|----------|-------------|---------|
| `BASTION_WIZARD` | Enable wizard mode | `true` |
| `BASTION_DIR` | Installation directory | `$HOME/bastion` |

## Installer Architecture

### Execution Flow

1. **Banner and Dependency Check**
   - Verifies Docker and Docker Compose
   - Offers automatic installation if missing

2. **Directory Preparation**
   - Clones or updates the repository
   - Preserves existing configurations

3. **.env Configuration**
   - Copies `.env.example` if it doesn't exist
   - Preserves existing values

4. **LLM Configuration**
   - Interactive wizard or environment variables
   - Supports multiple providers
   - Allows model selection (OpenRouter)

5. **Channel Configuration**
   - Interactive wizard or environment variables
   - Supports multiple channels
   - Automatically configures allowlist

6. **Configuration Generation**
   - `config/openclaw.json` — core configuration
   - `config/channels/*.json` — channel configuration
   - Applies fixes for known OpenClaw issues

7. **Workspace Preparation**
   - Syncs SOUL.md, USER.md, AGENTS.md
   - Copies skills to workspace
   - Pre-authorizes user_id in USER.md

8. **Startup**
   - Docker image pull
   - Forces container recreation
   - Health status check

## Applied Fixes

The installer automatically applies fixes for known issues:

### 1. Telegram Authentication Issue

**Symptom:** Bot doesn't respond, pairing code loop

**Fix:** Uses `dmPolicy: "allowlist"` with pre-configured user_id, avoiding the manual pairing flow that fails

### 2. Gateway Configuration

**Symptom:** Container doesn't accept connections

**Fix:** Forces `host: "0.0.0.0"` and `port: 18789` in openclaw.json

### 3. File Permissions

**Symptom:** Permission errors in container

**Fix:** Adjusts ownership to `1000:1000` (default container user)

### 4. Context Synchronization

**Symptom:** Bastion doesn't load SOUL.md or skills

**Fix:** Copies files to `config/workspace/` where OpenClaw reads them

## Security Guardrails

The installer respects the guardrails defined in `AGENTS.md`:

1. **Immutable Allowlist**
   - The `authorized_user_ids` field in USER.md is managed only by the installer
   - The agent can never modify this field
   - Explicit comment in the generated file

2. **Secure Pre-authorization**
   - User ID is added during installation
   - Avoids the need for manual initial authentication
   - Maintains security via TOTP after onboarding

## Troubleshooting

### Container won't start

```bash
cd ~/bastion
docker compose logs -f
```

Check that:
- API keys are correct in `.env`
- Docker has sufficient resources (memory, CPU)
- No port conflicts (18789, 443, 80)

### Bot not responding

1. Check if the user_id is correct:
   ```bash
   grep authorized_user_ids ~/bastion/USER.md
   ```

2. Check channel logs:
   ```bash
   docker compose logs openclaw | grep -i telegram
   ```

3. Test the bot connection:
   - Telegram: send `/start`
   - Others: send any message

### Reset from Scratch

```bash
cd ~/bastion
docker compose down -v
rm -rf config/
bash installer.sh
```

## Usage Examples

### Installation with OpenRouter (Free)

```bash
export BASTION_WIZARD=false
export OPENROUTER_API_KEY="sk-or-v1-..."
export OPENROUTER_MODEL="openai/gpt-oss-20b:free"
export TELEGRAM_BOT_TOKEN="123456:ABC..."
export TELEGRAM_USER_ID="987654321"
bash <(curl -fsSL https://bastion.run/install)
```

### Installation with Claude (Paid)

```bash
export BASTION_WIZARD=false
export ANTHROPIC_API_KEY="sk-ant-..."
export TELEGRAM_BOT_TOKEN="123456:ABC..."
export TELEGRAM_USER_ID="987654321"
bash <(curl -fsSL https://bastion.run/install)
```

### Installation with WhatsApp

```bash
export BASTION_WIZARD=false
export GROQ_API_KEY="gsk_..."
export WHATSAPP_API_URL="https://evolution.example.com"
export WHATSAPP_API_KEY="abc123..."
export WHATSAPP_NUMBER="15551234567"
bash <(curl -fsSL https://bastion.run/install)
```

## Next Steps

After installation:

1. Send `/start` to your bot
2. Complete onboarding (name, personas, TOTP)
3. Start using Bastion!

For more information:
- [Getting Started](getting-started.md)
- [Security Guide](security.md)
- [Personas](personas.md)
