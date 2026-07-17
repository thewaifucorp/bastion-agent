//! Daemon operational contract — Loop 3-D
//! (`docs/revamp/C3-cloud-ready-design.md`): the surface that lets a hosted
//! operator run the SAME `bastion` binary as "just another sink" runs it —
//! liveness/readiness distinct from each other, a lifecycle stop/reload
//! surface, and a daemon-access auth hook. This module never learns
//! billing/marketplace/tenancy/control-plane — it is pure operational
//! plumbing, mounted onto the SAME axum router `src/channel/webhook.rs`
//! already serves (no second server, no new port).
//!
//! # Liveness vs readiness
//!
//! `/healthz` (liveness) answers ONE question: is this process alive and
//! able to handle an HTTP request at all? It never consults a dependency —
//! a hung provider or a dead MCP connection must NOT make an orchestrator
//! kill-and-restart a process that is otherwise fine (that would just repeat
//! the same failure in a crash loop). Always `200` once the router is
//! serving.
//!
//! `/readyz` (readiness) answers: has THIS instance finished the startup
//! sequence that makes it safe to route real traffic to? Backed by
//! [`ReadinessState`] — session store, memory store, and provider are
//! marked ready the moment `daemon_loop` starts (they are guaranteed
//! initialized by then: `main()` already propagated any of their own
//! failures before ever calling `daemon_loop`), and `channels` is marked
//! ready only once every configured channel has finished its spawn attempt,
//! right before the daemon enters its main `select!` loop. `503` with a
//! JSON breakdown of which component(s) are not yet ready until all are.
//!
//! # Lifecycle
//!
//! `POST /lifecycle/stop` triggers the exact same graceful-shutdown path as
//! SIGTERM/Ctrl-C (`daemon_loop`'s `select!` gains one more arm awaiting the
//! same [`tokio::sync::Notify`] this handler fires). `POST /lifecycle/reload`
//! reloads the persona registry from disk — the one piece of daemon state
//! that already has a well-defined "reload from source of truth" operation
//! (`PersonaRegistry::load_dir`) without requiring a broader config-hot-swap
//! redesign; anything else "reload" could mean is out of scope for this
//! contract and the handler says so rather than pretending to cover it.
//!
//! # Daemon-access auth hook
//!
//! Both lifecycle endpoints require `Authorization: Bearer <token>` checked
//! against [`DaemonAccessAuth`] — resolved BY REFERENCE through the same
//! `SecretResolver` as every other daemon secret (`BASTION_DAEMON_TOKEN`).
//! This is explicitly NOT provider auth (`AuthResolver`/`AuthProfileRef`) —
//! it is "who may talk to the daemon's own control surface", the hook a
//! hosted operator injects their own access-control layer through (a
//! reverse proxy verifying its own session, a platform-issued short-lived
//! token, ...). Unconfigured (`None`) fails closed: the lifecycle endpoints
//! refuse every request rather than defaulting open.

use axum::extract::State;
use axum::http::{header::AUTHORIZATION, HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Named boot-sequence dependencies `/readyz` reports on. Each is set at
/// most once, monotonically false→true (never flipped back) — this is a
/// "has this instance finished booting" gate, not a live per-request health
/// check of each dependency (see module docs).
#[derive(Default)]
pub struct ReadinessState {
    session: AtomicBool,
    memory: AtomicBool,
    provider: AtomicBool,
    channels: AtomicBool,
}

#[derive(Debug, Serialize)]
struct ReadinessSnapshot {
    ready: bool,
    session: bool,
    memory: bool,
    provider: bool,
    channels: bool,
}

impl ReadinessState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn mark_session_ready(&self) {
        self.session.store(true, Ordering::SeqCst);
    }
    pub fn mark_memory_ready(&self) {
        self.memory.store(true, Ordering::SeqCst);
    }
    pub fn mark_provider_ready(&self) {
        self.provider.store(true, Ordering::SeqCst);
    }
    pub fn mark_channels_ready(&self) {
        self.channels.store(true, Ordering::SeqCst);
    }

    /// Fase 2.9: `/status`'s `ready` field reuses this exact boolean (not a
    /// second gate) — just the AND of the four components `/readyz` already
    /// reports, without exposing the per-component breakdown a second time.
    pub fn is_ready(&self) -> bool {
        self.snapshot().ready
    }

    fn snapshot(&self) -> ReadinessSnapshot {
        let session = self.session.load(Ordering::SeqCst);
        let memory = self.memory.load(Ordering::SeqCst);
        let provider = self.provider.load(Ordering::SeqCst);
        let channels = self.channels.load(Ordering::SeqCst);
        ReadinessSnapshot {
            ready: session && memory && provider && channels,
            session,
            memory,
            provider,
            channels,
        }
    }
}

/// Daemon-access auth: gates `/lifecycle/*` only (never `/healthz`/`/readyz`
/// — orchestrator probes must not need a credential to ask "are you up").
/// `None` = not configured, fails closed (every lifecycle request refused).
#[derive(Clone, Default)]
pub struct DaemonAccessAuth {
    token: Option<String>,
}

