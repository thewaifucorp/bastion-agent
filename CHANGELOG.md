# Changelog

All notable changes to `bastion-agent` are documented here. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versioning is a
product/release number (see
[bastion-core's VERSIONING.md](https://github.com/thewaifucorp/bastion-core/blob/main/docs/VERSIONING.md)
for how that differs from the library crates it depends on).

## [0.1.0] — 2026-07-15

### Added

Initial release — `bastion-agent` extracted as a standalone repository from
the original `bastion` monorepo, carrying the full development history of
the product:

- The `bastion` daemon (tokio): agent tool-loop, channels (Telegram /
  webhook / HTTP via axum, optional Discord / Slack / Email / local voice),
  MCP server composition, extension host, SQLite session persistence
- `skills/` — MCP-server skills (memory, skill-writer, self-improving, …)
- `mobile/` — Flutter companion app
- `installer.sh`, `Dockerfile`, `docker-compose*.yml` — install & deploy

Depends on [bastion-core](https://github.com/thewaifucorp/bastion-core) for
the runtime substrate (agent loop, capabilities, memory, cognition,
personas, mesh, providers, extension protocol).

[0.1.0]: https://github.com/thewaifucorp/bastion-agent/releases/tag/v0.1.0
