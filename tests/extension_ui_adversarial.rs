//! Extension UI adversarial suite — Loop 3-D, CLD-08
//! (`docs/revamp/C3-cloud-ready-design.md` §Ponto de segurança 2). Extends
//! `tests/extension_adversarial.rs`'s style (a malicious attempt driven
//! through the REAL host surface, not a bare struct call) to the two named
//! vectors for extension-provided UI:
//!
//! (a) UI trying to execute in the host UI's own origin/context — asserted
//!     at the actual mounted HTTP route by checking the isolating
//!     `Content-Security-Policy: sandbox ...` response header a compliant
//!     browser enforces (never `allow-same-origin`, which would defeat it).
//! (b) UI trying to invoke a capability outside its declared `PermissionSet`
//!     — asserted at the actual mounted `/invoke` route, blocked with a
//!     typed error body, never silently reaching the real registry.
//!
//! `src/extension/ui.rs` already has its own unit tests calling
//! `ExtensionUiHost` methods directly; this file drives the same scenarios
//! through the REAL axum `Router` (`bastion::extension::ui::router`), the
//! same "test the actual host surface" discipline
//! `tests/extension_adversarial.rs`/`tests/extension_subprocess.rs` use.

use axum::body::Body;
use bastion::extension::ui::{ExtensionUiHost, RegisteredUiExtension};
use bastion_extension_protocol::PermissionSet;
use bastion_runtime::capability::CapabilityRegistry;
use http::{header, Request, StatusCode};
use std::collections::HashMap;
use std::sync::Arc;
use tower::ServiceExt;

fn html_asset(body: &str) -> (String, Vec<u8>) {
    ("text/html".to_string(), body.as_bytes().to_vec())
}

async fn build_router_with_widget(permissions: PermissionSet) -> axum::Router {
    let host = ExtensionUiHost::new(Arc::new(CapabilityRegistry::new()), "alice".to_string());
    let mut assets = HashMap::new();
    assets.insert(
        "index.html".to_string(),
        html_asset("<html><body>widget UI</body></html>"),
    );
    host.register(
        "acme/widget".to_string(),
        RegisteredUiExtension::new(permissions, assets).unwrap(),
    )
    .await;
    bastion::extension::ui::router(host)
}

/// Adversarial vector (a): a served extension-UI asset must carry the
/// isolating CSP `sandbox` directive, and that directive must NEVER include
/// `allow-same-origin` — the one token whose presence would let sandboxed
/// script reach back into the host UI's own origin/DOM/cookies, defeating
/// the entire isolation contract.
#[tokio::test]
async fn served_extension_ui_asset_is_isolated_from_host_ui_origin() {
    let app = build_router_with_widget(PermissionSet::none()).await;
    let req = Request::builder()
        .uri("/acme%2Fwidget/index.html")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let csp = resp
        .headers()
        .get(header::CONTENT_SECURITY_POLICY)
        .expect("extension UI response must carry an isolating CSP header")
        .to_str()
        .unwrap();
    assert!(
        csp.contains("sandbox"),
        "extension UI must be served with a CSP sandbox directive: {csp}"
    );
    assert!(
        !csp.contains("allow-same-origin"),
        "extension UI's sandbox directive must NEVER include allow-same-origin \
         (that would let it execute in the host UI's own origin): {csp}"
    );

    // Defense in depth: nosniff so the browser cannot be tricked into
    // reinterpreting the served content-type.
    assert_eq!(
        resp.headers().get(header::X_CONTENT_TYPE_OPTIONS).unwrap(),
        "nosniff"
    );
}

/// Adversarial vector (b): extension UI calling a capability outside its
/// manifest's declared `PermissionSet` is blocked at the mediated `/invoke`
/// route — 403, typed error body, never a 200 carrying real registry
/// output.
#[tokio::test]
async fn extension_ui_invoke_outside_permission_set_is_blocked() {
    // Widget declares NO capabilities at all (PermissionSet::none()).
    let app = build_router_with_widget(PermissionSet::none()).await;
    let body = serde_json::json!({"capability": "some:sensitive-capability", "args": {}});
    let req = Request::builder()
        .method("POST")
        .uri("/acme%2Fwidget/invoke")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let out: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let msg = out["error"].as_str().unwrap();
    assert!(
        msg.contains("acme/widget") && msg.contains("some:sensitive-capability"),
        "typed error must name the extension and the denied capability, not a generic message: {msg}"
    );
}

/// A wholly unregistered extension id can never be used to reach any
/// capability through the mediated bridge — 404, not a silent pass-through.
#[tokio::test]
async fn extension_ui_invoke_for_unregistered_extension_is_blocked() {
    let app = build_router_with_widget(PermissionSet::none()).await;
    let body = serde_json::json!({"capability": "anything", "args": {}});
    let req = Request::builder()
        .method("POST")
        .uri("/never%2Fregistered/invoke")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// A capability the extension DID declare still reaches the real registry —
/// the mediation chokepoint must not become a blanket deny; it enforces
/// exactly the declared boundary, no more, no less.
#[tokio::test]
async fn extension_ui_invoke_within_permission_set_is_allowed() {
    let mut registry = CapabilityRegistry::new();
    registry
        .register(Arc::new(EchoCapability))
        .expect("register echo");
    let host = ExtensionUiHost::new(Arc::new(registry), "alice".to_string());
    host.register(
        "acme/widget".to_string(),
        RegisteredUiExtension::new(
            PermissionSet {
                capabilities: vec!["acme/echo".to_string()],
                ..PermissionSet::none()
            },
            HashMap::new(),
        )
        .unwrap(),
    )
    .await;
    let app = bastion::extension::ui::router(host);

    let body = serde_json::json!({"capability": "acme/echo", "args": {"x": 1}});
    let req = Request::builder()
        .method("POST")
        .uri("/acme%2Fwidget/invoke")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let out: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(out["data"]["echo"], serde_json::json!({"x": 1}));
}

struct EchoCapability;

#[async_trait::async_trait]
impl bastion_runtime::capability::Capability for EchoCapability {
    fn name(&self) -> &str {
        "acme/echo"
    }
    fn description(&self) -> &str {
        "echoes input"
    }
    fn input_schema(&self) -> &serde_json::Value {
        static SCHEMA: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
        SCHEMA.get_or_init(|| serde_json::json!({}))
    }
    async fn invoke(
        &self,
        args: serde_json::Value,
        _ctx: &bastion_runtime::capability::InvokeCtx,
    ) -> anyhow::Result<serde_json::Value> {
        Ok(serde_json::json!({"echo": args}))
    }
    fn is_local(&self) -> bool {
        true
    }
}
