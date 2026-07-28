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

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use bastion_runtime::task::{StopReason, TaskStatus, TaskStore};
use rand::RngCore;
use tokio::sync::Mutex;

use super::core_ops::{self, CoreOpError, CoreOpsState};
use super::credential::{AuthenticatedCredential, SqliteCredentialStore};
use super::dto::{
    CreateTaskRequest, CredentialIssueRequest, CredentialIssueResponse, CredentialRetrieveRequest,
    CredentialRetrieveResponse, ErrorEnvelope, RevisionGuardedRequest, SteerRequest,
    WebhookSubscriptionListResponse, WebhookSubscriptionRequest, WebhookSubscriptionResource,
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
/// `rate_limiter`: caller-supplied so `main.rs` can construct ONE instance
/// and share it with `mcp::server::BastionMcpServer` too — the 5 Control
/// Plane MCP tools get rate-limited the same way. Sharing is correct but not
/// load-bearing today: HTTP `/v1/*` credentials (`SqliteCredentialStore`,
/// `bcp_*` tokens) and MCP's own `TokenPermissions` (static config) are
/// separate token spaces with no overlapping values, so one instance vs. two
/// behaves identically either way — sharing is simpler, not a fix for a
/// real double-budget scenario in the CURRENT architecture.
///
/// `remote_credential_issuance`: `Some(auth)` mounts
/// `POST /v1/credentials` gated by that operator token (see
/// [`credentials_router`]); `None` — the default — does not mount it at all,
/// so issuance stays console-only exactly as before.
pub fn router(
    state: ControlPlaneState,
    rate_limiter: super::rate_limit::RateLimiter,
    remote_credential_issuance: Option<crate::channel::operational::DaemonAccessAuth>,
) -> Router {
    let app = Router::new()
        .route("/v1/tasks", get(list_tasks).post(create_task))
        .route("/v1/tasks/{id}", get(get_task).post(task_action))
        .route("/v1/tasks/{id}/attempts", get(get_task_attempts))
        .route(
            "/v1/webhook-subscriptions",
            get(list_webhook_subscriptions).post(create_webhook_subscription),
        )
        .route(
            "/v1/webhook-subscriptions/{id}",
            axum::routing::delete(revoke_webhook_subscription),
        )
        .route("/v1/openapi.yaml", get(get_openapi_spec))
        // Applied to every /v1/* route, before the ControlPlaneState below —
        // its own state (RateLimiter) is independent, see rate_limit.rs.
        .layer(axum::middleware::from_fn_with_state(
            rate_limiter.clone(),
            super::rate_limit::enforce,
        ))
        .with_state(state.clone());
    match remote_credential_issuance {
        Some(auth) => app.merge(credentials_router(
            state.credential_store.clone(),
            auth,
            rate_limiter,
        )),
        None => app,
    }
}

/// State for the one remotely-issuable-credential route. Separate from
/// [`ControlPlaneState`] because its authority is different in kind: every
/// other `/v1/*` route authenticates a Control Plane credential
/// (`x-bastion-token`), while this one authenticates the OPERATOR
/// (`Authorization: Bearer $BASTION_DAEMON_TOKEN`, the same fail-closed check
/// `/lifecycle/*` uses).
///
/// That asymmetry is the point. If a Control Plane credential could mint
/// credentials, any leaked integration token would be able to mint itself a
/// wider one and a fresh one after revocation — a privilege-escalation and
/// persistence path. Minting authority therefore stays with the operator
/// secret, and this route only changes WHERE the operator can be (a remote
/// deployment they hold the token for) rather than WHAT can mint.
#[derive(Clone)]
struct CredentialIssuanceState {
    credential_store: Arc<SqliteCredentialStore>,
    auth: crate::channel::operational::DaemonAccessAuth,
    pending_tokens: PendingIssuedTokens,
}

/// How long a `retrieval_ref` stays redeemable, in nanoseconds — short
/// enough that a ref sitting in a log/proxy/monitoring snapshot has a narrow
/// useful window; the operator's own tooling is expected to issue and
/// retrieve in the same breath, not stash the ref for later.
const RETRIEVAL_TTL_NANOS: i64 = 300 * 1_000_000_000;

/// Prefix on a retrieval reference — deliberately distinct from
/// [`super::credential`]'s `bcp_` token prefix so the two are never confused
/// at a glance (a leaked reference and a leaked bearer token are both
/// sensitive, but only one of them authenticates by itself here).
const RETRIEVAL_REF_PREFIX: &str = "bcpr_";

/// One-time-recovery holding pen for a freshly issued Control Plane token,
/// keyed by an unguessable retrieval reference. Deliberately NOT
/// `proposals::PendingSecretValues`'s pattern verbatim: that module's keys
/// (`rand_u64`) are documented as "uniqueness, not secrecy" — fine for a
/// proposal id, wrong for something that stands in for a bearer token. The
/// reference here uses the same CSPRNG-plus-base64 construction
/// [`super::credential::generate_token`] uses for the token itself.
///
/// In-memory only, same discipline as `PendingSecretValues`: a daemon
/// restart empties it by construction, so a ref that outlives the process
/// simply stops working rather than resolving against stale state.
#[derive(Clone, Default)]
struct PendingIssuedTokens {
    inner: Arc<Mutex<HashMap<String, (String, i64)>>>,
}

impl PendingIssuedTokens {
    /// Stash `token` behind a fresh reference, redeemable until
    /// `ttl_nanos` from now. Returns the reference and its absolute expiry
    /// (nanoseconds-since-epoch, this module's existing timestamp
    /// convention — see [`now_nanos`]).
    async fn put(&self, token: String, ttl_nanos: i64) -> (String, i64) {
        let mut bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut bytes);
        let reference = format!(
            "{RETRIEVAL_REF_PREFIX}{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
        );
        let expires_at = now_nanos().saturating_add(ttl_nanos);
        self.inner
            .lock()
            .await
            .insert(reference.clone(), (token, expires_at));
        (reference, expires_at)
    }

    /// Redeem exactly once. Missing, already-redeemed, and expired all
    /// return `None` — indistinguishable on the wire by design (never leak
    /// which of the three actually happened). An expired entry is removed
    /// on this read even though it wasn't "used" for its token, so it
    /// doesn't linger in memory past its own TTL.
    async fn take(&self, reference: &str) -> Option<String> {
        let (token, expires_at) = self.inner.lock().await.remove(reference)?;
        (now_nanos() <= expires_at).then_some(token)
    }
}

