//! Shared business logic for Control Plane task operations (US — External
//! Control Plane and SDK, Phase 5: "MCP alignment").
//!
//! Extracted out of `routes.rs` (Phases 2-4's HTTP handlers) so the HTTP
//! `/v1/*` surface and the MCP tool surface ([`super::mcp_tools`], feature
//! `mcp-server`) invoke the EXACT same task-store logic, event emission, and
//! error conditions — never two parallel implementations that could drift.
//! Each function here already assumes the caller (HTTP route or MCP tool
//! handler) has resolved an `owner` string through ITS OWN authentication
//! (Control Plane credentials for HTTP, `mcp::server::TokenPermissions` for
//! MCP) — this module has no opinion on how a caller proves who they are,
//! only on what happens once `owner` is known.
//!
//! [`CoreOpError`] is the one typed error vocabulary both surfaces branch on:
//! HTTP maps each variant to a specific `StatusCode` + `ErrorEnvelope.code`
//! (see `routes.rs`), MCP maps each to a distinguishable `CallToolResult`
//! text (see `mcp_tools.rs`) since `rmcp` has no structured error-code
//! channel today.

use std::sync::Arc;

use bastion_runtime::task::{
    AcceptanceCriterion, Bounds, CorrelationIds, ExecutionMode, Frame, Intent, IntentOrigin,
    OpaqueState, StopReason, TaskCase, TaskCaseId, TaskStatus, TaskStore, UsageAccum,
};

use super::business_state;
use super::dto::{AttemptListResponse, CreateTaskRequest, TaskListResponse, TaskResource};
use super::pagination::paginate;
use super::translate;
use super::webhook_delivery::{enqueue_event_for_subscribers, SqliteWebhookDeliveryStore};
use super::webhook_subscription::SqliteWebhookSubscriptionStore;

const DEFAULT_PAGE_SIZE: usize = 50;

/// State every core op needs — a strict subset of `routes::ControlPlaneState`
/// (no `credential_store`: authentication happens before these functions are
/// ever called, and is a per-surface concern these functions don't share).
#[derive(Clone)]
pub struct CoreOpsState {
    pub task_store: Arc<dyn TaskStore>,
    pub webhook_subscription_store: Arc<SqliteWebhookSubscriptionStore>,
    pub webhook_delivery_store: Arc<SqliteWebhookDeliveryStore>,
}

/// Typed outcome vocabulary shared by every core op. Deliberately carries no
/// human-readable message — each surface (HTTP status code + `ErrorEnvelope`,
/// MCP `CallToolResult` text) renders its own wording from the variant so
/// neither surface's phrasing leaks into the other's.
#[derive(Debug, Clone, PartialEq)]
pub enum CoreOpError {
    /// No task with that id is visible to this owner — never distinguishes
    /// "wrong owner" from "doesn't exist" (IDOR discipline, matches
    /// `credential::SqliteCredentialStore::revoke`'s existence check).
    NotFound,
    /// The task is already in a terminal status; carries that status so the
    /// caller can report it without a second lookup.
    Terminal(TaskStatus),
    /// The task's current status cannot transition to the requested target;
    /// carries the current status for the same reason as `Terminal`.
    InvalidTransition(TaskStatus),
    /// `expected_revision` did not match the task's current revision (OCC).
    StaleRevision,
    /// The store's own guarded write rejected the change — a genuine race in
    /// the gap between this op's read and its write (the local pre-checks
    /// above cannot see this one; the store call is the final authority).
    Conflict,
    /// Caller-supplied input failed a business-level validity check (e.g.
    /// empty objective/guidance/idempotency key) — carries a message safe to
    /// echo back verbatim (never derived from stored task content).
    InvalidInput(String),
    /// A store/dependency failure. The real error is already logged via
    /// `tracing::error!` at the point of failure; this variant intentionally
    /// carries nothing further so neither surface is tempted to leak
    /// internals to a caller.
    Internal,
}

