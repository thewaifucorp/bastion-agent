# Changelog

All notable changes to `bastion-agent` are documented here. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versioning is a
product/release number (see
[bastion-core's VERSIONING.md](https://github.com/thewaifucorp/bastion-core/blob/main/docs/VERSIONING.md)
for how that differs from the library crates it depends on).

## [Unreleased]

## [0.1.3] — 2026-07-17

### Added

- In-TUI subscription backends: `/backend` command and picker to switch the
  conversation runtime between Claude Code, Codex, and opencode subscriptions
  (and back to API-key models) live, without restarting the daemon. The
  selection is persisted and the picker shows each runtime's health and login
  status.
- In-TUI subscription login: `/connect claude|codex|opencode` suspends the TUI
  and runs the provider's login flow inside the core container, then reports the
  refreshed status — no manual `docker compose exec` required.
- `bastion connect` now runs real, provider-correct login verbs
  (`claude auth login`, `--setup-token` for a long-lived token), verifies the
  result, and offers `--import-host` to copy existing host credentials into the
  container so you don't have to log in again.
- `GET /status` endpoint reporting per-runtime CLI presence and login state
  (booleans only), plus startup logs that warn when a selected runtime is not
  logged in.
- Unified command catalog as the single source of truth for command scope,
  TUI autocomplete, the remote allow-list, and `/help`.
- Fuzzy command matching with "did you mean …?" suggestions across the TUI,
  webhook, and console.
- TUI input editing: cursor movement (Left/Right/Home/End/Ctrl+A/Ctrl+E) and
  command history (Up/Down), multibyte-safe.
- Shell completion generation via `bastion completions <shell>`, installed for
  bash by the installer with printed zsh/fish instructions.
- opencode subscription as a selectable backend in the installer.
- Interactive `/pet` subcommands for viewing stats, toggling game mode, caring
  for the companion, putting it to sleep, and selecting a pet pack; completion
  options remain visible after typing `/pet `. Nested menus offer eight foods,
  eight activities, and four rest durations with distinct care effects, while
  the status card visualizes XP and care as progress bars. Emoji-assisted menus
  and a capped human-input momentum reward complement fixed XP for completed AI
  replies without rewarding generated token volume.
- Instant `/theme` switching with named palettes or a custom `#RRGGBB` accent,
  persisted in `~/.config/bastion/tui.json` across sessions.
- Pixel-art Keeper and Patchwork mascot rendering through supported terminal
  graphics protocols, with state-specific faces and seals, automatic text-mode
  fallback, and a `BASTION_TUI_GRAPHICS=off` override.
- Persistent daemon-wide provider/model selection with a local
  recommended-model picker, secure `/connect` setup guidance, and `/model reset`
  to restore the configured default.

### Changed

- `/model` is now the canonical command; `/models` is an alias, and the
  model-browse picker opens for either form.
- Typed CLI arguments (`ValueEnum`) for the `connect` provider and the
  `companion` event/care actions, so invalid values are rejected with the valid
  options instead of failing at runtime.
- The container now persists the whole `/home/bastion` as a Docker volume so all
  CLI auth state (including `~/.claude.json`) survives `--force-recreate`;
  `volume-init` pre-creates and chowns the credential directories, fixing broken
  ownership when the host user id is not 1000.
- The webhook's container-side port is fixed at 8080; only the host-published
  port (`BASTION_HTTP_PORT`) is configurable, removing a silent connectivity
  footgun.

### Fixed

- Command errors (unknown model, missing API key) now surface an actionable
  message instead of a bare `HTTP 500`.
- `bastion connect` resolves the Compose project directory from any working
  directory (`BASTION_COMPOSE_DIR`, ancestor walk, or the install dir).
- First-run startup fails fast with the real error from the daemon log instead
  of polling an unreachable runtime for two minutes.
- Subscription auth is re-verified lazily, so logging in mid-session works
  without restarting the daemon.

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

- Local Compose publication now binds to `127.0.0.1` by default, avoiding
  false conflicts with services listening on the same port on another host
  interface; publication host and port are configurable.
- The TUI now fails immediately when Compose starts the core without publishing
  its HTTP port instead of polling an unreachable runtime for two minutes.
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
- Docker and installer smoke coverage in CI, plus a dedicated Python
  development dependency group for skill suites.
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

[Unreleased]: https://github.com/thewaifucorp/bastion-agent/compare/v0.1.3...HEAD
[0.1.3]: https://github.com/thewaifucorp/bastion-agent/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/thewaifucorp/bastion-agent/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/thewaifucorp/bastion-agent/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/thewaifucorp/bastion-agent/releases/tag/v0.1.0
