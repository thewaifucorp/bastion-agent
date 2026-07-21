# Control Plane security model

The Control Plane is a planned external HTTP API (`/v1/tasks*`) that lets an
outside orchestrator (e.g. Paperclip) create and drive Bastion's durable
`Pursue` tasks without adopting Bastion's internal Rust types. This document
is the threat model for that surface.

**Phase 5 status (final phase):** every route in the frozen fixture is live
(`src/control_plane/routes.rs`) — `GET /v1/tasks`, `GET /v1/tasks/{id}`,
`GET /v1/tasks/{id}/attempts`, `GET /v1/openapi.yaml`, `POST /v1/tasks`,
`POST /v1/tasks/{id}:pause|:resume|:cancel|:steer`, and
`POST /v1/webhook-subscriptions`, backed by real signed/retried outbound
delivery (`src/control_plane/webhook_delivery.rs`). The same task-store logic
those routes call is now ALSO reachable as 5 MCP tools
(`create_task`/`get_task`/`list_tasks`/`steer_task`/`cancel_task`, see "New in
Phase 5" below) and demonstrated end-to-end by a standalone proof adapter
(`paperclip-adapter/`). See
[`contracts/control-plane-v1.openapi.yaml`](contracts/control-plane-v1.openapi.yaml)
and "Known gaps" below for what's still genuinely absent (an event stream
covering all 5 spec event types, credential/subscription self-service, rate
limiting, a Python SDK).

## What a Control Plane credential is

A credential (`src/control_plane/credential.rs`) is a static, opaque bearer
token (`bcp_<random>`), bound at issuance to exactly one owner, optionally
tagged with a project string, and granted a subset of four scopes:
`tasks:read`, `tasks:create`, `tasks:control`, `webhooks:manage`
(`src/control_plane/scope.rs`).

This is a deliberately different model from the existing webhook channel's
`OwnerMap` (one shared token per owner, no scopes, configured in
`bastion.toml`/`.env`) — see [Configuration](configuration.md) and
[Security model](security.md) for that surface. A Control Plane credential is
issued and revoked at runtime, individually, and never grants more than the
scopes it was issued with.

## Threats and mitigations