impl std::fmt::Display for CoreOpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CoreOpError::NotFound => write!(f, "not found"),
            CoreOpError::Terminal(status) => write!(f, "task is already {status:?}"),
            CoreOpError::InvalidTransition(status) => {
                write!(f, "cannot transition a task in its current status ({status:?})")
            }
            CoreOpError::StaleRevision => {
                write!(f, "expected_revision does not match the task's current revision")
            }
            CoreOpError::Conflict => write!(f, "concurrent modification"),
            CoreOpError::InvalidInput(msg) => write!(f, "{msg}"),
            CoreOpError::Internal => write!(f, "internal error"),
        }
    }
}

fn now_nanos() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as i64
}

/// A request-correlation id — not a security token, just a grep handle.
/// Same "no UUID crate dependency" reasoning as `credential::uuid_like_id`.
fn uuid_like_id() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Derive a stable `TaskCaseId` from `(owner, idempotency_key)` — see
/// `routes.rs`'s former doc comment (now here) for why: `TaskStore::create_case`
/// is idempotent on its OWN `idempotency_key` parameter but that primitive
/// doesn't hand back the original case, and there is no "look up by
/// idempotency key" store method. Deriving the id itself sidesteps that.
fn deterministic_task_id(owner: &str, idempotency_key: &str) -> TaskCaseId {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(owner.as_bytes());
    hasher.update(b"\0");
    hasher.update(idempotency_key.as_bytes());
    let digest = hasher.finalize();
    let hex: String = digest.iter().take(12).map(|b| format!("{b:02x}")).collect();
    TaskCaseId(format!("cp_{hex}"))
}

/// Build a `TaskEventEnvelope` and hand it to `enqueue_event_for_subscribers`
/// — a fast, local DB write, never the outbound HTTP call itself (that only
/// ever happens later, out-of-band, in `webhook_delivery::run_delivery_loop`).
async fn emit_event(
    state: &CoreOpsState,
    owner: &str,
    event_type: &str,
    task_id: &str,
    revision: u64,
    payload: serde_json::Value,
) {
    let event = super::dto::TaskEventEnvelope {
        event_id: uuid_like_id(),
        event_type: event_type.to_string(),
        schema_version: 1,
        task_id: task_id.to_string(),
        revision,
        occurred_at: now_nanos(),
        payload,
    };
    enqueue_event_for_subscribers(
        &state.webhook_subscription_store,
        &state.webhook_delivery_store,
        owner,
        &event,
    )
    .await;
}

/// List `owner`'s tasks, optionally filtered by status, cursor-paginated.
/// `attempts` is always empty on every item (see `routes.rs`'s original doc
/// comment on `list_tasks` for why — avoids an N+1 fan-out).
pub async fn list_tasks(
    state: &CoreOpsState,
    owner: &str,
    status_filter: Option<&str>,
    cursor: Option<&str>,
) -> Result<TaskListResponse, CoreOpError> {
    let cases = state.task_store.list_cases_for_owner(owner).await.map_err(|e| {
        tracing::error!(event = "control_plane_tasks_list_failed", error = %e);
        CoreOpError::Internal
    })?;

    let filtered: Vec<_> = match status_filter {
        Some(status_filter) => cases
            .into_iter()
            .filter(|c| {
                let status_str = serde_json::to_value(translate::task_status(c.status))
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default();
                status_str.eq_ignore_ascii_case(status_filter)
            })
            .collect(),
        None => cases,
    };

    let (page, next_cursor) = paginate(
        filtered,
        |c| (c.created_at, c.id.0.as_str()),
        cursor,
        DEFAULT_PAGE_SIZE,
    );

    let items = page.iter().map(|case| translate::task_resource(case, vec![])).collect();
    Ok(TaskListResponse { items, next_cursor })
}

