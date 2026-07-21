//! Live `/v1/*` Control Plane routes (US — External Control Plane and SDK).
//! Phase 2 shipped the read-only routes and OpenAPI publication; Phase 3
//! added `POST /v1/tasks` (idempotent create) and
//! `POST /v1/tasks/{id}:pause|:resume|:cancel|:steer` (OCC-guarded
//! mutations). See the "colon action routes" note below for how the
//! `:action` suffix is actually matched — `axum`'s `matchit` router cannot
//! capture a partial path segment, so these do not register as literal
//! `{id}:pause`-style patterns.
//!
//! Deliberately built as a **self-contained, separately-stated** axum router
//! (`ControlPlaneState`, not `channel::webhook::AppState`) merged into the
//! main app via `Router::merge` — the exact pattern `serve_with_mesh` already
//! uses for `mcp_routes: Option<axum::Router>` (`channel/webhook.rs`). This
//! keeps every existing webhook route, its `AppState`, and its test helpers
//! completely untouched; adding a new bounded context here costs one new
//! optional parameter at the `serve_with_mesh` call site, not five edited
//! `AppState` literals.
//!
//! Auth is `x-bastion-token`, matching every other authenticated surface in
//! this codebase (`channel/webhook.rs`'s `resolve_owner_or_401`,
//! `mcp/server.rs`'s `authenticate_token`) — this resolves the "which header"
//! open decision from `docs/en/control-plane-security.md`'s Phase 1 draft.
//! The token is looked up against [`super::credential::SqliteCredentialStore`]
//! (Control Plane credentials), never `channel::OwnerMap` — the two
//! credential spaces are deliberately distinct (Phase 1's "Identity and
//! policy" design).
//!
//! Phase 5: every handler below is now a THIN transport-mapping layer —
//! header/body parsing in, [`super::core_ops`] call, `CoreOpError` ->
//! `StatusCode`/`ErrorEnvelope` out. The actual task-store logic lives in
//! `core_ops.rs`, shared with the MCP tool surface ([`super::mcp_tools`]) so
//! the two never drift (see that module's doc comment).

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use bastion_runtime::task::{StopReason, TaskStatus, TaskStore};

use super::core_ops::{self, CoreOpError, CoreOpsState};
use super::credential::{AuthenticatedCredential, SqliteCredentialStore};
use super::dto::{
    CreateTaskRequest, ErrorEnvelope, RevisionGuardedRequest, SteerRequest,
    WebhookSubscriptionRequest, WebhookSubscriptionResource,
};
use super::scope::{require_scope, Scope};
use super::webhook_delivery::SqliteWebhookDeliveryStore;
use super::webhook_subscription::SqliteWebhookSubscriptionStore;

/// The OpenAPI fixture, embedded at compile time — "publication" for Phase 2
/// means serving this frozen contract at a discoverable URL, not regenerating
/// it from the DTOs at runtime (the fixture IS the frozen source of truth;
/// `tests/control_plane_fixtures.rs` is what keeps `dto.rs` honest against it).
const OPENAPI_YAML: &str = include_str!("../../docs/en/contracts/control-plane-v1.openapi.yaml");

/// State for the `/v1/*` router, separate from `channel::webhook::AppState`
/// by design (see module doc).
#[derive(Clone)]
pub struct ControlPlaneState {
    pub task_store: Arc<dyn TaskStore>,
    pub credential_store: Arc<SqliteCredentialStore>,
    pub webhook_subscription_store: Arc<SqliteWebhookSubscriptionStore>,
    pub webhook_delivery_store: Arc<SqliteWebhookDeliveryStore>,
}

impl ControlPlaneState {
    /// The `core_ops` slice of this state — same three stores, minus
    /// `credential_store` (an HTTP-transport-only concern `core_ops` has no
    /// opinion on). Cheap: three `Arc` clones, no I/O.
    fn core(&self) -> CoreOpsState {
        CoreOpsState {
            task_store: self.task_store.clone(),
            webhook_subscription_store: self.webhook_subscription_store.clone(),
            webhook_delivery_store: self.webhook_delivery_store.clone(),
        }
    }
}

