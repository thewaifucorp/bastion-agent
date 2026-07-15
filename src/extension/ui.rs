//! Extension UI isolation — Loop 3-D, CLD-08
//! (`docs/revamp/C3-cloud-ready-design.md` §Ponto de segurança 2): an
//! extension may `provides: Ui` (`bastion_extension_protocol::Provided::Ui`,
//! declared in Loop 3-C, never wired to a serving mechanism until now).
//! Constraint (non-negotiable, same wording as the design doc):
//!
//! - Extension UI runs isolated by capability/sandbox — **forbidden to
//!   execute arbitrary code same-origin with the host UI**, no access to the
//!   host document's DOM/state, no unmediated privileged calls.
//! - Extension UI talks to the backend ONLY through the SAME
//!   `CapabilityRegistry` (mediated, gated by the permissions declared in
//!   the manifest) — never a privileged direct channel.
//!
//! This module is the host-level enforcement chokepoint for that contract —
//! product code, deliberately outside the kernel, exactly like
//! `src/extension/host.rs`/`facade.rs` are for the capability/lockfile side.
//! There is no existing rich web cockpit in this repo to attach a real
//! browser to (the cockpit today is the chat/slash-command surface) — this
//! ships the MECHANISM (isolating headers + a single mediated invoke
//! endpoint) with adversarial coverage that does not require a browser:
//! response headers are asserted directly (the CSP/sandbox contract a
//! compliant browser enforces), and the mediation chokepoint is asserted by
//! calling it directly, the same style `tests/extension_adversarial.rs`
//! already uses for the non-UI mechanisms. Wiring a per-owner instance of
//! this host into `main.rs`'s axum router is a follow-up once a real UI
//! consumer exists (same "mechanism now, product wiring later" pattern
//! M4-09/M4-10 already used in this codebase).
//!
//! # Isolation mechanism
//!
//! Every served asset carries `Content-Security-Policy: sandbox
//! allow-scripts; default-src 'self'`. The CSP `sandbox` directive — critically
//! WITHOUT the `allow-same-origin` token — is the standards-based mechanism
//! that makes a compliant browser treat the response as if it were an
//! `<iframe sandbox="allow-scripts">`: it may run its own script, but that
//! script executes in a forced, unique opaque origin, structurally unable to
//! read/write the embedding host document's DOM, cookies, or storage even if
//! it tries. `X-Content-Type-Options: nosniff` and `Content-Security-Policy:
//! frame-ancestors 'self'` are defense in depth alongside it.
//!
//! The ONLY channel back to the backend is `POST /ext-ui/{id}/invoke`, which
//! resolves the SAME `PermissionSet` the extension's manifest declared and
//! rejects (typed [`ExtensionError::CapabilityNotDeclared`]) any capability
//! name outside it BEFORE ever touching the real `CapabilityRegistry` —
//! there is no second, unmediated way for served script to reach the
//! backend.

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use bastion_extension_protocol::{ExtensionError, PermissionSet};
use bastion_memory::PrivacyTier;
use bastion_runtime::capability::{CapabilityRegistry, InvokeCtx};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// One extension's registered UI bundle: static assets (path, content-type,
/// bytes) plus the SAME `PermissionSet` its manifest declared — every
/// `/invoke` call from this extension's UI is checked against this, never
/// a wider ambient grant.
pub struct RegisteredUiExtension {
    pub permissions: PermissionSet,
    /// Relative asset path (no leading `/`, never containing `..` —
    /// enforced at registration, not just lookup, so a malformed
    /// registration cannot smuggle a traversal key into the map either).
    pub assets: HashMap<String, (String, Vec<u8>)>,
}

impl RegisteredUiExtension {
    /// `assets` keys are normalized/validated here — a caller cannot
    /// register an asset path containing `..` or a leading `/`, closing the
    /// same traversal vector `get_asset` separately rejects on lookup.
    pub fn new(
        permissions: PermissionSet,
        assets: HashMap<String, (String, Vec<u8>)>,
    ) -> Result<Self, ExtensionError> {
        for path in assets.keys() {
            if !is_safe_relative_path(path) {
                return Err(ExtensionError::InvalidManifest {
                    id: String::new(),
                    reason: format!("unsafe UI asset path '{path}'"),
                });
            }
        }
        Ok(Self {
            permissions,
            assets,
        })
    }
}

fn is_safe_relative_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.split('/').any(|seg| seg.is_empty() || seg == "..")
}

/// The extension-UI host: per-owner (an `InvokeCtx` always carries an
/// owner), mediates every `/ext-ui/{id}/invoke` call through the SAME
/// `CapabilityRegistry` the rest of the daemon uses, gated by that
/// extension's own declared permissions — never a raw registry handle
/// reachable from served script.
pub struct ExtensionUiHost {
    registry: Arc<CapabilityRegistry>,
    owner: String,
    privacy_tier: Option<PrivacyTier>,
    extensions: RwLock<HashMap<String, RegisteredUiExtension>>,
}