/// One task's safe summary, attempts included.
pub async fn get_task(state: &CoreOpsState, owner: &str, id: &str) -> Result<TaskResource, CoreOpError> {
    let case_id = TaskCaseId(id.to_string());
    let case = state
        .task_store
        .load_case(owner, &case_id)
        .await
        .map_err(|e| {
            tracing::error!(event = "control_plane_task_get_failed", error = %e);
            CoreOpError::Internal
        })?
        .ok_or(CoreOpError::NotFound)?;

    let attempts = state
        .task_store
        .list_attempts_for_case(owner, &case_id)
        .await
        .map_err(|e| {
            tracing::error!(event = "control_plane_task_get_attempts_failed", error = %e);
            CoreOpError::Internal
        })?;
    let attempt_dtos = attempts.iter().map(translate::attempt_summary).collect();

    Ok(translate::task_resource(&case, attempt_dtos))
}

/// Safe evidence/verdict timeline for one task. 404s if the task doesn't
/// exist *for this owner* (checked via `load_case` first, same IDOR
/// discipline as `get_task`).
pub async fn get_task_attempts(
    state: &CoreOpsState,
    owner: &str,
    id: &str,
    cursor: Option<&str>,
) -> Result<AttemptListResponse, CoreOpError> {
    let case_id = TaskCaseId(id.to_string());
    state
        .task_store
        .load_case(owner, &case_id)
        .await
        .map_err(|e| {
            tracing::error!(event = "control_plane_task_attempts_case_check_failed", error = %e);
            CoreOpError::Internal
        })?
        .ok_or(CoreOpError::NotFound)?;

    let attempts = state
        .task_store
        .list_attempts_for_case(owner, &case_id)
        .await
        .map_err(|e| {
            tracing::error!(event = "control_plane_task_attempts_list_failed", error = %e);
            CoreOpError::Internal
        })?;

    let (page, next_cursor) = paginate(
        attempts,
        |a: &bastion_runtime::task::Attempt| (a.started_at, a.id.0.as_str()),
        cursor,
        DEFAULT_PAGE_SIZE,
    );
    let items = page.iter().map(translate::attempt_summary).collect();
    Ok(AttemptListResponse { items, next_cursor })
}

/// Outcome of [`create_task`] — `created == false` means an earlier call with
/// the same `(owner, idempotency_key)` already created this task and this
/// call is a pure idempotent replay (HTTP maps this to 200 vs 201; MCP can
/// report it in its result text).
pub struct CreateTaskOutcome {
    pub resource: TaskResource,
    pub created: bool,
}

