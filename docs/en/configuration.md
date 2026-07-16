# Configuration

Bastion separates non-secret configuration from credentials. Commit reviewable behavior in `bastion.toml`; inject tokens and secrets through `.env` or your deployment secret manager.

## Configuration precedence

The binary loads configuration in this order:

1. `bastion.toml` (or the path in `BASTION_CONFIG`)
2. Environment variables with the `BASTION__` prefix and `__` as the nesting separator

For example:

```bash
BASTION__AGENT__DEFAULT_MODEL=your-model-name cargo run -- daemon
BASTION__SESSION__DB_PATH=/data/sessions.db cargo run -- daemon
```

## Core settings

| Area | Setting | Purpose |
| --- | --- | --- |
| Agent | `agent.default_model` | Provider model name used by the runtime. |
| Agent | `agent.daily_budget_usd` | Daily budget configured for the agent. |
| Session | `session.db_path` | SQLite session database location. |
| Session | `session.autocompact_threshold` | Session compaction threshold. |
| Logging | `logging.log_path` | JSON log file location. |
| MCP | `mcp.tool_call_timeout_secs` | Tool-call timeout. |

The checked-in file is a useful starting point, but its values are deployment defaults—not universal advice. Review every enabled service before using it outside local development.

## Secrets and runtime variables

These names are read directly by the product paths shown below. They belong in `.env`, never in a committed TOML file.

| Variable | Used for |
| --- | --- |
| `TELEGRAM_BOT_TOKEN` | Telegram channel. |
| `BASTION_WEBHOOK_ADDR` | Bind address for the webhook/mobile router. |
| `APP_JWT_SECRET` | Webhook and mobile pairing JWT signing; required when that surface is enabled. |
| `WHATSAPP_PHONE_NUMBER_ID`, `WHATSAPP_ACCESS_TOKEN`, `WHATSAPP_APP_SECRET`, `WHATSAPP_VERIFY_TOKEN` | WhatsApp Cloud API channel. |
| `DISCORD_BOT_TOKEN` | Discord channel. |
| `SLACK_BOT_TOKEN`, `SLACK_APP_TOKEN` | Slack Socket Mode channel. |
| `BASTION_OTEL_STDOUT` | Enables stdout OpenTelemetry exporting when set to `true`. |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | Enables OTLP/gRPC trace export. |

Provider-specific credentials are resolved by the provider configuration supplied by the `bastion-core` dependencies. Keep those credentials outside Git as well.

## Identities and channels

The `[[identity]]` table maps one canonical `owner_id` to channel-specific identifiers. An unmapped sender is rejected by the channel adapters.

```toml
[[identity]]
owner_id = "mario"
telegram_chat_id = "12345678"
discord_user_id = "111222333444555"
slack_user_id = "U01ABCDEF"
email_address = "mario@example.com"
```

Channels are disabled by default in the supplied configuration except for the configured Telegram example. Enable each channel deliberately under `[channels]`; see [Channels](channels.md) for its credential and exposure requirements.

## Compose-specific settings

The Compose deployment passes `BASTION_CONFIG=/bastion.toml`, exposes the core at `8080`, and requires `APP_JWT_SECRET` for the webhook/mobile router. It also uses `BASTION_UID` and `BASTION_GID` to align volume ownership with the host user. Review `docker-compose.yml` before changing network or volume boundaries.

## Safe configuration checklist

- Keep `.env` untracked and rotate a value if it is ever committed.
- Map only the people permitted to use each channel.
- Start with one channel and confirm its logs before enabling another.
- Treat public Discord/Slack messages and inbound email as untrusted content.
- Enable telemetry content events only after assessing the privacy impact.