/// The `POST /v1/credentials`(`/retrieve`) sub-router, with its own state and
/// the same rate-limit layer the rest of `/v1/*` carries.
///
/// The limiter keys on `x-bastion-token`, which this route does not use, so
/// every issuance/retrieval attempt shares one bucket. That is the desired
/// shape here: a bounded number of attempts per window against the operator
/// token, regardless of who is guessing.
fn credentials_router(
    credential_store: Arc<SqliteCredentialStore>,
    auth: crate::channel::operational::DaemonAccessAuth,
    rate_limiter: super::rate_limit::RateLimiter,
) -> Router {
    Router::new()
        .route("/v1/credentials", post(issue_credential))
        .route("/v1/credentials/retrieve", post(retrieve_credential_token))
        .layer(axum::middleware::from_fn_with_state(
            rate_limiter,
            super::rate_limit::enforce,
        ))
        .with_state(CredentialIssuanceState {
            credential_store,
            auth,
            pending_tokens: PendingIssuedTokens::default(),
        })
}

/// `POST /v1/credentials` — issue a Control Plane credential without a shell
/// on the daemon's host.
///
/// Closes the "issuance is console-only" gap for an operator running Bastion
/// on a VPS/container they cannot conveniently attach a console to, WITHOUT
/// widening what a Control Plane credential itself can do: the caller proves
/// operator authority with the daemon token, and an under-scoped or absent
/// token gets `401` from the same fail-closed check `/lifecycle/*` uses
/// (unconfigured token = every request refused).
///
/// The plaintext token is NOT in this response. Only its hash is stored
/// (mirroring the console command), and the response body carries a
/// one-time-use `retrieval_ref` instead — fetch the actual token exactly
/// once via `POST /v1/credentials/retrieve` before `retrieval_expires_at`.
/// Splitting issuance from retrieval narrows how many systems ever see the
/// plaintext in transit (a proxy/monitoring layer that snapshots this
/// response body gets a short-lived reference, not the credential) and
/// gives a crashed-before-reading caller a sharp failure (retrieval just
/// fails) instead of a silently un-retrievable token. This route is still
/// only mounted when the operator opts in
/// (`[control_plane] remote_credential_issuance`), and both bodies are worth
/// the same care as any other secret in transit — over plain HTTP either is
/// readable in flight, which is why the config field's documentation says to
/// terminate TLS in front of the daemon before enabling it.
async fn issue_credential(
    State(state): State<CredentialIssuanceState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> axum::response::Response {
    if !state.auth.authorized(&headers) {
        tracing::warn!(event = "v1_credential_issue_unauthorized");
        return error_response(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "this route requires the operator's daemon token",
        );
    }

    let req: CredentialIssueRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_body",
                &format!("invalid request body: {e}"),
            )
        }
    };

    let owner_id = req.owner_id.trim();
    if owner_id.is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_body",
            "owner_id must not be empty",
        );
    }
    let label = req.label.trim();
    if label.is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_body",
            "label must not be empty",
        );
    }
    // An empty scope list would mint a credential that authenticates and can
    // do nothing — always a mistake at the call site, never a useful default.
    if req.scopes.is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_body",
            "scopes must not be empty",
        );
    }
    // One unknown name rejects the whole request: silently dropping it would
    // hand back a credential with fewer grants than the caller believes it
    // has, which surfaces later as a confusing 403.
    let mut scopes = Vec::with_capacity(req.scopes.len());
    for name in &req.scopes {
        match Scope::from_wire_name(name) {
            Some(scope) => scopes.push(scope),
            None => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "invalid_scope",
                    &format!(
                        "unknown scope '{name}' (known: {})",
                        Scope::ALL
                            .iter()
                            .map(|s| s.wire_name())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                )
            }
        }
    }
    let project = req
        .project
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty());

    match state
        .credential_store
        .issue(
            owner_id,
            project,
            super::scope::ScopeSet::new(scopes.iter().copied()),
            label,
        )
        .await
    {
        Ok((id, token)) => {
            // Owner/label/scopes/id are logged; the token never is.
            tracing::info!(
                event = "control_plane_credential_issued_remotely",
                credential_id = %id,
                owner = %owner_id,
                label = %label,
            );
            let (retrieval_ref, retrieval_expires_at) =
                state.pending_tokens.put(token, RETRIEVAL_TTL_NANOS).await;
            (
                StatusCode::CREATED,
                Json(CredentialIssueResponse {
                    id,
                    owner_id: owner_id.to_string(),
                    project: project.map(str::to_string),
                    scopes: scopes.iter().map(|s| s.wire_name().to_string()).collect(),
                    label: label.to_string(),
                    retrieval_ref,
                    retrieval_expires_at,
                }),
            )
                .into_response()
        }
        Err(e) => {
            tracing::error!(event = "v1_credential_issue_failed", error = %e);
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "internal error",
            )
        }
    }
}