impl DaemonAccessAuth {
    pub fn new(token: Option<String>) -> Self {
        Self { token }
    }

    /// Same constant-time bearer check as `src/api/infer.rs`'s
    /// `InferState` — length is allowed to leak (fixed-length bearer
    /// token), the comparison itself never short-circuits.
    fn authorized(&self, headers: &HeaderMap) -> bool {
        let expected = match &self.token {
            Some(t) => t,
            None => return false, // fail closed: unconfigured means refused, never open
        };
        let provided = headers
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "));
        match provided {
            Some(tok) => constant_time_eq(tok.as_bytes(), expected.as_bytes()),
            None => false,
        }
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Liveness — always `200` once the router is serving. Never touches
/// `ReadinessState` or any other dependency.
pub async fn liveness_handler() -> impl IntoResponse {
    (StatusCode::OK, Json(serde_json::json!({"status": "alive"})))
}

/// Readiness — `200` only once every named dependency in [`ReadinessState`]
/// has reported ready; `503` with a JSON breakdown otherwise.
pub async fn readiness_handler(State(state): State<Arc<ReadinessState>>) -> impl IntoResponse {
    let snapshot = state.snapshot();
    let status = if snapshot.ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (status, Json(snapshot))
}

/// Shared lifecycle-control state mounted into the webhook `AppState`.
#[derive(Clone)]
pub struct LifecycleControl {
    pub auth: DaemonAccessAuth,
    /// Fired to signal `daemon_loop`'s `select!` to break, exactly like
    /// SIGTERM/Ctrl-C.
    pub shutdown: Arc<tokio::sync::Notify>,
    /// Fired to signal `daemon_loop` to reload the persona registry from
    /// disk. A `Notify` (not a return value) because the HTTP handler
    /// cannot itself touch `AgentLoop` — only the single `&mut agent` owner
    /// (`daemon_loop`) may.
    pub reload: Arc<tokio::sync::Notify>,
}

impl LifecycleControl {
    pub fn new(auth: DaemonAccessAuth) -> Self {
        Self {
            auth,
            shutdown: Arc::new(tokio::sync::Notify::new()),
            reload: Arc::new(tokio::sync::Notify::new()),
        }
    }
}

pub async fn lifecycle_stop_handler(
    State(lifecycle): State<LifecycleControl>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !lifecycle.auth.authorized(&headers) {
        tracing::warn!(event = "lifecycle_stop_unauthorized");
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({}))).into_response();
    }
    tracing::info!(event = "lifecycle_stop_requested_via_http");
    lifecycle.shutdown.notify_one();
    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({"status": "stopping"})),
    )
        .into_response()
}

/// Fase 2.9: one row of `/status`'s `runtimes` array — booleans only, never
/// an account name/email/label (that's the whole point of this route: a
/// remote caller — mobile companion, monitoring — can tell "is subscription
/// X usable right now" without ever seeing WHO is logged in).
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct RuntimeStatusRow {
    pub id: String,
    pub cli_present: bool,
    pub logged_in: bool,
}

#[derive(Debug, Serialize)]
pub struct StatusSnapshot {
    pub runtimes: Vec<RuntimeStatusRow>,
    pub ready: bool,
}

/// `GET /status` — booleans-only summary of every runtime Bastion knows how
/// to wrap (Fase 2.9). `cli_present` mirrors `agent_runtime_registry`'s own
/// `health()` probe (a `--version` spawn — installed/working, NOT
/// "logged in", see that module's doc); `logged_in` is a live
/// `auth_profile_registry::probe_host_cli` against the mapped
/// `[auth.<profile>]` entry. `ready` is `session && memory && provider &&
/// channels` from the SAME `ReadinessState` `/readyz` already reports (not a
/// new gate) — this route just adds runtime/login detail alongside it.
pub async fn status_handler(State(state): State<StatusState>) -> impl IntoResponse {
    let mut runtimes = Vec::new();
    for descriptor in state.runtime_registry.descriptors() {
        let cli_present = state.runtime_registry.resolve(descriptor.id).await.is_ok();
        let logged_in = match crate::agent::backend_command::RUNTIME_AUTH_PROFILES
            .iter()
            .find(|(id, _)| *id == descriptor.id)
            .and_then(|(_, profile)| state.auth.profiles.get(*profile))
        {
            Some(crate::config::AuthProfileEntry::HostCli { cli }) => {
                crate::auth_profile_registry::probe_host_cli(cli)
                    .await
                    .is_ok()
            }
            _ => false,
        };
        runtimes.push(RuntimeStatusRow {
            id: descriptor.id.to_string(),
            cli_present,
            logged_in,
        });
    }
    let ready = state.readiness.is_ready();
    (StatusCode::OK, Json(StatusSnapshot { runtimes, ready })).into_response()
}