/// Build the `/v1/*` router. Returns a fully state-erased `Router` (state
/// applied via `.with_state`), ready to `.merge()` into the main app — same
/// shape as the `mcp_routes: Option<axum::Router>` parameter it sits next to.
///
/// ## Colon action routes
/// The spec's `POST /tasks/{id}:pause` (etc.) paths use a literal `:` inside
/// one path segment (a Google API-style "custom method"). `axum` 0.8's
/// router is built on `matchit`, which only supports a capture (`{id}`)
/// spanning an ENTIRE segment — it cannot capture `{id}` and match literal
/// `:pause` within the same segment. So `POST /v1/tasks/{id}` is registered
/// on the SAME route entry as `GET /v1/tasks/{id}` (method-dispatched, same
/// `{id}` param), and the POST handler ([`task_action`]) manually splits the
/// captured segment on its LAST `:` to recover `(id, action)` — the URL a
/// client sends (`/v1/tasks/abc123:pause`) is unchanged; only how this
/// router internally matches it differs. See
/// `docs/en/control-plane-security.md`'s Phase 2 design note (where this was
/// first flagged) for the alternative considered and rejected
/// (`/tasks/{id}/pause`, which would break the frozen contract's paths).
pub fn router(state: ControlPlaneState) -> Router {
    Router::new()
        .route("/v1/tasks", get(list_tasks).post(create_task))
        .route("/v1/tasks/{id}", get(get_task).post(task_action))
        .route("/v1/tasks/{id}/attempts", get(get_task_attempts))
        .route(
            "/v1/webhook-subscriptions",
            post(create_webhook_subscription),
        )
        .route("/v1/openapi.yaml", get(get_openapi_spec))
        .with_state(state)
}

fn error_response(status: StatusCode, code: &str, message: &str) -> axum::response::Response {
    (
        status,
        Json(ErrorEnvelope {
            code: code.to_string(),
            message: message.to_string(),
            request_id: uuid_like_request_id(),
        }),
    )
        .into_response()
}

/// A request-correlation id for `ErrorEnvelope.request_id` — not a security
/// token, just a grep handle between a client-reported error and the daemon
/// log. Same "no UUID crate dependency" reasoning as
/// `credential::uuid_like_id`.
fn uuid_like_request_id() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Map a [`CoreOpError`] to this surface's `StatusCode` + `ErrorEnvelope`.
/// The one place HTTP renders `core_ops`'s typed vocabulary into wire form —
/// mirrors [`super::mcp_tools`]'s equivalent (but MCP-shaped) mapping.
fn core_error_response(err: CoreOpError, verb: &str) -> axum::response::Response {
    match err {
        CoreOpError::NotFound => error_response(
            StatusCode::NOT_FOUND,
            "not_found",
            "no task with that id is visible to this credential's owner",
        ),
        CoreOpError::Terminal(status) => error_response(
            StatusCode::CONFLICT,
            "task_terminal",
            &format!("task is already {status:?}; cannot {verb}"),
        ),
        CoreOpError::InvalidTransition(status) => error_response(
            StatusCode::CONFLICT,
            "invalid_transition",
            &format!("cannot {verb} a task in its current status ({status:?})"),
        ),
        CoreOpError::StaleRevision => error_response(
            StatusCode::CONFLICT,
            "stale_revision",
            "expected_revision does not match the task's current revision",
        ),
        CoreOpError::Conflict => error_response(
            StatusCode::CONFLICT,
            "conflict",
            &format!("could not {verb} task: concurrent modification"),
        ),
        CoreOpError::InvalidInput(msg) => {
            error_response(StatusCode::BAD_REQUEST, "invalid_body", &msg)
        }
        CoreOpError::Internal => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "internal error",
        ),
    }
}

/// Resolve `x-bastion-token` against the Control Plane credential store.
/// Mirrors `channel::webhook::resolve_owner_or_401`'s shape/logging
/// discipline exactly, but against a different credential space (see module
/// doc) and returning an `ErrorEnvelope` body instead of `{}`.
async fn resolve_credential_or_401(
    headers: &axum::http::HeaderMap,
    credential_store: &SqliteCredentialStore,
    event_name: &'static str,
) -> Result<AuthenticatedCredential, Box<axum::response::Response>> {
    let token = headers
        .get("x-bastion-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    match credential_store.authenticate(token).await {
        Ok(Some(cred)) => Ok(cred),
        Ok(None) => {
            tracing::warn!(event = event_name, "unknown or missing x-bastion-token");
            Err(Box::new(error_response(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "missing or unknown credential",
            )))
        }
        Err(e) => {
            // Store failure (e.g. sqlite unavailable) is an operational
            // problem, not the caller's fault — 401 would be misleading.
            tracing::error!(event = event_name, error = %e, "credential store lookup failed");
            Err(Box::new(error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "internal error",
            )))
        }
    }
}

// clippy::result_large_err (WR-08 precedent: channel/webhook.rs's
// resolve_owner_or_401 boxes its Err for the same reason) — Response is
// 128+ bytes; the Ok path (a small credential struct) is the common case.
fn require_scope_or_403(
    cred: &AuthenticatedCredential,
    scope: Scope,
) -> Result<(), Box<axum::response::Response>> {
    require_scope(&cred.scopes, scope).map_err(|_| {
        Box::new(error_response(
            StatusCode::FORBIDDEN,
            "scope_denied",
            "credential authenticated but lacks the required scope",
        ))
    })
}