/// `POST /v1/credentials/retrieve` — redeem a `retrieval_ref` from
/// [`issue_credential`] for the plaintext token it stands in for, exactly
/// once. Gated by the SAME operator daemon token as issuance (defense in
/// depth: a reference that leaks somewhere the operator token did not is
/// still useless) — the reference alone is never sufficient.
///
/// Every failure mode (wrong/missing operator token aside) collapses to the
/// same `404`: unknown reference, already-redeemed reference, and expired
/// reference are indistinguishable on the wire, mirroring
/// `get_task`/`get_task_attempts`'s "404 never distinguishes wrong owner
/// from no such task" discipline — there is nothing a caller legitimately
/// needs to learn by telling these apart.
async fn retrieve_credential_token(
    State(state): State<CredentialIssuanceState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> axum::response::Response {
    if !state.auth.authorized(&headers) {
        tracing::warn!(event = "v1_credential_retrieve_unauthorized");
        return error_response(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "this route requires the operator's daemon token",
        );
    }

    let req: CredentialRetrieveRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_body",
                &format!("invalid request body: {e}"),
            )
        }
    };

    match state.pending_tokens.take(req.retrieval_ref.trim()).await {
        Some(token) => {
            tracing::info!(event = "control_plane_credential_token_retrieved");
            Json(CredentialRetrieveResponse { token }).into_response()
        }
        None => {
            tracing::warn!(event = "v1_credential_retrieve_miss");
            error_response(
                StatusCode::NOT_FOUND,
                "not_found",
                "unknown, already-retrieved, or expired retrieval_ref",
            )
        }
    }
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
pub(super) fn uuid_like_request_id() -> String {
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
        cred.project.as_deref(),
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
                    revoked_at: None,
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

/// `GET /v1/webhook-subscriptions` — this owner's subscriptions, newest
/// first, revoked ones included (with `revoked_at` set) so an operator can
/// tell "never registered" from "registered and later revoked".
///
/// The signing secret is NEVER present here: it exists in exactly one
/// response, the `POST` that created the subscription
/// (`WebhookSubscriptionResource::secret`'s doc comment), so every item
/// below is constructed with `secret: None`. Owner scoping comes from the
/// authenticated credential, never from a query parameter —
/// `list_for_owner`'s `WHERE owner_id = ?1` is the isolation boundary.
async fn list_webhook_subscriptions(
    State(state): State<ControlPlaneState>,
    headers: axum::http::HeaderMap,
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

    match state
        .webhook_subscription_store
        .list_for_owner(&cred.owner_id)
        .await
    {
        Ok(subscriptions) => {
            let items: Vec<WebhookSubscriptionResource> = subscriptions
                .into_iter()
                .map(|s| WebhookSubscriptionResource {
                    id: s.id,
                    owner_id: s.owner_id,
                    target_url: s.target_url,
                    event_types: s.event_types,
                    created_at: s.created_at,
                    revoked_at: s.revoked_at,
                    secret: None,
                })
                .collect();
            Json(WebhookSubscriptionListResponse { items }).into_response()
        }
        Err(e) => {
            tracing::error!(event = "v1_webhook_subscription_list_failed", error = %e);
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "internal error",
            )
        }
    }
}

