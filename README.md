# bastion-agent

[![ci](https://github.com/thewaifucorp/bastion-agent/actions/workflows/ci.yml/badge.svg)](https://github.com/thewaifucorp/bastion-agent/actions/workflows/ci.yml)
[![license](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![rust](https://img.shields.io/badge/rust-2021_edition-orange.svg)](Cargo.toml)

The **personal AI-agent product** built on [bastion-core](https://github.com/thewaifucorp/bastion-core) — a self-hosted, longitudinal, contestable, authority-safe agent runtime.

`bastion-agent` is the flagship consumer of the `bastion-core` substrate: it composes the runtime, cognition, personas, memory and provider crates into a running daemon with channels, an extension host, a companion app, and an installer. The stable substrate lives in [bastion-core](https://github.com/thewaifucorp/bastion-core) and is consumed here as versioned dependencies.

## What it is

A daemon (tokio) running the agent tool-loop, serving channels (Telegram / webhook / HTTP via axum, plus optional Discord / Slack / Email / local voice), connecting MCP servers, calling LLM providers or external agent runtimes (Codex / Claude Code / OpenCode via the `AgentRuntime` contract), and persisting sessions in SQLite.

## Differentiators

- **Longitudinal** — accompanies you across years, not just sessions.
- **Contestable memory** — you can inspect, correct, and revoke what it learned; memory carries source and validity.
- **Authority-safe** — external content never gains authority just by entering the context; egress is gated by privacy tier; approvals are typed and non-bypassable.
- **Subscription-backed** — works with a supported subscription (Codex/ChatGPT, Claude, OpenCode) with no API key required; credentials stay by reference.
- **Extensible** — a signed-manifest extension host (declarative / WASM / subprocess) where installing an extension never grants authority.

## Docs

- [Getting Started](docs/en/getting-started.md) · [Começando](docs/pt-br/iniciando.md)
- [Installer Guide](docs/en/how-to-install.md) · [VPS Setup](docs/en/vps-setup.md)
- [Channels Setup](docs/channels-setup.md) (WhatsApp, Discord, Slack, Email, voice)
- [Mobile App](docs/en/mobile-app.md) · [Personas](docs/en/personas.md) · [Security](docs/en/security.md)
- Full index: [docs/en/README.md](docs/en/README.md) · [docs/pt-br/README.md](docs/pt-br/README.md)

## Layout

- `src/` — daemon, channels, api, config, extension host, MCP composition, the `bastion` binary
- `skills/` — MCP-server skills (memory, skill-writer, self-improving, …)
- `mobile/` — Flutter companion app
- `installer.sh`, `Dockerfile`, `docker-compose*.yml` — install & deploy

## Build

Depends on `bastion-core` via git-pinned crates during incubation (see `Cargo.toml`); moves to crates.io versions once the substrate publishes.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for the development setup, required
checks, and code standards. [CHANGELOG.md](CHANGELOG.md) tracks notable
changes.

## License

MIT — see [LICENSE](LICENSE). Fork it, self-host it, resell your own hosted
instances — no restriction beyond the license text.