/// State `/status` needs, mounted alongside `AppState` in `webhook.rs`.
#[derive(Clone)]
pub struct StatusState {
    pub runtime_registry: bastion_runtime::agent::backend::RuntimeRegistry,
    pub auth: crate::config::AuthConfig,
    pub readiness: Arc<ReadinessState>,
}

pub async fn lifecycle_reload_handler(
    State(lifecycle): State<LifecycleControl>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !lifecycle.auth.authorized(&headers) {
        tracing::warn!(event = "lifecycle_reload_unauthorized");
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({}))).into_response();
    }
    tracing::info!(event = "lifecycle_reload_requested_via_http");
    lifecycle.reload.notify_one();
    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({"status": "reloading", "scope": "persona_registry"})),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::routing::{get, post};
    use axum::Router;
    use http::{Request, StatusCode as HttpStatus};
    use tower::ServiceExt;

    #[test]
    fn readiness_state_starts_not_ready() {
        let state = ReadinessState::new();
        let snap = state.snapshot();
        assert!(!snap.ready);
        assert!(!snap.session && !snap.memory && !snap.provider && !snap.channels);
    }

    #[test]
    fn readiness_state_ready_only_after_every_component_marked() {
        let state = ReadinessState::new();
        state.mark_session_ready();
        assert!(!state.snapshot().ready);
        state.mark_memory_ready();
        assert!(!state.snapshot().ready);
        state.mark_provider_ready();
        assert!(!state.snapshot().ready);
        state.mark_channels_ready();
        assert!(state.snapshot().ready, "all four marked — must be ready");
    }

    #[tokio::test]
    async fn liveness_handler_always_200() {
        let app = Router::new().route("/healthz", get(liveness_handler));
        let req = Request::builder()
            .uri("/healthz")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), HttpStatus::OK);
    }

    #[tokio::test]
    async fn readiness_handler_503_until_all_dependencies_ready() {
        let readiness = ReadinessState::new();
        let app = Router::new()
            .route("/readyz", get(readiness_handler))
            .with_state(readiness.clone());
        let req = Request::builder()
            .uri("/readyz")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), HttpStatus::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn readiness_handler_200_once_all_dependencies_ready() {
        let readiness = ReadinessState::new();
        readiness.mark_session_ready();
        readiness.mark_memory_ready();
        readiness.mark_provider_ready();
        readiness.mark_channels_ready();
        let app = Router::new()
            .route("/readyz", get(readiness_handler))
            .with_state(readiness.clone());
        let req = Request::builder()
            .uri("/readyz")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), HttpStatus::OK);
    }

    fn lifecycle_router(lifecycle: LifecycleControl) -> Router {
        Router::new()
            .route("/lifecycle/stop", post(lifecycle_stop_handler))
            .route("/lifecycle/reload", post(lifecycle_reload_handler))
            .with_state(lifecycle)
    }

    #[tokio::test]
    async fn lifecycle_stop_without_configured_token_always_refused() {
        let lifecycle = LifecycleControl::new(DaemonAccessAuth::new(None));
        let app = lifecycle_router(lifecycle);
        let req = Request::builder()
            .method("POST")
            .uri("/lifecycle/stop")
            .header("authorization", "Bearer anything")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), HttpStatus::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn lifecycle_stop_wrong_token_refused() {
        let lifecycle = LifecycleControl::new(DaemonAccessAuth::new(Some("right".to_string())));
        let app = lifecycle_router(lifecycle);
        let req = Request::builder()
            .method("POST")
            .uri("/lifecycle/stop")
            .header("authorization", "Bearer wrong")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), HttpStatus::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn lifecycle_stop_correct_token_triggers_shutdown_notify() {
        let lifecycle = LifecycleControl::new(DaemonAccessAuth::new(Some("right".to_string())));
        let shutdown = lifecycle.shutdown.clone();
        let app = lifecycle_router(lifecycle);
        let req = Request::builder()
            .method("POST")
            .uri("/lifecycle/stop")
            .header("authorization", "Bearer right")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), HttpStatus::ACCEPTED);

        // notify_one() is observed by a `notified()` call that started
        // waiting before the notify — this proves the signal actually
        // fired, not merely that the handler didn't error.
        tokio::time::timeout(std::time::Duration::from_secs(1), shutdown.notified())
            .await
            .expect("shutdown notify must have fired");
    }

    #[tokio::test]
    async fn lifecycle_reload_correct_token_triggers_reload_notify() {
        let lifecycle = LifecycleControl::new(DaemonAccessAuth::new(Some("right".to_string())));
        let reload = lifecycle.reload.clone();
        let app = lifecycle_router(lifecycle);
        let req = Request::builder()
            .method("POST")
            .uri("/lifecycle/reload")
            .header("authorization", "Bearer right")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), HttpStatus::ACCEPTED);
        tokio::time::timeout(std::time::Duration::from_secs(1), reload.notified())
            .await
            .expect("reload notify must have fired");
    }
}