| Threat | Mitigation | Status |
|---|---|---|
| A caller supplies its own `owner_id` and reads/mutates another owner's tasks. | `AuthenticatedCredential.owner_id` is derived *only* from the authenticated token's stored row — `authenticate()` and every store method take no caller-supplied owner parameter. A future route handler has no code path to accept an `owner_id` from the request body/query and use it for authorization. | Enforced at the store/credential layer now. Full behavioral proof needs a live route (later phase). |
| A leaked or over-broad credential grants more than intended. | Four independent scopes, checked via `require_scope` — a credential issued with only `tasks:read` cannot create, control, or manage webhooks even if a handler bug tries to call into that path, because the handler must explicitly call `require_scope` with the specific scope the operation needs. | Enforced now (`scope.rs`, unit + integration tested). Wiring `require_scope` into an actual axum extractor is a later phase's job. |
| A credential's plaintext token is recovered from logs, the database, or a crash dump. | Only `sha256(token)` is ever persisted (`token_hash` column); the plaintext is generated and returned exactly once from `issue()` and is never logged, never re-derivable. Lookup on `authenticate()` is by exact hash equality (a SQL index match, not a raw-secret comparison), so no timing side-channel exists to close the way `mcp/server.rs`'s in-memory plaintext-token scan needs `constant_time_eq` for. | Enforced now. |
| A revoked credential keeps working because revocation is soft/delayed. | `revoke()` sets `revoked_at` and is owner-scoped with an IDOR guard (bails on 0 rows changed — never a silent no-op); `authenticate()` filters `WHERE revoked_at IS NULL`, so a revoked token stops resolving on its very next use. Revocation has no un-revoke path — a rotated credential must be reissued. | Enforced now (tested: `revoked_credential_no_longer_authenticates`, `revoked_credential_is_denied_end_to_end`). |
| One owner's credential list/revoke call reaches another owner's credentials (IDOR). | Every store method that reads or mutates by id is scoped with `WHERE owner_id = ?`; `list_for_owner`/`revoke` cannot be called without an owner and cannot return or affect another owner's rows. | Enforced now (tested: `credentials_are_isolated_across_owners`, `revoke_is_owner_scoped_idor_guard`). |
| "Project" isolation is assumed to isolate task visibility, but doesn't yet. | `project: Option<String>` is stored on the credential (and will be carried by `external_ref`/DTOs) but **no query anywhere filters by it**. `bastion-core`'s `TaskCase`/`SqliteTaskStore` have no `project` field at all — isolation, if any, is entirely owner-scoped until a later phase decides how `project` is actually enforced. | **Not enforced.** Treat `project` as a label, not a boundary, until a later phase says otherwise. |
| An orchestrator retries a `create` call (network blip) and duplicates execution. | `POST /v1/tasks` requires `Idempotency-Key`; `create_task` derives a stable `TaskCaseId` from `sha256(owner \|\| idempotency_key)` and checks `load_case` for it BEFORE ever calling `create_case` — a retry with the same owner+key finds the original and returns it (`200`, nothing new created) without re-running the request body at all. `create_case` is called with the derived id (not the raw header) as Core's own idempotency key, closing the residual TOCTOU race as defense-in-depth. | Enforced now (tested: `create_task_is_idempotent_on_owner_plus_idempotency_key`, `create_task_same_idempotency_key_different_owner_creates_separate_tasks`). |
| A mutation (pause/resume/steer/cancel) races a concurrent change and silently clobbers it. | Every mutation route requires `expected_revision` in the body and threads it directly into `TaskStore::transition_status`/`update_case` (both owner+revision-guarded at the SQL layer in `bastion-core`, unmodified). The route ALSO pre-checks locally (terminal status, valid state transition, matching revision) before ever calling the store, so the common "stale revision" case gets a sharp `409 stale_revision` instead of a generic conflict. | Enforced now (tested: `pause_with_stale_revision_returns_409_and_does_not_mutate`, `pause_a_pending_task_is_an_invalid_transition_409`, `cancel_an_already_terminal_task_returns_409`, `steer_on_terminal_task_returns_409`). |
| Evidence/attempt detail exposed by the API leaks more than a "safe summary." | `AttemptSummaryDto` deliberately excludes `actions`, `belief_refs`, and the `Verdict`'s `provenance`/`detail`/`evidence` ids — only id, timestamps, a coarse verification status, and usage numbers are modeled. Full artifact retrieval is explicitly deferred to "an explicit allowed scope and a signed/expiring retrieval route" per the spec — not built this phase. | DTO shape enforces this by construction (there is no field to accidentally serialize). No retrieval route exists yet, safe or otherwise. |
| A registered webhook subscription is used to reach an internal/loopback address (SSRF). | `SqliteWebhookSubscriptionStore::issue` calls `adaptive::browser::validate_public_url` — the SAME guard `HttpFetchBackend` runs before every page fetch (US-204: rejects loopback, private/RFC1918, link-local incl. the cloud-metadata address, unspecified, non-http(s) schemes). Real DNS resolution, not a string-pattern check. | Enforced now (tested: `issue_rejects_a_loopback_target_url`, `issue_rejects_a_private_range_target_url`, `issue_rejects_a_non_http_scheme`, `create_webhook_subscription_rejects_a_loopback_target_url` at the route level, `create_webhook_subscription_succeeds_for_a_real_public_url` proving the allow path against a real DNS lookup). See "New in Phase 4" below for the one narrowing versus `browser.rs`'s guarantee. |
| A credential-issuance endpoint itself becomes an unauthenticated privilege-escalation path. | Not in scope — no HTTP route exists for issuing credentials (`issue()` is a library function, callable only from trusted host code such as a future CLI subcommand or an already-authenticated owner-facing surface). Deciding how an owner requests a *new* Control Plane credential without already having one is an open question for a later phase. | Deferred; flagged so it isn't forgotten. |
| A delivered webhook payload is forged or tampered with in transit. | Every delivery is HMAC-SHA256-signed (`webhook_delivery::sign_payload`) over the exact bytes sent, `X-Bastion-Signature: sha256=<hex>` — the same `sha256=<hex>` shape `channel::whatsapp`'s INBOUND verification already uses, now mirrored outbound. The TS SDK's `verifyWebhookSignature` is the receiver-side counterpart, constant-time compared. | Enforced now (tested: `sign_payload_is_deterministic_and_hex_prefixed`, `deliver_one_sends_a_verifiable_signature_to_a_local_server`, and the TS `webhook.test.ts` suite, which cross-checks against an INDEPENDENT `node:crypto` HMAC implementation, not just its own code). |
| A subscriber's endpoint is temporarily down and a delivery is lost. | `SqliteWebhookDeliveryStore` is a durable queue (own SQLite table) with exponential backoff (30s/2m/10m/30m/1h/2h across 6 retries, 7 attempts total before giving up) — a delivery survives a daemon restart mid-retry. | Enforced now (tested: `mark_failed_reschedules_into_the_future_not_immediately_due`, `exhausting_all_attempts_marks_the_delivery_dead`). |
| A disconnected/slow webhook receiver stalls the API call that triggered the event (spec acceptance criterion). | `emit_event` (called from `create_task`/`transition_action`) only does a fast local DB write (`enqueue_event_for_subscribers` → one INSERT per matching subscription) — the actual outbound HTTP POST happens later, out-of-band, in `run_delivery_loop`. Awaiting the enqueue inline never blocks on network I/O. | Enforced by construction — no route handler calls `deliver_one` directly. |

