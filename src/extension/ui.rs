//! Extension UI isolation: an extension may `provides: Ui`
//! (`bastion_extension_protocol::Provided::Ui`, declared by the protocol
//! crate but not wired to a serving mechanism until this module).
//! Constraint (non-negotiable):
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
//! already uses for the non-UI mechanisms.
//!
//! # Mounting
//!
//! [`router`] is mounted by `main.rs` onto the daemon's axum router (the
//! third pre-built router `channel::webhook::serve_with_mesh` merges,
//! distinct from `/ui`'s dashboard route and `/app`'s embedded SPA), behind
//! two gates:
//!
//! - `[extension_ui] enabled` in `bastion.toml`, default false — an existing
//!   deployment gains no new surface by upgrading;
//! - `operational::require_daemon_access`, the same fail-closed
//!   `BASTION_DAEMON_TOKEN` bearer check `/lifecycle/*` uses — with no token
//!   configured every request is refused, because `/invoke` below reaches the
//!   real `CapabilityRegistry`.
//!
//! The gate lives at the mount site rather than in this module: this module
//! owns the isolation contract (what a served bundle may do), the layer owns
//! network reachability (who may talk to the surface at all).
//!
//! A mounted host starts empty — [`ExtensionUiHost::register`] is the entry
//! point an install path for `provides: Ui` extensions calls, and until
//! something registers a bundle, assets 404 and `/invoke` answers
//! [`ExtensionError::NotFound`].
//!
//! ## Per-bundle invoke credential
//!
//! Sandboxed script runs in an opaque origin and therefore cannot be handed
//! the operator's daemon token — so `serve_asset` mints a fresh,
//! short-lived, per-extension credential every time it serves an HTML asset
//! and injects it into the page as `<meta name="bastion-invoke-token"
//! content="...">`, right after `<head>`. A served bundle reads that tag and
//! sends the value back as the `x-bastion-ext-invoke-token` header on every
//! `POST /invoke` call; [`invoke_handler`] rejects the request with `401`
//! before it ever reaches [`ExtensionUiHost::invoke`] if the header is
//! missing, unknown, expired, or was minted for a DIFFERENT extension id.
//!
//! This is deliberately a network-reachability concern, not an authority
//! one — same split as the `[extension_ui] enabled` + daemon-token mount
//! gate above: the credential proves "this caller recently loaded THIS
//! bundle from THIS host," nothing more. It carries no scope of its own and
//! never widens what `/invoke` may do — [`ExtensionUiHost::invoke`]'s
//! `PermissionSet.allows_capability` check is unchanged and remains the
//! sole authority gate, checked identically whether or not the invoke
//! credential exists. Revoking is automatic: [`ExtensionUiHost::deregister`]
//! purges every live credential for that extension id, so
//! uninstalling/disabling an extension mid-session invalidates any page
//! still holding one.
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
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use bastion_extension_protocol::{ExtensionError, PermissionSet};
use bastion_memory::PrivacyTier;
use bastion_runtime::capability::{CapabilityRegistry, InvokeCtx};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

fn now_nanos() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as i64
}

/// How long a minted invoke credential stays valid. Long enough that a
/// human keeping a tab open doesn't get logged out mid-session; short
/// enough that a leaked one has a bounded useful window.
const INVOKE_CREDENTIAL_TTL_NANOS: i64 = 30 * 60 * 1_000_000_000;

/// Header a served bundle presents its invoke credential on.
const INVOKE_TOKEN_HEADER: &str = "x-bastion-ext-invoke-token";

/// Prefix on a minted invoke credential — visually distinct from other
/// token families in this codebase (`bcp_` Control Plane credentials,
/// `bcpr_` retrieval references) so a log line never confuses the three.
const INVOKE_TOKEN_PREFIX: &str = "extinv_";

/// One minted, per-bundle invoke credential — see the module doc's "Per-
/// bundle invoke credential" section. Carries no permissions of its own;
/// [`ExtensionUiHost::invoke`]'s `PermissionSet` check is the only authority
/// gate. `extension_id` is checked on presentation so a credential minted
/// while serving extension A's page can never authenticate a call against
/// extension B, even though B's own `PermissionSet` lookup would already
/// separately reject an unrelated capability name.
struct InvokeCredential {
    extension_id: String,
    expires_at_nanos: i64,
}

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
    invoke_credentials: RwLock<HashMap<String, InvokeCredential>>,
}

