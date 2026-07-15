//! Inference gateway for Python MCP containers (D-08 / D-09).
//!
//! POST /api/infer — receives {prompt, privacy_tier} from skill-writer / self-improving
//! and routes through the existing Provider trait + egress check.
//! Python containers hold ZERO raw API keys.

use axum::{
    extract::State,
    http::{header::AUTHORIZATION, HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use serde::{Deserialize, Serialize};

use bastion_memory::PrivacyTier;
use bastion_providers::SharedProvider;
use bastion_runtime::hooks::egress::check_egress;
use bastion_types::BastionError;

#[derive(Deserialize)]
struct InferRequest {
    prompt: String,
    privacy_tier: String, // "cloud_ok" | "local_only"
}

#[derive(Serialize)]
struct InferResponse {
    text: String,
}

#[derive(Clone)]
pub(crate) struct InferState {
    pub provider: SharedProvider,
    /// Shared secret required as `Authorization: Bearer <token>`. When `None`,
    /// auth is disabled (loopback-only dev mode — main.rs refuses non-loopback
    /// binds without a token). See SEC: unauthenticated token-minting.
    pub token: Option<String>,
}

fn parse_tier(s: &str) -> Option<PrivacyTier> {
    match s {
        "cloud_ok" => Some(PrivacyTier::CloudOk),
        "local_only" => Some(PrivacyTier::LocalOnly),
        _ => None,
    }
}

/// Constant-time byte comparison. Length is allowed to leak (acceptable for a
/// fixed-length bearer token); the equal-length comparison itself does not
/// short-circuit, preventing timing oracles on the secret's contents.
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

/// Validate the bearer token. Returns true when auth is disabled (`token: None`)
/// or the presented `Authorization: Bearer …` matches in constant time.
fn authorized(state: &InferState, headers: &HeaderMap) -> bool {
    let expected = match &state.token {
        Some(t) => t,
        None => return true,
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

async fn handle_infer(
    State(state): State<InferState>,
    headers: HeaderMap,
    Json(body): Json<InferRequest>,
) -> impl IntoResponse {
    if !authorized(&state, &headers) {
        tracing::warn!(event = "infer_unauthorized");
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({}))).into_response();
    }

    let tier = match parse_tier(&body.privacy_tier) {
        Some(t) => t,
        None => {
            tracing::warn!(event = "infer_bad_tier", tier = %body.privacy_tier);
            return (StatusCode::BAD_REQUEST, Json(serde_json::json!({}))).into_response();
        }
    };

    let prov = state.provider.read().await;

    if let Err(e) = check_egress(Some(tier), prov.name()) {
        let status = if e
            .downcast_ref::<BastionError>()
            .map(|b| matches!(b, BastionError::PrivacyEgressBlocked))
            .unwrap_or(false)
        {
            StatusCode::FORBIDDEN
        } else {
            StatusCode::INTERNAL_SERVER_ERROR
        };
        tracing::warn!(event = "infer_egress_blocked", provider = %prov.name());
        return (status, Json(serde_json::json!({}))).into_response();
    }

    match prov.complete_simple(&body.prompt).await {
        Ok(text) => Json(InferResponse { text }).into_response(),
        Err(e) => {
            tracing::error!(event = "infer_provider_error", err = %e);
            (StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({}))).into_response()
        }
    }
}

/// Build the axum Router for the /api/infer endpoint.
/// Mount this router in main.rs to expose the inference gateway.
///
/// `token` is the shared secret required on every request as
/// `Authorization: Bearer <token>`. Pass `None` only for loopback-only dev.
pub fn router(provider: SharedProvider, token: Option<String>) -> Router {
    let state = InferState { provider, token };
    Router::new()
        .route("/api/infer", post(handle_infer))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use http::{Request, StatusCode};
    use tower::ServiceExt;

    fn build_router() -> axum::Router {
        use std::sync::Arc;
        use tokio::sync::RwLock;
        let provider: bastion_providers::SharedProvider =
            Arc::new(RwLock::new(Box::new(StubProvider {
                name: "anthropic",
                fail: false,
            })
                as Box<dyn bastion_providers::Provider>));
        super::router(provider, None)
    }

    fn build_router_fail() -> axum::Router {
        use std::sync::Arc;
        use tokio::sync::RwLock;
        let provider: bastion_providers::SharedProvider =
            Arc::new(RwLock::new(Box::new(StubProvider {
                name: "anthropic",
                fail: true,
            })
                as Box<dyn bastion_providers::Provider>));
        super::router(provider, None)
    }

    fn build_router_ollama() -> axum::Router {
        use std::sync::Arc;
        use tokio::sync::RwLock;
        let provider: bastion_providers::SharedProvider =
            Arc::new(RwLock::new(Box::new(StubProvider {
                name: "ollama",
                fail: false,
            })
                as Box<dyn bastion_providers::Provider>));
        super::router(provider, None)
    }

    fn build_router_with_token(token: &str) -> axum::Router {
        use std::sync::Arc;
        use tokio::sync::RwLock;
        let provider: bastion_providers::SharedProvider =
            Arc::new(RwLock::new(Box::new(StubProvider {
                name: "anthropic",
                fail: false,
            })
                as Box<dyn bastion_providers::Provider>));
        super::router(provider, Some(token.to_owned()))
    }

    struct StubProvider {
        name: &'static str,
        fail: bool,
    }

    #[async_trait::async_trait]
    impl bastion_providers::Provider for StubProvider {
        async fn complete(
            &self,
            _messages: &[bastion_types::Message],
            _config: &bastion_types::CallConfig,
        ) -> anyhow::Result<bastion_types::LlmResponse> {
            Ok(bastion_types::LlmResponse {
                text: "ok".into(),
                tool_calls: None,
                usage: bastion_types::TokenUsage::default(),
            })
        }
        async fn complete_simple(&self, _prompt: &str) -> anyhow::Result<String> {
            if self.fail {
                anyhow::bail!("provider error")
            } else {
                Ok("ok".into())
            }
        }
        fn context_limit(&self) -> usize {
            4096
        }
        fn model_name(&self) -> &str {
            self.name
        }
        fn name(&self) -> &'static str {
            self.name
        }
    }

    #[tokio::test]
    async fn infer_invalid_tier_returns_400() {
        let app = build_router();
        let body = serde_json::json!({"prompt": "hi", "privacy_tier": "unknown"}).to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/api/infer")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn infer_local_only_non_ollama_returns_403() {
        let app = build_router(); // provider name = "anthropic"
        let body = serde_json::json!({"prompt": "hi", "privacy_tier": "local_only"}).to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/api/infer")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn infer_cloud_ok_returns_200_with_text() {
        let app = build_router(); // provider name = "anthropic", no fail
        let body = serde_json::json!({"prompt": "hi", "privacy_tier": "cloud_ok"}).to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/api/infer")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let out: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(out["text"], "ok");
    }

    #[tokio::test]
    async fn infer_provider_fail_returns_503() {
        let app = build_router_fail();
        let body = serde_json::json!({"prompt": "hi", "privacy_tier": "cloud_ok"}).to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/api/infer")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn infer_local_only_ollama_returns_200() {
        let app = build_router_ollama();
        let body = serde_json::json!({"prompt": "hi", "privacy_tier": "local_only"}).to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/api/infer")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn infer_error_response_no_internal_detail() {
        let app = build_router_fail();
        let body = serde_json::json!({"prompt": "hi", "privacy_tier": "cloud_ok"}).to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/api/infer")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_ne!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            !text.contains("provider error"),
            "must not leak internal error: {text}"
        );
        assert!(!text.contains("panicked"), "must not leak panic: {text}");
    }

    #[tokio::test]
    async fn infer_missing_token_returns_401() {
        let app = build_router_with_token("s3cret");
        let body = serde_json::json!({"prompt": "hi", "privacy_tier": "cloud_ok"}).to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/api/infer")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn infer_wrong_token_returns_401() {
        let app = build_router_with_token("s3cret");
        let body = serde_json::json!({"prompt": "hi", "privacy_tier": "cloud_ok"}).to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/api/infer")
            .header("content-type", "application/json")
            .header("authorization", "Bearer wrong")
            .body(Body::from(body))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn infer_correct_token_authorizes() {
        let app = build_router_with_token("s3cret");
        let body = serde_json::json!({"prompt": "hi", "privacy_tier": "cloud_ok"}).to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/api/infer")
            .header("content-type", "application/json")
            .header("authorization", "Bearer s3cret")
            .body(Body::from(body))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