## What "existing checks stay inside Bastion" means here

The spec's own non-goal is explicit: the Control Plane "cannot bypass"
Bastion's capability, privacy, approval, budget, and channel-trust checks.

Phase 3's `create_task`/mutation handlers do **not** literally call
`adaptive::enqueue_pursue`/`task_command::transition`/`task_command::steer` —
those functions can't accept a caller-supplied `acceptance`/`bounds` or
`expected_revision` (see the code comments on `create_task` and
`transition_action` in `src/control_plane/routes.rs` for exactly why). This
is a deliberate, narrower claim than "calls the identical function": neither
`enqueue_pursue` nor Phase 3's handlers touch `CapabilityRegistry`/
egress-gate/approval-queue AT CREATION TIME — both are pure `TaskCase`
construction plus a `TaskStore` write. What actually matters for this
invariant is:

- Task creation only ever produces `ExecutionMode::Pursue` cases
  (`CreateTaskRequest` has no `mode` field; `create_task` hardcodes it) —
  the Control Plane supplies an *objective*, never a privileged execution
  mode or a way to skip the adaptive loop.
- Every mutation (`transition_status`/`update_case`) goes through the SAME
  `TaskStore` trait methods `task_command.rs` uses, with the SAME owner+
  revision guards — not a parallel, lighter-weight path.
- Once a task actually **runs** (adapts, chooses actions), it goes through
  the normal `AdaptiveCycle`/`CapabilityRegistry`/egress-gate/approval-queue
  machinery regardless of how it was created — Phase 3 does not touch that
  machinery at all, so it cannot weaken it.
- Nothing here creates a new provider-egress path. `check_egress`
  (`bastion-runtime::hooks::egress`) is untouched.

## Resolved: which header carries the credential

**Decided in Phase 2: `x-bastion-token`**, not `Authorization: Bearer`.
Consistent with every other authenticated surface in this codebase (the
webhook channel's `resolve_owner_or_401`, MCP's `authenticate_token`) —
`src/control_plane/routes.rs`'s `resolve_credential_or_401` mirrors both
exactly, against the Control Plane credential store instead of `OwnerMap`.
The fixture's `securitySchemes.bastionToken` reflects this (`type: apiKey`,
`in: header`, `name: x-bastion-token`). Interoperability with
off-the-shelf `Authorization: Bearer`-only HTTP clients was the argument for
the alternative; consistency with the rest of the product won.

## New in Phase 2: GET-route specifics

- **404 never distinguishes "wrong owner" from "no such task."**
  `get_task`/`get_task_attempts` call `TaskStore::load_case(owner, id)`,
  which is IDOR-safe by construction (`bastion-core`, unmodified) — a
  caller can never learn whether an id exists under a *different* owner by
  comparing 404 responses, the same discipline
  `credential::SqliteCredentialStore::revoke`'s existence check uses.
- **`list_tasks` never embeds attempts** (`attempts: []` on every item) to
  avoid an N+1 `list_attempts_for_case` fan-out over a potentially large
  list. `get_task` and `get_task/{id}/attempts` do include them. A client
  that needs attempt detail for many tasks must call `/attempts` per task —
  documented behavior, not an oversight.
- **Pagination is an app-layer slice, not a real cursor query.**
  `bastion_runtime::task::TaskStore` has no `LIMIT`/`OFFSET`/keyset support —
  `list_cases_for_owner`/`list_attempts_for_case` return everything for the
  owner/case in one call (confirmed against the pinned dependency). Every
  `/v1/*` list endpoint fetches the full set, sorts it deterministically
  (timestamp DESC, id DESC as a tiebreaker), and slices in
  `control_plane::pagination`. Fine at personal-agent scale; would need a
  real `TaskStore` extension (a `bastion-core` change) before it scales to
  thousands of cases per owner.