impl ExtensionUiHost {
    pub fn new(registry: Arc<CapabilityRegistry>, owner: String) -> Arc<Self> {
        Arc::new(Self {
            registry,
            owner,
            privacy_tier: Some(PrivacyTier::CloudOk),
            extensions: RwLock::new(HashMap::new()),
        })
    }

    pub async fn register(&self, extension_id: String, ui: RegisteredUiExtension) {
        self.extensions.write().await.insert(extension_id, ui);
    }

    pub async fn deregister(&self, extension_id: &str) {
        self.extensions.write().await.remove(extension_id);
    }

    async fn asset(&self, extension_id: &str, path: &str) -> Option<(String, Vec<u8>)> {
        if !is_safe_relative_path(path) {
            return None;
        }
        let extensions = self.extensions.read().await;
        let ext = extensions.get(extension_id)?;
        ext.assets.get(path).cloned()
    }

    /// The ONE mediated bridge a served UI may use. Checks the extension's
    /// OWN declared `PermissionSet.capabilities` before ever calling
    /// `CapabilityRegistry::invoke` — this is the enforcement chokepoint,
    /// not the served script's own good behavior.
    async fn invoke(
        &self,
        extension_id: &str,
        capability: &str,
        args: serde_json::Value,
    ) -> Result<bastion_runtime::capability::TaggedValue, ExtensionError> {
        let extensions = self.extensions.read().await;
        let ext = extensions
            .get(extension_id)
            .ok_or_else(|| ExtensionError::NotFound {
                id: extension_id.to_string(),
            })?;
        if !ext.permissions.allows_capability(capability) {
            return Err(ExtensionError::CapabilityNotDeclared {
                extension: extension_id.to_string(),
                capability: capability.to_string(),
            });
        }
        drop(extensions); // release the read lock before the (potentially slow) invoke

        let ctx = InvokeCtx {
            owner: self.owner.clone(),
            privacy_tier: self.privacy_tier,
        };
        self.registry
            .invoke(capability, args, &ctx)
            .await
            .map_err(|e| ExtensionError::Mechanism {
                id: extension_id.to_string(),
                detail: e.to_string(),
            })
    }
}

#[derive(Deserialize)]
struct InvokeRequest {
    capability: String,
    #[serde(default)]
    args: serde_json::Value,
}

