# Changelog

All notable changes to `bastion-agent` are documented here. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versioning is a
product/release number (see
[bastion-core's VERSIONING.md](https://github.com/thewaifucorp/bastion-core/blob/main/docs/VERSIONING.md)
for how that differs from the library crates it depends on).

## [Unreleased]

### Added

- **Control Plane**: an embedded, external-facing HTTP API (`/v1/tasks*`) that
  lets an outside orchestrator (e.g. Paperclip) create, list, and drive
  Bastion's durable `Pursue` tasks — pause/resume/cancel/steer — without
  adopting Bastion's internal Rust types. Authenticated by its own scoped
  bearer credentials (`tasks:read`, `tasks:create`, `tasks:control`,
  `webhooks:manage`), independent from the existing webhook channel's token
  model. Signed, retried outbound webhook delivery notifies subscribers of
  task events, and the frozen `/v1/openapi.yaml` contract is drift-tested
  against its DTOs.
- The same task-store logic is now also reachable as 5 MCP tools
  (`create_task`/`get_task`/`list_tasks`/`steer_task`/`cancel_task`), exposed
  through a registry dedicated to external MCP callers, and demonstrated
  end-to-end by a standalone `paperclip-adapter/` proof.
- A TypeScript SDK (`sdk/typescript/`) for the Control Plane API.
- Threat model doc: [`docs/en/control-plane-security.md`](docs/en/control-plane-security.md).
- **Web app (`GET /app`)**: a bundled Vite/React app — vigília (live persona
  lanterns + event ledger), tarefas (durable tasks with attempt verdicts and
  pause/resume/steer/cancel), chat (the console's turn over `/webhook`), and
  config, in the site's arcade-terminal visual identity. Embedded into the
  binary at compile time when `web/dist` exists (releases and the Docker
  image build it; a binary without it answers `/app` with build guidance).
- **Observability frontend**: `GET /ui` serves an embedded, offline-capable
  dashboard — live persona lanterns and a turn/task ledger over `/events`,
  plus the durable-task table with pause/resume/steer/cancel over `/v1`.
  `/events` now carries turn/persona/cabinet events (emitted around persona
  routing) and the adaptive loop's task lifecycle events, alongside
  `mesh_sync`. See [`docs/en/observability.md`](docs/en/observability.md).
- `/credential` console command issues/lists/revokes Control Plane bearer
  credentials (token printed once, console only) — the first
  credential-issuance surface.
- Webhook subscribers now receive the spec's remaining two event types:
  `attempt.completed` (every attempt verification) and `task.escalated`,
  emitted from the adaptive execution loop into the same signed delivery
  queue.

## [0.2.1] — 2026-07-20

### Added

- **Self-update control plane**: the daemon periodically checks the official
  GitHub Release and exposes `/update` in the TUI and trusted channels. An
  explicit `/update apply` reaches a narrowly-authenticated host helper rather
  than granting the container Docker or checkout-write authority; the local
  installer builds, health-checks, and rolls back failed releases.

- **Procedural-memory feedback for `Pursue`**: before delegating work, Bastion
  selects up to four relevant, `CloudOk` procedural beliefs using learned
  utility/confidence and lexical fit; the runtime receives bounded guidance and
  each attempt stores the exact belief references it used.

### Changed

- Terminal, verified `Pursue` attempts now reinforce or penalize only their
  referenced procedural beliefs. Attribution is persisted once per durable task
  (including delegated child tasks), so restart/resume cannot duplicate credit.
  `LocalOnly` beliefs remain outside delegated-runtime prompts.

## [0.2.0] — 2026-07-20

### Added

- **Adaptive Execution**, a progressive request lifecycle selected without an
  extra LLM call: `Respond` is the cheap, side-effect-free default; `Act`
  permits one bounded effect; and `Pursue` creates a durable, resumable,
  owner-scoped `TaskCase`. Explicit mode overrides take precedence over the
  conservative selector.
- A background `Pursue` driver backed by Bastion Core's task contract. Each
  case retains attempts, evidence, verdicts, usage/budgets, status transitions,
  and its recomputed next decision across restart.
- `/task` cockpit: list tasks, inspect their attempts/evidence/verdicts, pause,
  resume, steer, and cancel them. Every lookup and mutation is owner-scoped.
- Delegated coding execution through registered external `AgentRuntime`s.
  Bastion captures runtime diffs, artifacts, usage, and terminal exit status as
  task evidence; it verifies deterministically before accepting a result,
  retries within the task budget, and escalates rather than looping forever.
- Conservative concurrent delegation for independent `Pursue` objectives:
  child tasks retain parent/child provenance, share the parent's budget, obey a
  parallelism bound, and leave divergent child outcomes inspectable.
- A governed, provider-neutral browser capability with per-owner sessions and a
  functional read-only HTTP backend for navigate, snapshot, and download.
  Snapshots return the resolved source URL and truncated text explicitly marked
  `trusted: false`, so web content is never promoted to an instruction source.
- Browser egress safeguards: approval before fetch, public `http(s)` URL
  validation, private/link-local/metadata-address rejection, redirects disabled,
  no cookies or credentials returned, and downloads restricted to new,
  workspace-relative files to prevent path escape and symlink overwrite.
- Durable owner-scoped `/schedule` commands for one-shot and repeating intents.
  Schedules survive restart, have defined missed-run handling, and re-enter the
  same adaptive path when they fire.
- Mode selection for console input, schedules, and inbound channels. Trusted
  inbound requests may create durable tasks; untrusted email and public-channel
  content is classified only for telemetry and cannot escalate into `Pursue` or
  delegated execution.
- Privacy-conscious per-mode tracing and cost attribution: lifecycle events
  expose only identifiers/status, never prompts or evidence payloads.
- English and Brazilian Portuguese Adaptive Execution guides, plus a README
  introduction to task mode.

### Changed

- Updated all `bastion-*` dependencies to the Core revision that supplies the
  Adaptive Execution task substrate.


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

[Unreleased]: https://github.com/thewaifucorp/bastion-agent/compare/v0.2.1...HEAD
[0.2.1]: https://github.com/thewaifucorp/bastion-agent/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/thewaifucorp/bastion-agent/compare/v0.1.3...v0.2.0
[0.1.3]: https://github.com/thewaifucorp/bastion-agent/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/thewaifucorp/bastion-agent/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/thewaifucorp/bastion-agent/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/thewaifucorp/bastion-agent/releases/tag/v0.1.0
