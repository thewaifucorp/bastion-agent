//! Frozen v1 wire contract for the Control Plane HTTP API (US — External
//! Control Plane and SDK, Phase 1: "API/spec fixtures").
//!
//! These structs are the external representation the spec doc requires:
//! "The external representation must be versioned and intentionally smaller
//! than the internal Rust struct." None of them derive `Serialize` directly
//! on `bastion_runtime::task::TaskCase` (or its nested types) — every field
//! below has a doc comment naming the exact Core field(s) it is a safe,
//! translated view of, so a reviewer (and `tests/control_plane_fixtures.rs`)
//! can catch drift if Core adds a field this contract should also expose or
//! deliberately continues to omit.
//!
//! Phase 1 note: these types are not yet returned by any live route — no
//! axum handler in this repo constructs or serializes them outside tests.
//! They exist now so the wire shape can be reviewed and frozen (see
//! `docs/en/contracts/control-plane-v1.openapi.yaml`) before any handler is
//! written against it in a later phase.

use serde::{Deserialize, Serialize};

/// Mirrors `bastion_runtime::task::ExecutionMode`. The Control Plane only
/// ever *creates* `Pursue` tasks ([`CreateTaskRequest`] has no `mode` field —
/// per the doc's own user story, this API exists specifically for "durable
/// Pursue tasks"), but a `GET` on a task created some other way (TUI, inbound
/// channel) could in principle report any of the three, so all three are
/// modeled here for read-path completeness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskMode {
    Respond,
    Act,
    Pursue,
}

/// Mirrors `bastion_runtime::task::TaskStatus` 1:1 (all 8 variants; no
/// narrowing — the doc's route table exposes `status` directly).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatusDto {
    Pending,
    Running,
    AwaitingApproval,
    Paused,
    Completed,
    Escalated,
    Cancelled,
    Failed,
}

/// A safe, external view of `bastion_runtime::task::StopReason`. Only
/// `Impossible`/`Escalated`'s host-authored `String` is exposed — never any
/// evidence/verdict detail (`StopReason` itself carries none, but
/// `TaskCase.attempts` does; see [`AttemptSummaryDto`] for how that's
/// excluded).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StopReasonDto {
    Completed,
    /// ← `StopReason::BudgetExceeded(BudgetKind)`. `dimension` is the
    /// lowercased `BudgetKind` variant name (`steps`, `wall_clock`, `tokens`,
    /// `money`, `parallelism`).
    BudgetExceeded {
        dimension: String,
    },
    Cancelled,
    AwaitingApproval,
    /// ← `StopReason::Impossible(String)`.
    Impossible {
        reason: String,
    },
    /// ← `StopReason::Escalated(String)`.
    Escalated {
        reason: String,
    },
}

/// ← `bastion_runtime::task::UsageAccum` + `bastion_runtime::task::Bounds`,
/// merged into one caller-facing summary (the doc's `budget_summary` field).
/// `cost_coverage` is carried through so a caller can tell an exact dollar
/// figure from an estimated one — dropping it would let a low-fidelity
/// number look more precise than it is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BudgetSummaryDto {
    /// ← `UsageAccum.llm_calls`.
    pub llm_calls: u32,
    /// ← `UsageAccum.steps`.
    pub steps: u32,
    /// ← `UsageAccum.input_tokens` + `output_tokens`.
    pub total_tokens: u64,
    /// ← `UsageAccum.cost_usd`.
    pub cost_usd: Option<f64>,
    /// ← `UsageAccum.cost_coverage` (`"reported" | "estimated" | "unknown"`).
    pub cost_coverage: String,
    /// ← `UsageAccum.wall_clock_ms`.
    pub wall_clock_ms: u64,
    /// ← `Bounds.max_cost_usd`. The declared limit `cost_usd` is checked
    /// against — `None` means unbounded.
    pub max_cost_usd: Option<f64>,
    /// ← `Bounds.max_steps`.
    pub max_steps: Option<u32>,
}