#[derive(serde::Deserialize)]
struct ListTasksQuery {
    cursor: Option<String>,
    status: Option<String>,
}

/// `GET /v1/tasks` — list the authenticated credential's owner's tasks.
async fn list_tasks(
    State(state): State<ControlPlaneState>,
    headers: axum::http::HeaderMap,
    Query(q): Query<ListTasksQuery>,
) -> axum::response::Response {
    let cred =
        match resolve_credential_or_401(&headers, &state.credential_store, "v1_tasks_unauthorized")
            .await
        {
            Ok(c) => c,
            Err(resp) => return *resp,
        };
    if let Err(resp) = require_scope_or_403(&cred, Scope::TasksRead) {
        return *resp;
    }

    match core_ops::list_tasks(
        &state.core(),
        &cred.owner_id,
        q.status.as_deref(),
        q.cursor.as_deref(),
    )
    .await
    {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => core_error_response(e, "list"),
    }
}

/// `GET /v1/tasks/{id}` — one task's safe summary, attempts included.
async fn get_task(
    State(state): State<ControlPlaneState>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
) -> axum::response::Response {
    let cred = match resolve_credential_or_401(
        &headers,
        &state.credential_store,
        "v1_task_get_unauthorized",
    )
    .await
    {
        Ok(c) => c,
        Err(resp) => return *resp,
    };
    if let Err(resp) = require_scope_or_403(&cred, Scope::TasksRead) {
        return *resp;
    }

    match core_ops::get_task(&state.core(), &cred.owner_id, &id).await {
        Ok(resource) => Json(resource).into_response(),
        Err(e) => core_error_response(e, "get"),
    }
}

#[derive(serde::Deserialize)]
struct ListAttemptsQuery {
    cursor: Option<String>,
}

/// `GET /v1/tasks/{id}/attempts` — safe evidence/verdict timeline.
async fn get_task_attempts(
    State(state): State<ControlPlaneState>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<ListAttemptsQuery>,
) -> axum::response::Response {
    let cred = match resolve_credential_or_401(
        &headers,
        &state.credential_store,
        "v1_task_attempts_unauthorized",
    )
    .await
    {
        Ok(c) => c,
        Err(resp) => return *resp,
    };
    if let Err(resp) = require_scope_or_403(&cred, Scope::TasksRead) {
        return *resp;
    }

    match core_ops::get_task_attempts(&state.core(), &cred.owner_id, &id, q.cursor.as_deref()).await
    {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => core_error_response(e, "list attempts for"),
    }
}

/// `GET /v1/openapi.yaml` — publishes the frozen contract fixture.
/// Deliberately unauthenticated: an API's own schema being public (like
/// Swagger UI / most public OpenAPI docs) is the norm, and the document
/// contains no secret material — only shapes and route descriptions.
async fn get_openapi_spec() -> axum::response::Response {
    (
        StatusCode::OK,
        [("content-type", "application/yaml")],
        OPENAPI_YAML,
    )
        .into_response()
}

