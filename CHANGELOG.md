# Changelog

All notable changes to `bastion-agent` are documented here. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versioning is a
product/release number (see
[bastion-core's VERSIONING.md](https://github.com/thewaifucorp/bastion-core/blob/main/docs/VERSIONING.md)
for how that differs from the library crates it depends on).

## [Unreleased]

### Added

- **Structured persona contract v2 form (C0-P4)**: `web/src/views/Personas.tsx`
  replaces the raw-textarea-only editor with the structured form as the
  PRIMARY way to edit a persona, built from C0-P3's parsed `GET
  /personas/{slug}` `contract`.
  - Fields: name, description, objectives/goals/skills as editable
    add/remove/reorder string lists, an operating scope textarea, a
    privacy-tier select (`local-only`/`cloud-ok`), a weight number input,
    and a tools control that's an explicit two-way toggle — "unrestricted"
    (omits `tools` entirely, contract-v2's `None`) vs. "allowlist" (a
    checklist sourced from `GET /loadout`'s `tools[]`, plus a free-typed
    custom-capability input, since custom ids are legal and the allowlist
    must be non-empty once chosen).
  - On save the form ASSEMBLES the full SOUL.md: a hand-built YAML
    frontmatter (name, description, `bastion:{privacy_tier, weight}`,
    objectives, goals, tools, scope, skills — string scalars double-quoted
    JSON-escaped, which is valid YAML, rather than a bare-word heuristic)
    followed by `---` and the ORIGINAL markdown body recovered from the
    persona's current raw content by mirroring bastion-core's `parse_soul`
    split (strip leading `---`, split at the closing `\n---`, trim leading
    whitespace) — the form edits the frontmatter only, the persona's prose
    is never touched.
  - Client-side validation mirrors `PersonaFront::validate()` (objectives/
    goals non-empty, scope present, a chosen allowlist non-empty) with
    inline field errors shown after the first submit attempt; a 400 from
    `POST /proposals` (C0-P3's `{"problems": [...]}`) is now carried on
    `ApiError.problems` and rendered as the same banner.
  - A legacy persona (parses, but `validate()` problems non-empty) shows an
    "upgrade this persona to contract v2" banner listing what's missing,
    still editable through the same form. An unparseable persona (`contract:
    null`) forces a raw-SOUL.md fallback mode (the structured toggle is
    disabled — there's nothing to seed fields from) so it's still fixable.
    A manual "raw"/"structured" toggle also stays available as an advanced
    escape hatch for any persona.
  - `web/src/api.ts`: `PersonaContract`/`PersonaReadResponse` types for the
    C0-P3 response shape; `ApiError` gained an optional `problems?:
    string[]` populated from a 400 body's `problems` array.
  - Pending-proposal staging UX (the `/proposal approve <id>` note) and the
    `configTick`-driven refresh on `config.change_requested`/
    `config.applied` (wired in `web/src/App.tsx`, matching Providers/Models)
    are unchanged from C0-P3/earlier.

- **Agent-side persona contract v2 validation (C0-P3)**: the web PROPOSES a
  `persona_edit`, but nothing wrote a SOUL.md that fails to parse or
  declares an incomplete contract-v2 (empty `objectives`/`goals`/`scope`, or
  an explicit-but-empty `tools` allowlist) until now — both the web POST and
  the console's approve accepted anything under the size cap.
  - `src/proposals.rs`: `validate_persona_contract(content: &str) ->
    Result<(), Vec<String>>` — the ONE shared gate, built on the pinned
    core's `bastion_personas::persona::parse_soul` +
    `PersonaFront::validate()`. `apply()`'s `PersonaEdit` branch now calls it
    before writing (and before the backup copy): a parse or validate failure
    bails with every problem listed, joined readably, and nothing is
    written.
  - `src/loadout.rs` `proposals_create_handler`'s `persona_edit` arm calls
    the same helper and answers `400` with `{"problems": [...]}` on failure
    — the web gets immediate feedback instead of only discovering the
    rejection when the console tries (and fails) to approve.
  - `GET /personas/{slug}` (`persona_read_handler` / the new pure
    `persona_read_body` helper) now also returns the parsed structured
    contract: `{slug, content, contract: {name, description, objectives,
    goals, tools, scope, skills, privacy_tier, weight} | null, problems:
    string[]}`. A successful parse always populates `contract` (even a
    legacy SOUL.md missing every v2 field) with `validate()`'s problems
    listed alongside it, prompting the P4 web form to offer an upgrade
    rather than silently accepting it; an unparseable file still answers
    `200` with the raw `content`, `contract: null`, and the parse error as
    `problems`' one entry — never a `500`.

- **TUI config-applied notice + companion event HTTP forwarding (A4-U/A5 S6)**:
  closes the two gaps S1-S5 left open — the TUI never reacted to a
  `config.applied` from elsewhere, and `bastion companion event` always
  wrote `companion.json` directly even with a daemon (and its own
  `CompanionEventCapability`) already running as the single writer.
  - TUI (`run_app`'s `AppMsg::SseEvent` arm, `src/tui.rs`): a `config.applied`
    frame whose `key` is `model.selected`, `backend.selected`,
    `model.fallbacks`, or `routing.rules` now surfaces a one-line notice in
    the transcript (`"config updated from <origin>: <key>"`), reusing the
    same `Line::System` mechanism other inline confirmations use. No new
    cache to invalidate: `/model`, `/backend`, `/models`, and `/routing` all
    read the daemon fresh over HTTP on every invocation, so the notice is
    the whole fix. Pure parsing helper `config_applied_notice` mirrors
    `is_companion_updated_event`'s shape (`event`-or-`type`, tolerant of
    malformed/foreign frames).
  - `POST /companion/event` (owner-token, `src/loadout.rs`) `{event,
    source}` — same `session-start | activity | session-stop` kinds and
    `^[A-Za-z0-9._-]+$` (1-32 char) source validation
    `CompanionEventCapability`'s schema already enforces. Routes through the
    SAME `CompanionHandle::record_event` the capability uses, persists,
    broadcasts `companion.updated`, and answers with the updated snapshot
    plus the recorded-event message (the same due-cue-aware text the
    direct-file path always returned).
  - `bastion companion event` (CLI) is now async and mirrors
    `companion_care`'s S5 forward-then-fallback pattern: detects a reachable
    local daemon (`runtime_ready`/`is_local_url`/`local_bootstrap_token`)
    and forwards over HTTP with the local bootstrap token, falling back to
    the direct file read/write on any failure (stderr warning, never a hard
    error). This closes the S5 gap note below — the CLI and
    `CompanionEventCapability` no longer race on `companion.json` while a
    daemon is running.

- **Companion/buddy on the web, shared daemon state (A5 S5)**: the TUI's
  tamagotchi-style companion gets a web view with the daemon as the single
  writer of `companion.json` while it's running.
  - `src/tui/companion.rs`: `CompanionState::snapshot()` — the one place
    level/XP/need-percent/due-cue formulas are computed, reused by both the
    TUI's own `/pet stats` panel and the new HTTP route; `parse_care_action`
    pulled out of the old inline matches (shared by the CLI and the HTTP
    handler, `"rest"` still an alias for `sleep`).
  - `src/tui.rs`: `CompanionHandle` — an `Arc<Mutex<CompanionState>>`
    wrapper that is the daemon's single in-process writer, shared between
    `POST /companion/care` (`src/loadout.rs`) and
    `CompanionEventCapability`'s hook-triggered session events
    (`src/companion_capability.rs`) instead of each independently
    load()/save()-ing the file. Every mutation broadcasts
    `companion.updated` (`event/type`, `reason` — `care`/`event`/
    `level_up`, `level`, `xp`) on `/events`.
  - `GET /companion` (owner-token): `{game_enabled, level, xp,
    successful_turns, needs: {water, food, play, rest}, cues, frame:
    {rows, width}, pack_name}` — `frame` is a static, markup-stripped
    representative portrait (the pack's idle `guard` frame, or a small
    built-in ASCII face when no custom pet pack is loaded); the TUI keeps
    the full tick-animated experience, this is intentionally simpler.
  - `POST /companion/care` (owner-token) `{action}` (`water`/`feed`/
    `play`/`sleep`, `rest` accepted as an alias) — applies care through
    `CompanionHandle`, persists, broadcasts, and answers with the updated
    snapshot.
  - Standalone CLI (`bastion companion care`, always its own short-lived
    process): now async — detects a reachable local daemon with the same
    `runtime_ready`/`is_local_url`/`local_bootstrap_token` mechanism the
    interactive chat client uses to auto-connect, and forwards the action
    over HTTP with the local bootstrap token when one is running instead of
    writing the file out from under it; falls back to the direct file
    read/write (pre-A5 behavior) on any HTTP failure, with a stderr
    warning. `bastion companion event` had no HTTP counterpart yet at this
    slice (no `POST /companion/event` route) — a documented residual race
    with `CompanionEventCapability` when both were used at once, closed in
    S6 above.
  - The interactive TUI (a separate OS process from the daemon — it talks
    HTTP/SSE, never shares memory) reloads its own `CompanionState` from
    disk when an SSE frame carries `companion.updated`, narrowing but not
    closing a residual two-writer race on `companion.json` — documented in
    code, cosmetic-only state (XP/care timers).
  - Web: a "Buddy" sidebar view — monospace pet frame, four need bars,
    Water/Feed/Play/Sleep care buttons (optimistic refresh from the
    daemon's own response), level/XP, a friendly explanation when
    `game_enabled` is `false`, live refresh on `companion.updated`.

- **LLM routing by call-site class (A4.5 S4)**: route model choice by
  deterministic call-site class — `chat_turn`, `pursue_task`, `cabinet`,
  `reflection`, `compaction` — never semantic classification.
  - `src/routing.rs`: `RouteClass` + `RoutingTable` — effective rule per
    class is the config-store `routing.rules` override else the new
    optional `[routing]` bastion.toml table else nothing. Honest v1: a
    class is `supported` only when the agent can reach a knob on the
    pinned core rev — `chat_turn` (hot `SharedProvider` swap, the `/model`
    mechanism) and `reflection` (Reflector model resolved at startup, so
    next-restart). `pursue_task`/`cabinet`/`compaction` have no
    agent-reachable model knob yet (external-runtime `SessionSpec` has no
    model field; Cabinet and compaction run on the loop's own provider
    inside core) — rules for them are validated, persisted and reported
    `supported: false`, with the required core seam documented in code.
  - `GET /routing` (owner-token): all five classes, always — effective
    model, source (`override`/`toml`/null), `supported`.
  - New proposal kind `routing_config { rules }` (web proposes, console
    approves): class names validated against the enum, model ids must be
    non-empty but are never gated on the catalog (custom ids route by
    prefix, like `/model`). Approve persists the whole map as the
    `routing.rules` override (origin `web`) and applies the supported
    knobs — `chat_turn` hot-swaps live with the same connectivity guard as
    `model_config`; `reflection` lands on the next restart.
  - Startup reads the table: a `chat_turn` rule outranks `default_model`
    when constructing the loop provider; a `reflection` rule outranks
    `[reflector].model`. Both degrade with a warning (never abort boot)
    when the rule's provider is disconnected.
  - Web: a "Routing" matrix inside the Models view — one row per class
    with a model dropdown from the catalog, a clear button, source/draft
    chips; unsupported classes rendered disabled with the honest tooltip.
    Stages `routing_config`; pending proposals show the console-approve
    instruction; re-fetches on `config.applied`.

### Changed

- `config.applied` SSE frames now carry BOTH `"event"` and `"type"` fields
  (repo convention is `"event"`; `"type"` kept for existing consumers —
  the web app already checked both).
- `GET /providers` items now include `display_name` and `env_key`
  (null for local/subscription rows) from the daemon's own provider
  whitelist; the web app's mirrored `PROVIDER_META` table is deleted and
  the views consume the new fields.

- **Provider manager web UI (A4 S3)**: the web app grows real Providers and
  Models views on top of the S2 endpoints — both stage-only, mirroring the
  Personas pattern (web proposes, console approves).
  - **Providers** (replaces the Connect command wrapper; `#/connect` deep
    links redirect): one card per `GET /providers` item — kind chip (API
    key / Subscription / Local), connection state with source label, catalog
    model count. Disconnected API-key providers get an inline "add key" flow
    that stages a `secret_set` proposal: password input, value dropped from
    component state before the POST resolves, never echoed anywhere;
    the card then shows the pending proposal id with the console-approve
    instruction and the restart-expiry caveat. Subscription CLIs point at
    console `/connect`.
  - **Models** (replaces the `/model` command wrapper): the merged catalog
    grouped by provider, the effective default badged, the fallback ladder
    numbered with add/remove/reorder (client-capped at 16). Edits build a
    draft; staging sends only the changed halves as a `model_config`
    proposal. Pending model proposals list with the approve instruction.
  - **Audit strip** (shared component on both views): recent
    `GET /config/overrides` rows (key, origin, relative time) — the single
    audited write path made visible.
  - Both views re-fetch on the `config.change_requested` and
    `config.applied` SSE frames, so an approval on the console updates the
    page live. Backends stays a command wrapper by design.