/// ← `bastion_runtime::task::Attempt`, summary-only. Deliberately excludes
/// `actions`/`belief_refs`/full `Verdict` detail — the spec's "Identity and
/// policy" section requires "Evidence defaults to metadata/safe summaries.
/// Full artifacts need an explicit allowed scope and a signed/expiring
/// retrieval route," which is not built this phase, so nothing beyond this
/// summary is exposed yet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttemptSummaryDto {
    /// ← `Attempt.id`.
    pub id: String,
    /// ← `Attempt.started_at`.
    pub started_at: i64,
    /// ← `Attempt.ended_at`.
    pub ended_at: Option<i64>,
    /// ← presence/absence of `Attempt.verdict`, plus its
    /// `VerificationStatus` when present — never the verdict's `detail` or
    /// `evidence` ids.
    pub verified: Option<AttemptVerificationDto>,
    /// ← `Attempt.usage`, same translation as [`BudgetSummaryDto`]'s
    /// usage-derived fields (bounds don't apply per-attempt).
    pub llm_calls: u32,
    pub total_tokens: u64,
    pub cost_usd: Option<f64>,
}

/// ← `bastion_runtime::task::VerificationStatus`, exposed without the
/// `Verdict`'s `provenance`/`detail`/`evidence` fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptVerificationDto {
    Unverified,
    Failed,
    Partial,
    Succeeded,
}

/// The `Task` resource from the spec doc's "Contract boundary" section,
/// field-for-field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskResource {
    /// ← `TaskCase.id`.
    pub id: String,
    /// ← `TaskCase.owner`. Always derived from the authenticated credential
    /// on writes — a caller-supplied value is never trusted (see the threat
    /// model doc); present here only as a read-path field.
    pub owner_id: String,
    /// Opaque caller id, unique per owner/integration, used for idempotent
    /// create. Not a Core field — Core has no `external_ref` concept; this is
    /// stored/looked-up entirely at the Control Plane layer in a later phase.
    pub external_ref: Option<String>,
    /// ← `TaskCase.mode`.
    pub mode: TaskMode,
    /// ← `TaskCase.frame.objective`.
    pub objective: String,
    /// ← `TaskCase.status`.
    pub status: TaskStatusDto,
    /// ← `TaskCase.stop_reason`.
    pub stop_reason: Option<StopReasonDto>,
    /// ← `TaskCase.created_at` (nanoseconds since epoch, per Core's
    /// convention — kept as-is rather than reformatted, so the DTO layer adds
    /// no lossy conversion).
    pub created_at: i64,
    /// ← `TaskCase.updated_at`.
    pub updated_at: i64,
    /// ← `TaskCase.revision`. Required by mutation endpoints in a later phase
    /// as the expected-revision optimistic-concurrency token.
    pub revision: u64,
    /// ← `TaskCase.usage` + `TaskCase.bounds`.
    pub budget_summary: BudgetSummaryDto,
    /// ← `TaskCase.attempts` (a `Vec<AttemptId>` in Core) resolved to
    /// summaries. Resolving the ids to full `Attempt` records is a later
    /// phase's route-handler job; this DTO only defines the target shape.
    pub attempts: Vec<AttemptSummaryDto>,
}

/// `POST /v1/tasks` request body. `mode` is deliberately absent — the
/// Control Plane only ever creates `Pursue` tasks (see [`TaskMode`]'s doc
/// comment); a caller cannot ask for `Respond`/`Act` through this endpoint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateTaskRequest {
    /// ← becomes `TaskCase.frame.objective`.
    pub objective: String,
    /// Opaque caller id for idempotent create (see [`TaskResource::external_ref`]).
    /// `#[serde(default)]`: serde does NOT treat a missing JSON key as `None`
    /// for an `Option<T>` field on its own — every field here that's
    /// optional on the wire needs the attribute explicitly, or a caller
    /// omitting it (the common case) gets a deserialize error instead of the
    /// intended default (caught by `tests/control_plane_routes.rs`'s
    /// `create_task_is_idempotent_on_owner_plus_idempotency_key`, which
    /// omits all three optional fields).
    #[serde(default)]
    pub external_ref: Option<String>,
    /// ← becomes `TaskCase.frame.acceptance` (each entry's `description`;
    /// Core's `AcceptanceCriterion.check` — a host-registered verifier name —
    /// is not caller-settable this phase).
    #[serde(default)]
    pub acceptance: Vec<String>,
    /// ← becomes `TaskCase.bounds`. `None` fields mean "use the deployment
    /// default," never "unbounded" by omission.
    #[serde(default)]
    pub bounds: Option<CreateTaskBoundsDto>,
}

/// Caller-supplied subset of `bastion_runtime::task::Bounds` accepted at
/// creation. Only the two budget dimensions the spec's `budget_summary`
/// surfaces back are settable this phase; `max_wall_clock_ms`/`max_tokens`/
/// `max_parallelism` remain deployment-controlled until a later phase widens
/// this DTO deliberately (not by accident).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateTaskBoundsDto {
    #[serde(default)]
    pub max_steps: Option<u32>,
    #[serde(default)]
    pub max_cost_usd: Option<f64>,
}

