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

## Portable state (`BASTION_DATA_DIR`)

Bastion's state is not one file — it's four: the session SQLite database
(`session.db_path`, which also holds tasks, credentials, config overrides,
webhook delivery, goals, and belief/memory), the persona directory
(`PersonaRegistry` loads from `.` by default), the companion save file
(`~/.config/bastion/companion.json`), and secrets (`BASTION_SECRETS_DIR`).
Each has its own path convention, and the persona directory in particular is
resolved relative to the process's current working directory — not a fixed
location — which makes it easy to lose track of what to back up or mount when
moving an instance to a new host or container.

Set `BASTION_DATA_DIR` to a single directory and Bastion fills in a default
for each of the four under that root, **unless the more specific variable is
already set** (an explicit `BASTION_SECRETS_DIR`, for example, always wins
over the `BASTION_DATA_DIR` default):

| Under `BASTION_DATA_DIR` | Defaults... | ...unless this is already set |
| --- | --- | --- |
| `bastion.db` | `session.db_path` | `BASTION__SESSION__DB_PATH` |
| `bastion.log` | `logging.log_path` | `BASTION__LOGGING__LOG_PATH` |
| `personas/` | persona load directory | `BASTION_PERSONAS_DIR` |
| `companion.json` | companion save file | `BASTION_COMPANION_PATH` |
| `secrets/` | secret resolver directory | `BASTION_SECRETS_DIR` |

```bash
docker run -v bastion-data:/data -e BASTION_DATA_DIR=/data thewaifucorp/bastion daemon
```

A container started this way can be destroyed and recreated against the same
volume with every subsystem — sessions, tasks, memory/beliefs, personas,
companion, secrets — intact, without hand-editing any config. Omitting
`BASTION_DATA_DIR` leaves every path exactly as it is today (`.bastion/`
relative paths, `.` for personas, `~/.config/bastion/`) — fully backward
compatible.

## Provider and model selection

In the local TUI, use `/connect` to see secure setup steps for a provider and
`/models` to open the searchable picker of recommended models. The selection is
saved beside the daemon session database and is restored on the next start.
`/model` shows the active choice; `/model reset` removes the preference and
returns to `agent.default_model` in `bastion.toml`.

Keys remain outside chat and TOML: configure them in `.env` or the deployment
secret store before selecting that provider.

## Core settings

| Area | Setting | Purpose |
| --- | --- | --- |
| Agent | `agent.default_model` | Provider model name used by the runtime. |
| Agent | `agent.daily_budget_usd` | Daily budget configured for the agent. |
| Session | `session.db_path` | SQLite session database location. |
| Session | `session.autocompact_threshold` | Session compaction threshold. |
| Logging | `logging.log_path` | JSON log file location. |
| TUI | `tui.theme`, `tui.accent` | RGB preset or custom terminal accent. |
| TUI | `tui.mascot`, `tui.animations`, `tui.game`, `tui.pet` | Companion display, progression, and optional pet pack. |
| MCP | `mcp.tool_call_timeout_secs` | Tool-call timeout. |

The checked-in file is a useful starting point, but its values are deployment defaults—not universal advice. Review every enabled service before using it outside local development.

## Secrets and runtime variables

These names are read directly by the product paths shown below. They belong in `.env`, never in a committed TOML file.

| Variable | Used for |
| --- | --- |
| `BASTION_DATA_DIR` | Single root that defaults `session.db_path`, `logging.log_path`, `BASTION_PERSONAS_DIR`, `BASTION_COMPANION_PATH`, and `BASTION_SECRETS_DIR` — see [Portable state](#portable-state-bastion_data_dir) above. |
| `BASTION_SECRETS_DIR` | Directory the file-based `SecretResolver` reads from (mounted-secrets deployments). |
| `BASTION_PERSONAS_DIR` | Directory `PersonaRegistry` loads `SOUL.md` files from; defaults to `.`. |
| `BASTION_COMPANION_PATH` | Companion save-file path; defaults to `~/.config/bastion/companion.json`. |
| `TELEGRAM_BOT_TOKEN` | Telegram channel. |
| `BASTION_PUBLISH_HOST`, `BASTION_HTTP_PORT` | Host interface and port published by Compose; defaults to `127.0.0.1:8080`. |
| `BASTION_WEBHOOK_ADDR` | Container-internal bind address for the webhook/mobile router. |
| `APP_JWT_SECRET` | Webhook and mobile pairing JWT signing; required when that surface is enabled. |
| `BASTION_BOOTSTRAP_TOKEN` | Initial owner-scoped API/TUI access; generated by the installer and intended for rotation after onboarding. |
| `BASTION_INFER_TOKEN` | Authenticates private sidecar calls to the inference gateway. |
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

The local webhook is enabled by default; external messaging channels are disabled. A channel starts only when `enabled = true` and its required environment credentials exist. See [Channels](channels.md).

## Compose-specific settings

The Compose deployment keeps using the same `bastion.toml`, overriding local paths and MCP URLs through `BASTION__...` variables. It exposes the core at `8080` and uses `BASTION_UID`/`BASTION_GID` for volume ownership.

## Safe configuration checklist

- Keep `.env` untracked and rotate a value if it is ever committed.
- Map only the people permitted to use each channel.
- Start with one channel and confirm its logs before enabling another.
- Treat public Discord/Slack messages and inbound email as untrusted content.
- Enable telemetry content events only after assessing the privacy impact.
