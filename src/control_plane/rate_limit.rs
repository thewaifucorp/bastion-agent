//! Fixed-window rate limiter, shared between the Control Plane `/v1/*` HTTP
//! routes and the 5 Control Plane MCP tools (`mcp::server::BastionMcpServer`).
//!
//! Backlog: "rate limiting no Control Plane e nas tools MCP" (P0). Keyed by
//! CREDENTIAL, not IP (this is an authenticated API — the raw
//! `x-bastion-token` value itself is a fine key, no need to resolve it to an
//! `AuthenticatedCredential`/`TokenPermissions` first), a single fixed limit
//! with no configuration surface yet. One instance is built per `Daemon`
//! process (see `main.rs`) and shared by both surfaces; `McpStdio` is its
//! own separate process and gets its own instance — there's nothing to share
//! across a process boundary. Sharing HTTP and MCP behind one instance is
//! simpler, not a fix for a real double-budget scenario: `/v1/*` credentials
//! (`SqliteCredentialStore`, `bcp_*` tokens) and MCP's `TokenPermissions`
//! (static config) are separate token spaces with no overlapping values
//! today, so one instance vs. two behaves identically either way.
//!
//! Deliberately hand-rolled rather than a `tower_governor`/`governor`
//! dependency: the v1 requirement (fixed window, in-memory, one key shape)
//! doesn't need a general-purpose rate-limiting crate's configuration
//! surface, and this codebase's own convention is to avoid a dependency for
//! a self-contained handful of lines (see `credential::uuid_like_id`,
//! `routes::uuid_like_request_id`).
//!
//! Algorithm: fixed window, not sliding/token-bucket — simpler to reason
//! about and test, and "generous fixed limit" was the agreed v1 shape, not
//! smooth throughput shaping. A key can burst up to `MAX_REQUESTS_PER_WINDOW`
//! right at a window boundary (two bursts ~`WINDOW` apart can land in
//! adjacent windows) — an accepted, documented imprecision for v1.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use tokio::sync::Mutex;

use super::dto::ErrorEnvelope;

const WINDOW: Duration = Duration::from_secs(60);
/// `pub` (not just visible within this crate) so integration tests in
/// `tests/control_plane_routes.rs` can drive exactly this many requests
/// through the real router instead of hardcoding a number that could drift
/// out of sync with this one.
pub const MAX_REQUESTS_PER_WINDOW: u32 = 60;

struct Window {
    count: u32,
    started_at: Instant,
}

/// Cheap to clone (one `Arc`) — a single instance is built once in
/// [`super::routes::router`] and captured by the middleware closure.
#[derive(Clone, Default)]
pub struct RateLimiter {
    windows: Arc<Mutex<HashMap<String, Window>>>,
}

impl RateLimiter {
    pub fn new() -> Self {
        Self::default()
    }

    /// `true` = under the limit for `key`'s current window (and the request
    /// counts against it); `false` = this request is over the limit.
    ///
    /// `pub(crate)`, not private: shared with `mcp::server::BastionMcpServer`
    /// (the same instance, constructed once in `main.rs` — see that
    /// constructor's own doc comment for why sharing one `RateLimiter` is
    /// harmless either way in the current architecture).
    pub(crate) async fn check(&self, key: &str) -> bool {
        let mut windows = self.windows.lock().await;
        let now = Instant::now();
        let window = windows.entry(key.to_owned()).or_insert_with(|| Window {
            count: 0,
            started_at: now,
        });
        if now.duration_since(window.started_at) >= WINDOW {
            window.count = 0;
            window.started_at = now;
        }
        window.count += 1;
        window.count <= MAX_REQUESTS_PER_WINDOW
    }
}

/// `axum::middleware::from_fn_with_state` handler — applied to the `/v1/*`
/// router only (see `routes::router`), so it never touches the webhook
/// channel's own request volume.
///
/// Keyed on the raw `x-bastion-token` header value, BEFORE any credential
/// lookup — a missing/garbage token shares one bucket with every other
/// missing/garbage token (it can't do anything past the 401 the handler
/// itself still issues), so this never needs to touch the credential store
/// or fail differently than "count it and move on."
pub async fn enforce(State(limiter): State<RateLimiter>, req: Request, next: Next) -> Response {
    let key = req
        .headers()
        .get("x-bastion-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();

    if !limiter.check(&key).await {
        tracing::warn!(event = "v1_rate_limited");
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(ErrorEnvelope {
                code: "rate_limited".to_string(),
                message: format!(
                    "more than {MAX_REQUESTS_PER_WINDOW} requests in the current \
                     {}s window for this credential",
                    WINDOW.as_secs()
                ),
                request_id: super::routes::uuid_like_request_id(),
            }),
        )
            .into_response();
    }

    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn allows_up_to_the_limit_then_denies() {
        let limiter = RateLimiter::new();
        for _ in 0..MAX_REQUESTS_PER_WINDOW {
            assert!(limiter.check("tok_a").await, "must allow up to the limit");
        }
        assert!(
            !limiter.check("tok_a").await,
            "must deny the request past the limit"
        );
    }

    #[tokio::test]
    async fn keys_are_isolated_from_each_other() {
        let limiter = RateLimiter::new();
        for _ in 0..MAX_REQUESTS_PER_WINDOW {
            assert!(limiter.check("tok_a").await);
        }
        assert!(
            !limiter.check("tok_a").await,
            "tok_a must be over its own limit"
        );
        assert!(
            limiter.check("tok_b").await,
            "a different key must have its own, untouched budget"
        );
    }

    #[tokio::test]
    async fn window_resets_after_it_elapses() {
        let limiter = RateLimiter::new();
        {
            let mut windows = limiter.windows.lock().await;
            windows.insert(
                "tok_a".to_string(),
                Window {
                    count: MAX_REQUESTS_PER_WINDOW,
                    // Force the window to already look elapsed rather than
                    // sleeping 60s in a unit test.
                    started_at: Instant::now() - WINDOW - Duration::from_secs(1),
                },
            );
        }
        assert!(
            limiter.check("tok_a").await,
            "an elapsed window must reset the count, not keep denying forever"
        );
    }
}