/// Body for `POST /v1/tasks/{id}:pause|:resume|:cancel` (US Phase 3). The
/// optimistic-concurrency guard — `bastion_runtime::task::TaskStore`'s
/// mutating methods take this exact `expected_revision: u64` shape directly
/// (`TaskStore::transition_status`/`update_case`), so this DTO is a 1:1 pass
/// through, not a translation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevisionGuardedRequest {
    pub expected_revision: u64,
}

/// Body for `POST /v1/tasks/{id}:steer` — `RevisionGuardedRequest`'s field
/// plus `guidance`. Flattened rather than composed (no nested
/// `revision_guard: {...}`) so the wire shape matches the OpenAPI fixture's
/// `allOf: [RevisionGuardedRequest, {guidance}]`, which is JSON-equivalent to
/// one flat object when the two schemas share no field names.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SteerRequest {
    pub expected_revision: u64,
    pub guidance: String,
}

/// `GET /v1/tasks` response envelope. Cursor-based pagination, frozen now
/// even though no route reads it yet (spec: "Pagination must be cursor
/// based").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskListResponse {
    pub items: Vec<TaskResource>,
    pub next_cursor: Option<String>,
}

/// `GET /v1/tasks/{id}/attempts` response envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttemptListResponse {
    pub items: Vec<AttemptSummaryDto>,
    pub next_cursor: Option<String>,
}

/// One consistent error shape for the whole `/v1/*` surface (spec: "Use
/// RFC 9457-style error envelopes"). `code` is a stable, machine-matchable
/// slug (e.g. `"stale_revision"`, `"scope_denied"`); `message` is
/// human-readable and MUST NOT embed request payload content (mirrors the
/// egress hook's payload-independence rule elsewhere in this codebase).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorEnvelope {
    pub code: String,
    pub message: String,
    /// Correlates a client-reported error with server-side logs/traces
    /// without exposing trace internals.
    pub request_id: String,
}

/// `POST /v1/webhook-subscriptions` request body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WebhookSubscriptionRequest {
    /// Subscriber-supplied callback URL. A later phase's route handler must
    /// validate this against SSRF (private/loopback ranges) before ever
    /// issuing a request to it — noted here as a contract requirement, not
    /// implemented by this DTO.
    pub target_url: String,
    /// Which of the 5 event types (see [`TaskEventEnvelope::event_type`])
    /// this subscription wants; empty means "all." `#[serde(default)]` for
    /// the same reason as `CreateTaskRequest`'s optional fields — `Vec<T>`
    /// is not exempt from serde's "every key must be present" default any
    /// more than `Option<T>` is.
    #[serde(default)]
    pub event_types: Vec<String>,
}

/// `POST /v1/webhook-subscriptions` response / list-item shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WebhookSubscriptionResource {
    pub id: String,
    pub owner_id: String,
    pub target_url: String,
    pub event_types: Vec<String>,
    pub created_at: i64,
    /// The HMAC signing secret (`webhook_delivery::sign_payload`'s key) —
    /// present ONLY in the response to the `POST` call that created this
    /// subscription; `None` everywhere else (a future list-subscriptions
    /// endpoint reusing this same DTO must never populate it). Mirrors
    /// [`super::credential::SqliteCredentialStore::issue`]'s
    /// plaintext-token-shown-once pattern — there is no way to retrieve a
    /// lost secret; the subscription must be re-created.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
}

/// One outbound event envelope, matching the spec's "Events" section 1:1:
/// `event_id`, schema version, owner-scoped task id, monotonic task
/// revision, timestamp, safe payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskEventEnvelope {
    pub event_id: String,
    /// One of: `"task.created"`, `"task.status_changed"`,
    /// `"attempt.completed"`, `"task.escalated"`, `"task.terminal"`.
    pub event_type: String,
    pub schema_version: u32,
    pub task_id: String,
    /// ← `TaskCase.revision` at the moment this event was raised — lets a
    /// receiver detect and discard/reorder stale deliveries.
    pub revision: u64,
    pub occurred_at: i64,
    /// The safe, event-type-specific summary. Deliberately typed as raw JSON
    /// here rather than an enum-per-event-type — the exact per-event payload
    /// shape is a later-phase decision; this envelope only freezes the
    /// fields every event shares.
    pub payload: serde_json::Value,
}