- **`GET /v1/openapi.yaml` is deliberately unauthenticated** — an API
  publishing its own schema publicly is the norm (Swagger UI, etc.) and the
  document carries no secret material.
- **Colon action routes, resolved.** `POST /v1/tasks/{id}:pause` (etc.) is
  registered on the SAME route entry as `GET /v1/tasks/{id}` — `matchit`
  can't natively split `{id}:pause`, so `task_action`
  (`src/control_plane/routes.rs`) captures the whole segment and manually
  `rsplit_once(':')`s it. The URL a client sends is unchanged from the
  fixture; only the internal registration differs. See [`router`]'s doc
  comment for the full reasoning this section originally flagged.

## New in Phase 3: mutation-route specifics

- **`external_ref` lives in `business_state`, not a new table.**
  `TaskCase.business_state` is host-owned opaque JSON
  (`bastion_runtime::task::OpaqueState`) the kernel never interprets.
  `agent::task_command::steer` already has a convention for it — a JSON
  array of tagged note objects, appended to on every steer call. Phase 3's
  `create_task` writes `external_ref` using that SAME convention
  (`control_plane::business_state`) specifically so a later TUI/chat
  `/task steer` call on an API-created task cannot clobber the
  `external_ref` it set — the two code paths freely interleave on the same
  field (tested: `task_resource_recovers_external_ref_from_business_state`,
  `steer_appends_a_note_and_preserves_external_ref`).
- **Two real bugs this phase's tests actually caught before Docker:**
  1. `CreateTaskRequest`'s optional fields (`external_ref`, `acceptance`,
     `bounds`) needed explicit `#[serde(default)]` — serde does NOT treat a
     missing JSON key as `None`/`vec![]` for an `Option<T>`/`Vec<T>` field on
     its own. Every DTO in this module with an optional wire field now has
     the attribute (`dto.rs`).
  2. `SqliteTaskStore::create_case`'s `idempotency_key` uniqueness is
     **global**, not owner-scoped. Passing the raw `Idempotency-Key` header
     value straight through would let two different owners submitting the
     same literal key collide — Core silently no-ops the second owner's
     insert (its own documented idempotent-create contract), and that owner
     would 500 on the follow-up re-fetch. Fixed by passing the
     already-owner-scoped derived `task_id` as Core's idempotency key
     instead (matching `adaptive::enqueue_pursue`'s own convention of using
     its generated id the same way).
- **Mutation error mapping is local-pre-check-then-generic-409.**
  `transition_action`/`steer_action` check terminality, the state-machine
  transition (`TaskStatus::can_transition_to`), and the revision match
  LOCALLY (from data `load_case` already returned) before ever calling the
  store, so the common cases get a specific `409` code
  (`task_terminal`/`invalid_transition`/`stale_revision`). The store call
  itself has no typed error to split further (`bastion-core`'s
  `transition_status`/`update_case` are plain `anyhow::bail!` strings — see
  the code comment on `transition_action`), so any failure THERE (a genuine
  race in the gap between the read and the write) collapses to a generic
  `409 conflict`. Known imprecision, not a bug — flagged in case a future
  phase wants a typed error upstream in `bastion-core` instead.
- **Audit trail is BOTH `tracing` AND the webhook event stream now.** Every
  successful mutation still logs a structured `tracing::info!`
  (`control_plane_task_created`/`_transitioned`/`_steered`) — unchanged from
  Phase 3 — AND (Phase 4) calls `emit_event`, which builds a
  `TaskEventEnvelope` and enqueues a signed delivery to every ACTIVE
  subscription matching that event type for that owner. `emit_event` does
  NOT go through `bastion_runtime::task::TaskLifecycleEvent`/`AdaptiveCycle`
  (confirmed still unwired to `SqliteTaskStore` mutations or
  `task_command.rs`) — it's a Control-Plane-local construction, independent
  of that mechanism.

## New in Phase 4: webhook specifics

- **The SSRF guard's guarantee is narrower than `browser.rs`'s.**
  `validate_public_url` runs ONCE, at `POST /webhook-subscriptions` time —
  never again on each delivery. `browser.rs`'s own doc comment already notes
  its guarantee has a residual DNS-rebinding gap between the check and the
  socket connect; this module's gap is wider (the window is "until the
  subscription is revoked," not "the next few milliseconds") because
  re-checking on every delivery was deliberately rejected — it would also
  make `deliver_one` untestable against a local mock server without adding a
  `cfg(test)` escape hatch to the SSRF guard itself, which was judged worse.
  Practical implication: an attacker would need to gain control of DNS for
  an ALREADY-REGISTERED subscriber hostname (not just any hostname) to
  exploit this — a real but narrow threat, documented rather than silently
  accepted.