/// `POST /v1/webhook-subscriptions` — register a signed event target.
/// `target_url` is SSRF-validated by
/// `SqliteWebhookSubscriptionStore::issue` (see that module's doc comment
/// for exactly when/how) — a loopback/private/link-local/reserved address or
/// non-http(s) scheme is rejected here, before anything is persisted.
async fn create_webhook_subscription(
    State(state): State<ControlPlaneState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> axum::response::Response {
    let cred = match resolve_credential_or_401(
        &headers,
        &state.credential_store,
        "v1_webhook_subscription_unauthorized",
    )
    .await
    {
        Ok(c) => c,
        Err(resp) => return *resp,
    };
    if let Err(resp) = require_scope_or_403(&cred, Scope::WebhooksManage) {
        return *resp;
    }

    let req: WebhookSubscriptionRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_body",
                &format!("invalid request body: {e}"),
            )
        }
    };

    match state
        .webhook_subscription_store
        .issue(&cred.owner_id, &req.target_url, req.event_types.clone())
        .await
    {
        Ok((id, secret)) => {
            tracing::info!(
                event = "control_plane_webhook_subscription_created",
                owner = %cred.owner_id,
                subscription_id = %id,
                credential_id = %cred.credential_id,
            );
            // `secret` is returned exactly once, here — WebhookSubscriptionResource.secret
            // is `#[serde(skip_serializing_if = "Option::is_none")]`, so this
            // is the only response shape that will ever carry it (a future
            // list-subscriptions endpoint must construct the DTO with
            // `secret: None`).
            (
                StatusCode::CREATED,
                Json(WebhookSubscriptionResource {
                    id,
                    owner_id: cred.owner_id.clone(),
                    target_url: req.target_url,
                    event_types: req.event_types,
                    created_at: now_nanos(),
                    secret: Some(secret),
                }),
            )
                .into_response()
        }
        Err(e) => {
            tracing::warn!(event = "v1_webhook_subscription_create_failed", error = %e);
            error_response(
                StatusCode::BAD_REQUEST,
                "invalid_target_url",
                "target_url failed validation (must be a public http(s) URL)",
            )
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

/// `POST /v1/tasks` — create (or idempotently return) a durable `Pursue`
/// task. Requires `Idempotency-Key` (spec: "Every mutation requires an
/// idempotency key"). Header extraction/absence is an HTTP-transport
/// concern handled here; emptiness is re-validated inside
/// `core_ops::create_task` regardless (the MCP surface has no header to
/// extract from, so that check must live in the shared function too).
async fn create_task(
    State(state): State<ControlPlaneState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> axum::response::Response {
    let cred = match resolve_credential_or_401(
        &headers,
        &state.credential_store,
        "v1_task_create_unauthorized",
    )
    .await
    {
        Ok(c) => c,
        Err(resp) => return *resp,
    };
    if let Err(resp) = require_scope_or_403(&cred, Scope::TasksCreate) {
        return *resp;
    }

    let idempotency_key = match headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
    {
        Some(k) if !k.is_empty() => k.to_string(),
        _ => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "missing_idempotency_key",
                "the Idempotency-Key header is required",
            )
        }
    };

    let req: CreateTaskRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_body",
                &format!("invalid request body: {e}"),
            )
        }
    };

    match core_ops::create_task(&state.core(), &cred.owner_id, &idempotency_key, req).await {
        Ok(outcome) => {
            let status = if outcome.created {
                StatusCode::CREATED
            } else {
                StatusCode::OK
            };
            (status, Json(outcome.resource)).into_response()
        }
        Err(e) => core_error_response(e, "create"),
    }
}

/// `POST /v1/tasks/{id}` dispatch target for the `:pause|:resume|:cancel|:steer`
/// actions — see [`router`]'s doc comment for why this single route entry
/// handles all four rather than four separately-registered paths.
async fn task_action(
    State(state): State<ControlPlaneState>,
    headers: axum::http::HeaderMap,
    Path(id_action): Path<String>,
    body: axum::body::Bytes,
) -> axum::response::Response {
    let Some((id, action)) = id_action.rsplit_once(':') else {
        return error_response(StatusCode::NOT_FOUND, "not_found", "unknown route");
    };
    if id.is_empty() {
        return error_response(StatusCode::NOT_FOUND, "not_found", "unknown route");
    }

    let cred = match resolve_credential_or_401(
        &headers,
        &state.credential_store,
        "v1_task_action_unauthorized",
    )
    .await
    {
        Ok(c) => c,
        Err(resp) => return *resp,
    };
    if let Err(resp) = require_scope_or_403(&cred, Scope::TasksControl) {
        return *resp;
    }

    match action {
        "pause" => {
            transition_action(&state, &cred, id, &body, TaskStatus::Paused, None, "pause").await
        }
        "resume" => {
            transition_action(
                &state,
                &cred,
                id,
                &body,
                TaskStatus::Running,
                None,
                "resume",
            )
            .await
        }
        "cancel" => {
            transition_action(
                &state,
                &cred,
                id,
                &body,
                TaskStatus::Cancelled,
                Some(StopReason::Cancelled),
                "cancel",
            )
            .await
        }
        "steer" => steer_action(&state, &cred, id, &body).await,
        _ => error_response(StatusCode::NOT_FOUND, "not_found", "unknown task action"),
    }
}

async fn transition_action(
    state: &ControlPlaneState,
    cred: &AuthenticatedCredential,
    id: &str,
    body: &[u8],
    target: TaskStatus,
    stop_reason: Option<StopReason>,
    verb: &str,
) -> axum::response::Response {
    let req: RevisionGuardedRequest = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_body",
                &format!("invalid request body: {e}"),
            )
        }
    };

    match core_ops::transition_task(
        &state.core(),
        &cred.owner_id,
        id,
        target,
        stop_reason,
        req.expected_revision,
        verb,
    )
    .await
    {
        Ok(resource) => Json(resource).into_response(),
        Err(e) => core_error_response(e, verb),
    }
}

async fn steer_action(
    state: &ControlPlaneState,
    cred: &AuthenticatedCredential,
    id: &str,
    body: &[u8],
) -> axum::response::Response {
    let req: SteerRequest = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_body",
                &format!("invalid request body: {e}"),
            )
        }
    };

    match core_ops::steer_task(
        &state.core(),
        &cred.owner_id,
        id,
        &req.guidance,
        req.expected_revision,
    )
    .await
    {
        Ok(resource) => Json(resource).into_response(),
        Err(e) => core_error_response(e, "steer"),
    }
}
