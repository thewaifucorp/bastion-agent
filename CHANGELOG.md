# Changelog

All notable changes to `bastion-agent` are documented here. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versioning is a
product/release number (see
[bastion-core's VERSIONING.md](https://github.com/thewaifucorp/bastion-core/blob/main/docs/VERSIONING.md)
for how that differs from the library crates it depends on).

## [0.1.2] — 2026-07-16

### Added

- Official `bastion` terminal UI as the default command. It discovers or starts
  the local runtime, waits for readiness, and consumes local bootstrap access
  automatically; `bastion chat` remains the explicit remote form.
- Host CLI installation under `~/.local/bin`, backed by the release binary
  already built in the Docker image.
- Responsive RGB terminal companion with onboarding, guard, build, Cabinet,
  success, and alert states; configurable themes and custom accents.
- Opt-in companion progression and non-punitive feeding, water, play, and rest
  reminders based only on observed active time.
- Declarative, authority-free animated pet packs, a generator/validator skill,
  and CLI/MCP lifecycle event bridges for external coding agents.

### Fixed

- First-run local TUI no longer enters the device-pairing flow or requires the
  user to start a separate daemon manually.
- CLI installation now resolves the freshly built core image before any
  Compose containers exist.
- Compose services now use project-scoped names, allowing installed and
  development stacks to coexist without global container-name conflicts.

## [0.1.1] — 2026-07-16

### Added

- Official `bastion chat` terminal UI, with daemon URL, owner, and token
  configuration through CLI flags or `BASTION_*` environment variables.
- Bootstrap-token access for a fresh self-hosted deployment.
- Docker and installer smoke coverage in CI, plus a dedicated
  `requirements-dev.txt` for Python skill suites.
- Explicit memory-system lineage and attribution for the ideas combined from
  Mem0 and MemPalace.

### Changed

- Reworked the project documentation around Bastion as a trustworthy Life OS:
  cross-domain life management, Cabinet reprioritization, adaptive memory,
  proactive assistance, and policy-enforced execution.
- Replaced the legacy installer with an idempotent Docker-first flow using
  `.env`, `bastion.toml`, generated secrets, Compose validation, and health
  checks.
- Rebuilt the agent image as a Debian multi-stage, non-root image. Rust builds
  default to two parallel jobs to remain usable on memory-constrained machines.
- Pinned `bastion-core` crates to a public Git revision so the agent repository
  builds independently from a sibling checkout.
- Moved the mesh E2E stack and node fixtures under `tests/mesh/`.
- Limited onboarding and persona assignment to locally available Bastion
  skills. The optional vulnerability source now uses a generic skill-registry
  contract.
- Updated channel startup to respect each channel's `enabled` setting.

### Fixed

- Pairing tokens now preserve the canonical owner identity separately from
  device metadata.
- Container configuration no longer masks the memory model baked into the
  image, and Compose now injects the current MCP URLs and local paths.
- Remote TUI startup no longer requires a local `bastion.toml`.
- Python skill-test documentation now avoids cross-suite module-name
  collisions.

### Removed

- The Pokedev CLI name, configuration directory, and binary surface.
- Legacy OpenClaw/ClawHub bootstrap, discovery, installation code, manifests,
  migration guides, and stale localized documentation.

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

[0.1.1]: https://github.com/thewaifucorp/bastion-agent/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/thewaifucorp/bastion-agent/releases/tag/v0.1.0