- **Only 3 of the 5 spec event types are actually emittable.**
  `task.created` (from `create_task`), `task.status_changed` and
  `task.terminal` (from `transition_action`, the latter only when the new
  status is terminal) are real. `attempt.completed` and `task.escalated`
  are NOT emitted from anywhere — both would need to fire from the
  execution/adaptive-loop machinery (an attempt finishing, an escalation
  decision), which no Control Plane route touches. `steer_action`
  deliberately emits nothing — steering isn't a status change and doesn't
  fit any of the 5 event types.
- **The webhook signing secret round-trips in the response DTO.**
  `WebhookSubscriptionResource` gained a `secret: Option<String>`
  (`#[serde(skip_serializing_if = "Option::is_none")]` — omitted entirely,
  never serialized as `null`, when absent) after this was caught as a real
  gap during Phase 4 review: the DTO originally had no field for it at all,
  so `create_webhook_subscription` would have discarded the secret
  `SqliteWebhookSubscriptionStore::issue` generates, making the endpoint
  unusable for real signature verification — a functional bug, not cosmetic.
  Fixed before shipping (tested:
  `webhook_subscription_resource_omits_secret_key_when_none` for the
  omit-when-absent shape, `create_webhook_subscription_succeeds_for_a_real_public_url`
  for the route genuinely returning a non-empty secret on creation).
- **`run_delivery_loop` ticks every 5s** (`main.rs`), sweeping ALL owners'
  pending deliveries in one pass — same unscoped-sweep shape as
  `adaptive::schedule::run_scheduler`. Cheap when the queue is empty (one
  `SELECT`); no backpressure/concurrency limit on how many deliveries fire
  per tick, acceptable at personal-agent scale.

## New in Phase 5: MCP tool exposure + core_ops extraction + Paperclip adapter

- **`routes.rs`'s task-store logic moved into `core_ops.rs`.** Every HTTP
  handler that touches `TaskStore`/emits events (`list_tasks`, `get_task`,
  `get_task_attempts`, `create_task`, `transition_task`, `steer_task`) is now
  a thin wrapper: parse headers/body, call the matching `core_ops` function,
  map its typed `CoreOpError` to a `StatusCode`/`ErrorEnvelope`. This is not
  a refactor for its own sake — it's what makes the MCP tools below share
  the EXACT same task-store behavior, event emission, and error conditions
  as the HTTP routes, from one implementation, rather than a second one that
  could silently drift. All 34 existing route integration tests and all 16
  fixture tests pass unchanged against the refactored handlers (regression
  check, not new coverage).
- **5 MCP tools, one dedicated registry, deliberately NOT the shared one.**
  `create_task`/`get_task`/`list_tasks`/`steer_task`/`cancel_task`
  (`src/control_plane/mcp_tools.rs`, `#[cfg(feature = "mcp-server")]`) are
  `Capability` impls wrapping `core_ops`. They are registered into a
  SEPARATE `CapabilityRegistry`
  (`control_plane::mcp_tools::build_registry`), never
  `agent.capability_registry` — the registry every OTHER capability in this
  codebase shares with Bastion's own internal LLM tool-calling loop
  (`agent/loop_.rs`). Registering these 5 into the shared registry would
  have silently let a running Pursue task's own reasoning call
  `create_task`/`cancel_task` on itself or a sibling task — a real,
  unrequested capability the planning doc never asked for; the doc's whole
  premise is an EXTERNAL orchestrator driving tasks, not internal
  self-modification. `BastionMcpServer` (`src/mcp/server.rs`) now holds
  both registries and checks the Control-Plane one first in `call_tool`
  (`control_plane_registry.list_names().contains(...)`), merging both into
  `list_tools`' listing (re-sorted after the merge to preserve the existing
  byte-stable-ordering guarantee, COST-01/D-14b).