- **Provider manager core (A4 S2)**: the daemon now knows its own model
  catalog and provider status.
  - `src/model_catalog.rs`: a curated static table of current model ids per
    provider kind (anthropic/openai/gemini/groq/openrouter/ollama),
    classified by the SAME prefix rules the provider registry routes with,
    merged with whatever bastion.toml / config-store overrides actually name
    so custom ids always appear.
  - `GET /models` (owner-token): the merged catalog grouped by provider
    kind, plus the EFFECTIVE `default_model`/`fallback_models`
    (config-store override else bastion.toml).
  - `GET /providers` (owner-token): per-provider connection status —
    booleans only, never key material. API-key providers report `source:
    "env" | "secrets_dir" | null` via the same secret resolvers the daemon
    boots with; `[auth.*]` subscription CLIs are live-probed by exit code
    (like `/status`); ollama reports `kind: "local"` with `connected` =
    "some effective model routes to it" (no network probe from a GET).
- **New proposal kinds (web proposes, console approves)**:
  - `model_config { default_model?, fallback_models? }` — apply writes
    through the unified config store (origin `web`, actor = approving
    owner); the default model hot-swaps the live provider exactly like
    `/model`; the fallback ladder is persisted under the new
    `model.fallbacks` key and loaded at the next startup (the running
    loop's ladder is construction-time — hot swap is a bastion-core seam).
  - `secret_set { provider_id, env_key }` — provider API keys by
    REFERENCE. The value is never written to the proposal table: the web
    POST pens it in memory keyed by proposal id; console approve writes
    `BASTION_SECRETS_DIR/<ENV_KEY>` (0600) and drops it. If the daemon
    restarted in between, approve fails with "re-submit from the web".
    Env keys are validated against `^[A-Z][A-Z0-9_]{2,63}$` AND the known
    provider env-key whitelist; the audit trail records only
    `secret.set:<ENV_KEY> = {"set": true}`.

- **Unified config store (A4-U S1)**: runtime config overrides (`/model`,
  `/backend`) now funnel through one audited write path — an append-only
  `config_overrides` SQLite table (key, JSON value, origin
  `console|web|channel|migration`, actor, timestamp) in the session DB. The
  current value of a key is its latest row; every prior row is retained as
  audit history. Each successful apply broadcasts a `config.applied` event
  on `/events`, so every surface hears about changes live. `bastion.toml`
  stays the declarative base; store overrides overlay it at startup exactly
  like the retired `.bastion/model-selection.json` /
  `backend-selection.json` files did — existing files are imported once
  (origin `migration`) and renamed `*.imported`.
- `GET /config/overrides`: the effective override overlay (latest row per
  key, with origin and applied-at provenance), owner-token authenticated
  like `/loadout` and `/proposals`.

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
  end-to-end by a standalone `integrations/paperclip-adapter/` proof.
- A TypeScript SDK (`sdk/typescript/`) for the Control Plane API.
- A Python SDK (`sdk/python/bastion_control_plane`) mirroring the TypeScript
  SDK field-for-field, zero runtime dependencies (stdlib only), now covered
  by CI (`pytest sdk/python/tests/`, 24 tests).
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

### Fixed

- MCP callers invoking the 5 Control Plane tools were gated only by the
  coarse `read_only` boolean — a non-read-only MCP token could invoke all 5
  uniformly. `TokenPermissions` now carries a `control_plane_scopes` allowlist
  checked per-tool (`tasks:read`/`tasks:create`/`tasks:control`) before
  dispatch, matching the scope model the HTTP `/v1/*` routes already
  enforce; the scope check is gated on the same "does this name actually
  resolve to the Control Plane registry" decision used for dispatch, so it
  can't be fooled by a future shared-registry capability sharing a CP tool's
  name.
- `BastionApiError.__init__` (Python SDK) no longer raises `KeyError` on a
  malformed/non-conforming server error body — missing envelope fields now
  default to an empty string instead of masking the real HTTP error.

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