/// Create (or idempotently return) a durable `Pursue` task. Builds the
/// `TaskCase` directly here rather than calling `adaptive::enqueue_pursue`
/// (`src/adaptive/enqueue.rs`) — that function hardcodes empty
/// `acceptance`/default `Bounds` and generates its own id, so it cannot honor
/// a caller-supplied `acceptance`/`bounds` or this op's idempotency-key
/// derived id. This does NOT violate `docs/en/control-plane-security.md`'s
/// "must call the same execution path" invariant — that invariant is about
/// the CapabilityRegistry/egress-gate/approval-queue machinery a task's
/// ACTIONS run through once adapting, which neither `enqueue_pursue` nor this
/// touches at creation time (both are pure `TaskCase` construction +
/// `store.create_case`).
pub async fn create_task(
    state: &CoreOpsState,
    owner: &str,
    idempotency_key: &str,
    req: CreateTaskRequest,
) -> Result<CreateTaskOutcome, CoreOpError> {
    if idempotency_key.trim().is_empty() {
        return Err(CoreOpError::InvalidInput(
            "idempotency_key must not be empty".to_string(),
        ));
    }
    if req.objective.trim().is_empty() {
        return Err(CoreOpError::InvalidInput("objective must not be empty".to_string()));
    }

    let task_id = deterministic_task_id(owner, idempotency_key);

    // Idempotent replay: a case at this derived id already existing means an
    // earlier call with the same owner+key already created it — return that,
    // ignoring this call's body entirely (the idempotency-key contract is
    // "same key -> same result", not "merge the two request bodies").
    if let Some(existing) = state.task_store.load_case(owner, &task_id).await.map_err(|e| {
        tracing::error!(event = "control_plane_task_create_lookup_failed", error = %e);
        CoreOpError::Internal
    })? {
        return Ok(CreateTaskOutcome {
            resource: translate::task_resource(&existing, vec![]),
            created: false,
        });
    }

    let now = now_nanos();
    let bounds = req
        .bounds
        .as_ref()
        .map(|b| Bounds {
            max_steps: b.max_steps,
            max_wall_clock_ms: None,
            max_tokens: None,
            max_cost_usd: b.max_cost_usd,
            max_parallelism: None,
        })
        .unwrap_or_default();

    let case = TaskCase {
        id: task_id.clone(),
        owner: owner.to_string(),
        mode: ExecutionMode::Pursue,
        intent: Intent {
            owner: owner.to_string(),
            mode: ExecutionMode::Pursue,
            summary: req.objective.clone(),
            // No IntentOrigin variant fits "external API/tool call" precisely
            // (Message/Event/Schedule) — Event is the closest ("something
            // outside the conversation triggered this").
            origin: IntentOrigin::Event,
        },
        frame: Frame {
            objective: req.objective.clone(),
            acceptance: req
                .acceptance
                .iter()
                .map(|description| AcceptanceCriterion {
                    description: description.clone(),
                    check: None,
                })
                .collect(),
            context_refs: vec![],
        },
        bounds,
        status: TaskStatus::Pending,
        stop_reason: None,
        attempts: vec![],
        pending_approvals: vec![],
        next_decision: None,
        usage: UsageAccum::default(),
        parent: None,
        correlation: CorrelationIds::default(),
        business_state: OpaqueState(business_state::new_business_state(req.external_ref.as_deref())),
        created_at: now,
        updated_at: now,
        revision: 1,
    };

    // Pass `task_id` itself as Core's idempotency_key, NOT the raw caller
    // key: `SqliteTaskStore`'s unique index on idempotency_key is GLOBAL, not
    // owner-scoped, so two different owners submitting the same literal key
    // would otherwise collide at Core's storage layer. `task_id` is already
    // unique per (owner, idempotency_key) — reusing it here matches
    // `adaptive::enqueue_pursue`'s own convention of passing its generated id
    // as the idempotency key.
    state.task_store.create_case(&case, &task_id.0).await.map_err(|e| {
        tracing::error!(event = "control_plane_task_create_failed", error = %e);
        CoreOpError::Internal
    })?;

    // Re-fetch rather than echo the in-memory `case`: closes the rare TOCTOU
    // race where a concurrent call with the same idempotency key won
    // `create_case` first — the stored row, not our local guess, is always
    // the source of truth.
    let stored = state
        .task_store
        .load_case(owner, &task_id)
        .await
        .map_err(|e| {
            tracing::error!(event = "control_plane_task_create_refetch_failed", error = %e);
            CoreOpError::Internal
        })?
        .ok_or_else(|| {
            tracing::error!(event = "control_plane_task_create_vanished", task_id = %task_id);
            CoreOpError::Internal
        })?;

    tracing::info!(
        event = "control_plane_task_created",
        owner = %owner,
        task_id = %stored.id,
    );

    emit_event(
        state,
        owner,
        "task.created",
        &stored.id.0,
        stored.revision,
        serde_json::json!({
            "status": translate::task_status(stored.status),
            "objective": stored.frame.objective,
        }),
    )
    .await;

    Ok(CreateTaskOutcome {
        resource: translate::task_resource(&stored, vec![]),
        created: true,
    })
}