#[derive(Serialize)]
struct InvokeResponse {
    data: serde_json::Value,
    trusted: bool,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

fn extension_error_status(e: &ExtensionError) -> StatusCode {
    match e {
        ExtensionError::CapabilityNotDeclared { .. } => StatusCode::FORBIDDEN,
        ExtensionError::NotFound { .. } => StatusCode::NOT_FOUND,
        _ => StatusCode::BAD_REQUEST,
    }
}

/// `GET /ext-ui/{id}/{*path}` — serves one static asset, isolated.
async fn serve_asset(
    State(host): State<Arc<ExtensionUiHost>>,
    Path((id, path)): Path<(String, String)>,
) -> impl IntoResponse {
    match host.asset(&id, &path).await {
        Some((content_type, bytes)) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, content_type),
                (
                    header::CONTENT_SECURITY_POLICY,
                    "sandbox allow-scripts; default-src 'self'; frame-ancestors 'self'".to_string(),
                ),
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
            ],
            bytes,
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

/// `POST /ext-ui/{id}/invoke` — the one mediated bridge back to the backend.
async fn invoke_handler(
    State(host): State<Arc<ExtensionUiHost>>,
    Path(id): Path<String>,
    Json(body): Json<InvokeRequest>,
) -> impl IntoResponse {
    match host.invoke(&id, &body.capability, body.args).await {
        Ok(tagged) => (
            StatusCode::OK,
            Json(InvokeResponse {
                data: tagged.data,
                trusted: tagged.trusted,
            }),
        )
            .into_response(),
        Err(e) => {
            tracing::warn!(
                event = "extension_ui_invoke_denied",
                extension = %id,
                error = %e,
            );
            (
                extension_error_status(&e),
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
                .into_response()
        }
    }
}

/// Builds the axum sub-router for extension UI — mount at any prefix (e.g.
/// `.nest("/ext-ui", extension::ui::router(host))`).
pub fn router(host: Arc<ExtensionUiHost>) -> Router {
    Router::new()
        .route("/{id}/invoke", post(invoke_handler))
        .route("/{id}/{*path}", get(serve_asset))
        .with_state(host)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bastion_extension_protocol::EgressScope;

    fn html_asset(body: &str) -> (String, Vec<u8>) {
        ("text/html".to_string(), body.as_bytes().to_vec())
    }

    #[test]
    fn safe_relative_path_rejects_traversal_and_absolute() {
        assert!(is_safe_relative_path("index.html"));
        assert!(is_safe_relative_path("assets/app.js"));
        assert!(!is_safe_relative_path("../secret"));
        assert!(!is_safe_relative_path("/etc/passwd"));
        assert!(!is_safe_relative_path("assets/../../escape"));
        assert!(!is_safe_relative_path(""));
    }

    #[test]
    fn registered_ui_extension_rejects_unsafe_asset_path_at_construction() {
        let mut assets = HashMap::new();
        assets.insert("../escape".to_string(), html_asset("<html></html>"));
        let result = RegisteredUiExtension::new(PermissionSet::none(), assets);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn serve_asset_returns_isolating_headers() {
        let host = ExtensionUiHost::new(Arc::new(CapabilityRegistry::new()), "alice".to_string());
        let mut assets = HashMap::new();
        assets.insert("index.html".to_string(), html_asset("<html>hi</html>"));
        host.register(
            "acme/widget".to_string(),
            RegisteredUiExtension::new(PermissionSet::none(), assets).unwrap(),
        )
        .await;

        let app = router(host);
        let req = axum::http::Request::builder()
            .uri("/acme%2Fwidget/index.html")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = tower::ServiceExt::oneshot(app, req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let csp = resp
            .headers()
            .get(header::CONTENT_SECURITY_POLICY)
            .expect("CSP header must be present")
            .to_str()
            .unwrap();
        assert!(csp.contains("sandbox"));
        assert!(
            !csp.contains("allow-same-origin"),
            "sandbox MUST NOT include allow-same-origin — that would defeat the isolation: {csp}"
        );
    }

    #[tokio::test]
    async fn asset_path_traversal_is_denied() {
        let host = ExtensionUiHost::new(Arc::new(CapabilityRegistry::new()), "alice".to_string());
        let mut assets = HashMap::new();
        assets.insert("index.html".to_string(), html_asset("<html></html>"));
        host.register(
            "acme/widget".to_string(),
            RegisteredUiExtension::new(PermissionSet::none(), assets).unwrap(),
        )
        .await;

        assert!(host
            .asset("acme/widget", "../../etc/passwd")
            .await
            .is_none());
        assert!(host.asset("acme/widget", "/etc/passwd").await.is_none());
    }

    /// Cross-extension confinement: extension B's assets are never
    /// reachable by asking for extension A's id, and vice versa.
    #[tokio::test]
    async fn cross_extension_assets_are_not_reachable() {
        let host = ExtensionUiHost::new(Arc::new(CapabilityRegistry::new()), "alice".to_string());
        let mut a_assets = HashMap::new();
        a_assets.insert("secret.html".to_string(), html_asset("A's secret"));
        host.register(
            "acme/a".to_string(),
            RegisteredUiExtension::new(PermissionSet::none(), a_assets).unwrap(),
        )
        .await;
        host.register(
            "acme/b".to_string(),
            RegisteredUiExtension::new(PermissionSet::none(), HashMap::new()).unwrap(),
        )
        .await;

        assert!(host.asset("acme/a", "secret.html").await.is_some());
        assert!(
            host.asset("acme/b", "secret.html").await.is_none(),
            "extension b must not see extension a's registered asset"
        );
    }

    /// Adversarial vector (a) — CLD-08's own wording: extension UI trying to
    /// execute a call outside its declared `PermissionSet` is blocked with a
    /// typed error, never silently reaching the real registry.
    #[tokio::test]
    async fn invoke_outside_permission_set_is_blocked_with_typed_error() {
        let host = ExtensionUiHost::new(Arc::new(CapabilityRegistry::new()), "alice".to_string());
        host.register(
            "acme/widget".to_string(),
            RegisteredUiExtension::new(PermissionSet::none(), HashMap::new()).unwrap(),
        )
        .await;

        let err = host
            .invoke("acme/widget", "some:capability", serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(matches!(err, ExtensionError::CapabilityNotDeclared { .. }));
    }

    /// Adversarial vector (b) — a completely unregistered/unknown extension
    /// id can never be used to reach ANY capability.
    #[tokio::test]
    async fn invoke_for_unknown_extension_is_blocked() {
        let host = ExtensionUiHost::new(Arc::new(CapabilityRegistry::new()), "alice".to_string());
        let err = host
            .invoke("never/registered", "anything", serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(matches!(err, ExtensionError::NotFound { .. }));
    }

    #[tokio::test]
    async fn invoke_within_declared_permission_set_reaches_the_real_registry() {
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
                    egress: EgressScope::None,
                    ..PermissionSet::none()
                },
                HashMap::new(),
            )
            .unwrap(),
        )
        .await;

        let result = host
            .invoke(
                "acme/widget",
                "acme/echo",
                serde_json::json!({"hello": "world"}),
            )
            .await
            .expect("declared capability must reach the real registry");
        assert_eq!(result.data, serde_json::json!({"echo": {"hello": "world"}}));
    }

    /// Minimal in-process capability for `invoke_within_declared_permission_set_reaches_the_real_registry`.
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
            _ctx: &InvokeCtx,
        ) -> anyhow::Result<serde_json::Value> {
            Ok(serde_json::json!({"echo": args}))
        }
        fn is_local(&self) -> bool {
            true
        }
    }
}