/// `DELETE /v1/webhook-subscriptions/{id}` — stop delivering to a target.
///
/// Revocation is a tombstone (`revoked_at`), not a row deletion, so the
/// subscription keeps showing up in the list with its timestamp. The store's
/// `UPDATE ... WHERE id = ?1 AND owner_id = ?2` is the IDOR guard: another
/// owner's id answers `404 not_found`, indistinguishable from an id that
/// never existed, so this route cannot be used to probe for other owners'
/// subscription ids. Revoking twice is `409 already_revoked` rather than a
/// silent success — an idempotent-looking `204` would hide the fact that the
/// second caller was acting on stale state.
async fn revoke_webhook_subscription(
    State(state): State<ControlPlaneState>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
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

    match state
        .webhook_subscription_store
        .revoke(&cred.owner_id, &id)
        .await
    {
        Ok(()) => {
            tracing::info!(
                event = "control_plane_webhook_subscription_revoked",
                owner = %cred.owner_id,
                subscription_id = %id,
                credential_id = %cred.credential_id,
            );
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => match e.downcast_ref::<super::webhook_subscription::RevokeError>() {
            Some(super::webhook_subscription::RevokeError::NotFound) => error_response(
                StatusCode::NOT_FOUND,
                "not_found",
                "no webhook subscription with that id is visible to this owner",
            ),
            Some(super::webhook_subscription::RevokeError::AlreadyRevoked) => error_response(
                StatusCode::CONFLICT,
                "already_revoked",
                "this webhook subscription is already revoked",
            ),
            None => {
                tracing::error!(event = "v1_webhook_subscription_revoke_failed", error = %e);
                error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "internal error",
                )
            }
        },
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

    match core_ops::create_task(
        &state.core(),
        &cred.owner_id,
        &idempotency_key,
        req,
        cred.project.as_deref(),
    )
    .await
    {
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

#[cfg(test)]
mod pending_issued_tokens_tests {
    use super::PendingIssuedTokens;

    #[tokio::test]
    async fn put_then_take_returns_the_token_exactly_once() {
        let pending = PendingIssuedTokens::default();
        let (reference, _expires_at) = pending.put("bcp_secret".to_string(), 60_000_000_000).await;

        assert_eq!(
            pending.take(&reference).await,
            Some("bcp_secret".to_string())
        );
        assert_eq!(
            pending.take(&reference).await,
            None,
            "a reference must not be redeemable twice"
        );
    }

    #[tokio::test]
    async fn take_on_an_unknown_reference_returns_none() {
        let pending = PendingIssuedTokens::default();
        assert_eq!(pending.take("bcpr_never-issued").await, None);
    }

    #[tokio::test]
    async fn an_expired_reference_cannot_be_redeemed() {
        let pending = PendingIssuedTokens::default();
        // Negative TTL: expires_at is already in the past the instant it's stored.
        let (reference, _expires_at) = pending.put("bcp_secret".to_string(), -1).await;
        assert_eq!(
            pending.take(&reference).await,
            None,
            "an expired reference must not be redeemable even on its first use"
        );
    }

    #[tokio::test]
    async fn two_references_for_two_tokens_are_independent() {
        let pending = PendingIssuedTokens::default();
        let (ref_a, _) = pending.put("bcp_a".to_string(), 60_000_000_000).await;
        let (ref_b, _) = pending.put("bcp_b".to_string(), 60_000_000_000).await;
        assert_ne!(ref_a, ref_b, "references must be unpredictable/unique");

        assert_eq!(pending.take(&ref_a).await, Some("bcp_a".to_string()));
        // Redeeming A must not affect B.
        assert_eq!(pending.take(&ref_b).await, Some("bcp_b".to_string()));
    }
}