- **MCP auth reuses `mcp::server::TokenPermissions`, not a second Control
  Plane credential.** An MCP caller is authenticated by
  `authenticate_token`'s existing fail-closed check before
  `CapabilityRegistry::invoke` is ever reached; `InvokeCtx.owner` (from the
  token's configured `owner_id`) is passed straight to `core_ops` as the
  owner string. There is no per-tool scope gate mirroring
  `tasks:read`/`tasks:create`/`tasks:control`/`webhooks:manage` — an MCP
  token that can invoke tools at all can do everything `core_ops` exposes
  for its owner, matching how every other MCP tool has no finer-grained
  scoping today, only the blanket `read_only` flag.
- **Read-only MCP tokens still can't invoke ANY tool, including the new
  read-only-safe ones.** `call_tool`'s `perms.read_only` check
  (`mcp/server.rs`) is a pre-existing, blanket "read-only token cannot
  invoke tools" gate applied before dispatch — unchanged by this phase. A
  read-only token cannot call `get_task`/`list_tasks` even though those are
  logically read-only, because that per-tool distinction doesn't exist
  anywhere in this server yet (not a Control-Plane-specific gap; every
  other tool has the same blanket restriction). Left alone deliberately —
  loosening it for two specific tool names would be a broader MCP-server
  policy change beyond "expose 5 Control Plane tools," and no read-only MCP
  token is configured anywhere in this deployment today.
- **`create_task`'s MCP schema requires `idempotency_key`, mirroring the
  HTTP route's required `Idempotency-Key` header.** No "lighter-weight"
  MCP path that skips idempotency — `core_ops::create_task` validates it's
  non-empty regardless of which surface called it (HTTP's header-absence
  400 is a transport-only pre-check on top).
- **`cancel_task`/`steer_task`'s MCP schemas require `expected_revision`**,
  same OCC contract as the HTTP mutation routes — an MCP caller cannot
  cancel/steer "blind" any more than an HTTP caller can.
- **MCP tool errors carry the same stable `code` slugs as `ErrorEnvelope`,
  in plain text.** `rmcp` has no structured tool-error channel (a failed
  `call_tool` is flat `Content::text`) — `mcp_tools::map_core_op_error`
  prefixes each message with the identical slug HTTP's `ErrorEnvelope.code`
  uses (`not_found:`, `stale_revision:`, `task_terminal:`, ...) so a caller
  parsing either surface's errors uses the same vocabulary.
- **`paperclip-adapter/` is a standalone crate, not a Rust dependency on
  `bastion`.** Paperclip's actual codebase isn't available to this repo, so
  this is a reference implementation proving the public `/v1/*` HTTP
  contract alone is sufficient: `heartbeat`/`poll`/`cancel`, each built only
  against `docs/en/contracts/control-plane-v1.openapi.yaml`'s documented
  shapes (`paperclip-adapter/src/types.rs` hand-transcribes them
  independently — it does not import `control_plane::dto`). Terminal
  outcomes are mapped from `status`/`stop_reason.kind`'s typed
  discriminants, never from parsing `reason`/`dimension` string content.
  Session state the caller must persist is exactly `{task_id, revision}` —
  the adapter has no database of its own. Tested against a mocked server
  (`wiremock`, `paperclip-adapter/tests/adapter.rs`) for request shape and
  typed-outcome mapping; a real end-to-end run against a live `bastion-core`
  container (create → poll → cancel against a genuine SQLite-backed task)
  was performed manually for Phase 5 sign-off.

## Known gaps carried forward past Phase 5

Two of the original gaps were closed by the observability frontend work
(see [Observability](observability.md)):

- ~~`attempt.completed`/`task.escalated` are not emitted~~ — CLOSED. The
  adaptive execution loop now emits both through
  `observability::LifecycleObserver` (`src/observability.rs`) into the same
  signed, durable delivery queue as the Phase 4 events: `attempt.completed`
  on every attempt verification, `task.escalated` on a terminal transition
  to `Escalated` (single-cycle convergence failures and delegated-parent
  aggregation failures alike).
- ~~No credential-issuance surface exists~~ — PARTIALLY CLOSED. The
  console-only `/credential` command (`src/agent/credential_command.rs`)
  issues/lists/revokes credentials; the plaintext token prints exactly once
  to the operator's terminal. Deliberately still NOT reachable from `/v1/*`
  or any remote channel — issuance stays a trusted-host operation.
  Webhook-subscription management (list/revoke) remains absent.

Still open:
- `project` is stored but not enforced anywhere, including by every route
  added so far (`list_tasks`/`create_task` scope by owner only).
- No rate limiting on any route or MCP tool.
- No Python SDK (spec: "Python SDK second") — the TS SDK (`sdk/typescript/`)
  is the only client library this project shipped.
- MCP tools have no per-tool scope gate (`tasks:read` vs `tasks:control`
  etc.) the way HTTP credentials do — see "New in Phase 5" above. An MCP
  token that can call tools at all has the full `core_ops` surface for its
  owner.