impl ExtensionUiHost {
    pub fn new(registry: Arc<CapabilityRegistry>, owner: String) -> Arc<Self> {
        Arc::new(Self {
            registry,
            owner,
            privacy_tier: Some(PrivacyTier::CloudOk),
            extensions: RwLock::new(HashMap::new()),
            invoke_credentials: RwLock::new(HashMap::new()),
        })
    }

    pub async fn register(&self, extension_id: String, ui: RegisteredUiExtension) {
        self.extensions.write().await.insert(extension_id, ui);
    }

    /// Removes the registered bundle AND purges every live invoke credential
    /// minted for it — an uninstalled/disabled extension's credentials stop
    /// working immediately, even for a page still holding one from before.
    pub async fn deregister(&self, extension_id: &str) {
        self.extensions.write().await.remove(extension_id);
        self.invoke_credentials
            .write()
            .await
            .retain(|_, cred| cred.extension_id != extension_id);
    }

    async fn asset(&self, extension_id: &str, path: &str) -> Option<(String, Vec<u8>)> {
        if !is_safe_relative_path(path) {
            return None;
        }
        let extensions = self.extensions.read().await;
        let ext = extensions.get(extension_id)?;
        ext.assets.get(path).cloned()
    }

    /// Mint a fresh invoke credential for `extension_id`, valid for
    /// [`INVOKE_CREDENTIAL_TTL_NANOS`]. Called once per HTML asset serve —
    /// see [`serve_asset`]; every load of the bundle's entry page gets its
    /// own credential rather than reusing a stale one indefinitely.
    async fn mint_invoke_credential(&self, extension_id: &str) -> String {
        let mut bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut bytes);
        let token = format!(
            "{INVOKE_TOKEN_PREFIX}{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
        );
        self.invoke_credentials.write().await.insert(
            token.clone(),
            InvokeCredential {
                extension_id: extension_id.to_string(),
                expires_at_nanos: now_nanos().saturating_add(INVOKE_CREDENTIAL_TTL_NANOS),
            },
        );
        token
    }

    /// Whether `presented` is a live credential minted for exactly
    /// `extension_id`. Deliberately does not consume/rotate on a successful
    /// check — a page issues many `/invoke` calls over its lifetime with the
    /// same credential, unlike the one-time-use references elsewhere in
    /// this codebase (`control_plane::routes::PendingIssuedTokens`).
    async fn check_invoke_credential(&self, extension_id: &str, presented: &str) -> bool {
        match self.invoke_credentials.read().await.get(presented) {
            Some(cred) => cred.extension_id == extension_id && now_nanos() <= cred.expires_at_nanos,
            None => false,
        }
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
            allowed_tools: None,
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

/// Insert `<meta name="bastion-invoke-token" content="{token}">` right
/// after the first `<head>`, or prepend it if the document has none
/// (browsers tolerate a leading `<meta>` on a head-less fragment). Non-UTF-8
/// bytes are served unmodified — a binary asset misregistered with an
/// `text/html` content-type should not panic or corrupt.
fn inject_invoke_token(bytes: Vec<u8>, token: &str) -> Vec<u8> {
    let meta = format!("<meta name=\"bastion-invoke-token\" content=\"{token}\">");
    match String::from_utf8(bytes) {
        Ok(mut html) => {
            match html.find("<head>") {
                Some(pos) => html.insert_str(pos + "<head>".len(), &meta),
                None => html.insert_str(0, &meta),
            }
            html.into_bytes()
        }
        Err(e) => e.into_bytes(),
    }
}

/// `GET /ext-ui/{id}/{*path}` — serves one static asset, isolated. An HTML
/// asset also gets a fresh invoke credential injected (see the module doc's
/// "Per-bundle invoke credential" section) — every other content type is
/// served byte-for-byte as registered.
async fn serve_asset(
    State(host): State<Arc<ExtensionUiHost>>,
    Path((id, path)): Path<(String, String)>,
) -> impl IntoResponse {
    match host.asset(&id, &path).await {
        Some((content_type, bytes)) => {
            let bytes = if content_type.starts_with("text/html") {
                let token = host.mint_invoke_credential(&id).await;
                inject_invoke_token(bytes, &token)
            } else {
                bytes
            };
            (
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, content_type),
                    (
                        header::CONTENT_SECURITY_POLICY,
                        "sandbox allow-scripts; default-src 'self'; frame-ancestors 'self'"
                            .to_string(),
                    ),
                    (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
                ],
                bytes,
            )
                .into_response()
        }
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

/// `POST /ext-ui/{id}/invoke` — the one mediated bridge back to the backend.
/// Requires a live [`InvokeCredential`] minted for THIS `id` on the
/// `x-bastion-ext-invoke-token` header — checked here, at the transport
/// layer, before [`ExtensionUiHost::invoke`] is ever called: same "the gate
/// lives at the mount site, this module owns the isolation contract" split
/// the module doc already draws for network reachability vs. authority.
async fn invoke_handler(
    State(host): State<Arc<ExtensionUiHost>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<InvokeRequest>,
) -> impl IntoResponse {
    let presented = headers
        .get(INVOKE_TOKEN_HEADER)
        .and_then(|v| v.to_str().ok());
    let authorized = match presented {
        Some(token) => host.check_invoke_credential(&id, token).await,
        None => false,
    };
    if !authorized {
        tracing::warn!(
            event = "extension_ui_invoke_missing_or_invalid_credential",
            extension = %id,
        );
        return (
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                error: "missing or invalid invoke credential".to_string(),
            }),
        )
            .into_response();
    }

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

    // ---- invoke credential (extension UI invoke-credential debt) ---------

    #[test]
    fn inject_invoke_token_inserts_right_after_head() {
        let html = "<html><head><title>t</title></head><body>hi</body></html>";
        let out = inject_invoke_token(html.as_bytes().to_vec(), "extinv_abc");
        let out = String::from_utf8(out).unwrap();
        assert!(out.starts_with(
            "<html><head><meta name=\"bastion-invoke-token\" content=\"extinv_abc\">"
        ));
        assert!(out.contains("<title>t</title>"));
    }

    #[test]
    fn inject_invoke_token_prepends_when_no_head_tag() {
        let html = "<body>fragment only</body>";
        let out = inject_invoke_token(html.as_bytes().to_vec(), "extinv_abc");
        let out = String::from_utf8(out).unwrap();
        assert!(out.starts_with("<meta name=\"bastion-invoke-token\" content=\"extinv_abc\">"));
        assert!(out.ends_with("<body>fragment only</body>"));
    }

    #[test]
    fn inject_invoke_token_passes_through_non_utf8_bytes_unmodified() {
        let bytes = vec![0xff, 0xfe, 0x00, 0x01];
        let out = inject_invoke_token(bytes.clone(), "extinv_abc");
        assert_eq!(
            out, bytes,
            "non-UTF-8 content must be served as-is, never corrupted"
        );
    }

    #[tokio::test]
    async fn mint_invoke_credential_produces_distinct_tokens_with_the_right_prefix() {
        let host = ExtensionUiHost::new(Arc::new(CapabilityRegistry::new()), "alice".to_string());
        let a = host.mint_invoke_credential("acme/widget").await;
        let b = host.mint_invoke_credential("acme/widget").await;
        assert!(a.starts_with("extinv_"));
        assert_ne!(a, b, "each mint must produce a fresh, unpredictable token");
    }

    #[tokio::test]
    async fn check_invoke_credential_accepts_the_right_extension_and_rejects_others() {
        let host = ExtensionUiHost::new(Arc::new(CapabilityRegistry::new()), "alice".to_string());
        let token = host.mint_invoke_credential("acme/widget").await;

        assert!(host.check_invoke_credential("acme/widget", &token).await);
        assert!(
            !host.check_invoke_credential("acme/other", &token).await,
            "a token minted for one extension must not authenticate another"
        );
        assert!(
            !host
                .check_invoke_credential("acme/widget", "extinv_never-issued")
                .await,
            "an unknown token must not authenticate"
        );
    }

    #[tokio::test]
    async fn an_expired_invoke_credential_is_rejected() {
        let host = ExtensionUiHost::new(Arc::new(CapabilityRegistry::new()), "alice".to_string());
        let token = "extinv_manually-inserted".to_string();
        host.invoke_credentials.write().await.insert(
            token.clone(),
            InvokeCredential {
                extension_id: "acme/widget".to_string(),
                expires_at_nanos: now_nanos() - 1,
            },
        );
        assert!(!host.check_invoke_credential("acme/widget", &token).await);
    }

    #[tokio::test]
    async fn deregister_purges_that_extensions_invoke_credentials_only() {
        let host = ExtensionUiHost::new(Arc::new(CapabilityRegistry::new()), "alice".to_string());
        let token_a = host.mint_invoke_credential("acme/a").await;
        let token_b = host.mint_invoke_credential("acme/b").await;

        host.deregister("acme/a").await;

        assert!(!host.check_invoke_credential("acme/a", &token_a).await);
        assert!(
            host.check_invoke_credential("acme/b", &token_b).await,
            "deregistering one extension must not touch another's live credentials"
        );
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