/// Shared implementation for pause/resume/cancel. Pre-checks terminality,
/// the state-machine transition, and the revision guard LOCALLY (all three
/// are things `load_case` already gave us enough to decide) so the
/// common-case error is specific; the store call itself is the final,
/// race-safe authority (`TaskStore::transition_status` is itself
/// owner+revision-guarded) and its failure collapses to `Conflict` (a
/// genuine concurrent-modification race in the gap between our read and this
/// write, which the local pre-checks cannot see).
pub async fn transition_task(
    state: &CoreOpsState,
    owner: &str,
    id: &str,
    target: TaskStatus,
    stop_reason: Option<StopReason>,
    expected_revision: u64,
    verb: &str,
) -> Result<TaskResource, CoreOpError> {
    let case_id = TaskCaseId(id.to_string());
    let case = state
        .task_store
        .load_case(owner, &case_id)
        .await
        .map_err(|e| {
            tracing::error!(event = "control_plane_task_action_load_failed", verb, error = %e);
            CoreOpError::Internal
        })?
        .ok_or(CoreOpError::NotFound)?;

    if case.status.is_terminal() {
        return Err(CoreOpError::Terminal(case.status));
    }
    if !case.status.can_transition_to(target) {
        return Err(CoreOpError::InvalidTransition(case.status));
    }
    if case.revision != expected_revision {
        return Err(CoreOpError::StaleRevision);
    }

    let new_revision = state
        .task_store
        .transition_status(owner, &case_id, target, stop_reason.clone(), expected_revision)
        .await
        .map_err(|e| {
            tracing::warn!(event = "control_plane_task_action_conflict", verb, error = %e);
            CoreOpError::Conflict
        })?;

    tracing::info!(
        event = "control_plane_task_transitioned",
        owner = %owner,
        task_id = %case_id,
        verb,
        new_revision,
    );

    let mut updated = case;
    updated.status = target;
    updated.stop_reason = stop_reason;
    updated.revision = new_revision;
    updated.updated_at = now_nanos();

    emit_event(
        state,
        owner,
        "task.status_changed",
        &case_id.0,
        new_revision,
        serde_json::json!({ "status": translate::task_status(updated.status), "verb": verb }),
    )
    .await;
    // Distinct from task.status_changed per the spec's own event list (a
    // subscriber may want "task finished" without every intermediate status
    // noise) — emitted in ADDITION when the new status is terminal.
    if updated.status.is_terminal() {
        emit_event(
            state,
            owner,
            "task.terminal",
            &case_id.0,
            new_revision,
            serde_json::json!({
                "status": translate::task_status(updated.status),
                "stop_reason": updated.stop_reason.as_ref().map(translate::stop_reason),
            }),
        )
        .await;
    }

    Ok(translate::task_resource(&updated, vec![]))
}

/// Steer a running task — mirrors `agent::task_command::steer`'s
/// append-to-business_state logic exactly (via
/// [`business_state::append_steer_note`]) so a TUI/chat steer and an
/// API/MCP steer interleave safely on the same task, but threads the
/// CALLER's `expected_revision` through to `update_case` instead of a
/// freshly-read one — `task_command::steer` always re-reads, which cannot
/// enforce an external caller's OCC contract ("stale value yields a
/// conflict"). No event is emitted here: `guidance` steering has no
/// corresponding entry in the spec's 5 event types (task.created,
/// task.status_changed, task.terminal, attempt.completed, task.escalated).
pub async fn steer_task(
    state: &CoreOpsState,
    owner: &str,
    id: &str,
    guidance: &str,
    expected_revision: u64,
) -> Result<TaskResource, CoreOpError> {
    if guidance.trim().is_empty() {
        return Err(CoreOpError::InvalidInput("guidance must not be empty".to_string()));
    }

    let case_id = TaskCaseId(id.to_string());
    let mut case = state
        .task_store
        .load_case(owner, &case_id)
        .await
        .map_err(|e| {
            tracing::error!(event = "control_plane_task_steer_load_failed", error = %e);
            CoreOpError::Internal
        })?
        .ok_or(CoreOpError::NotFound)?;

    if case.status.is_terminal() {
        return Err(CoreOpError::Terminal(case.status));
    }
    if case.revision != expected_revision {
        return Err(CoreOpError::StaleRevision);
    }

    case.business_state.0 = business_state::append_steer_note(case.business_state.0.clone(), guidance);
    case.updated_at = now_nanos();

    let new_revision = state
        .task_store
        .update_case(&case, expected_revision)
        .await
        .map_err(|e| {
            tracing::warn!(event = "control_plane_task_steer_conflict", error = %e);
            CoreOpError::Conflict
        })?;

    tracing::info!(
        event = "control_plane_task_steered",
        owner = %owner,
        task_id = %case_id,
        new_revision,
    );

    case.revision = new_revision;
    Ok(translate::task_resource(&case, vec![]))
}
