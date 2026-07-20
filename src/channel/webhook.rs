// Webhook channel — POST /webhook, GET /events, POST /mesh/ingest, POST /auth/exchange, POST /mesh/pair.
//
// Security: owner is resolved from a trusted auth-token→owner_id map (CR-03).
// The request body MUST NOT control owner identity. Unknown tokens → 401.
// Errors are mapped to non-2xx status codes without leaking internal detail (CR-05).
use crate::channel::{Channel, OwnerMap};
use axum::http::StatusCode;
use bastion_mesh::mesh::{MeshPeer, MeshPeerMap};
use bastion_runtime::agent::handle::AgentHandle;
use bastion_types::BastionError;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};

/// Webhook channel: accepts `POST /webhook` and returns the agent reply as JSON (CHAN-03).
pub struct WebhookChannel {
    pub(crate) addr: String,
    pub(crate) default_persona: Option<String>,
    /// Trusted auth-token → owner_id map. Unknown tokens are rejected with 401.
    pub(crate) owner_map: OwnerMap,
}

impl WebhookChannel {
    pub fn new(addr: impl Into<String>) -> Self {
        Self {
            addr: addr.into(),
            default_persona: None,
            owner_map: OwnerMap::default(),
        }
    }

    pub fn with_default_persona(mut self, persona: impl Into<String>) -> Self {
        self.default_persona = Some(persona.into());
        self
    }

    /// Configure the trusted token→owner map. Without this, all requests are rejected.
    pub fn with_owner_map(mut self, map: OwnerMap) -> Self {
        self.owner_map = map;
        self
    }
}

#[async_trait::async_trait]
impl Channel for WebhookChannel {
    async fn run(self: Box<Self>, agent: AgentHandle) -> anyhow::Result<()> {
        let (events_tx, _) = broadcast::channel::<String>(128);
        let mesh_peer_map = Arc::new(RwLock::new(MeshPeerMap::new()));
        // WR-01: fail-closed — refuse to start if APP_JWT_SECRET is not set.
        // Falling back to a hardcoded default is a silent auth bypass once CR-01 is fixed.
        let jwt_secret = std::env::var("APP_JWT_SECRET").map_err(|_| {
            tracing::error!(
                event = "webhook_no_jwt_secret",
                "APP_JWT_SECRET is not set — refusing to start webhook channel (WR-01: silent default is an auth bypass)"
            );
            anyhow::anyhow!("APP_JWT_SECRET must be set; refusing to start with hardcoded default")
        })?;
        serve(
            agent,
            &self.addr,
            self.owner_map,
            events_tx,
            mesh_peer_map,
            jwt_secret,
        )
        .await
    }

    fn default_persona(&self) -> Option<&str> {
        self.default_persona.as_deref()
    }
}

// ─── axum handler ────────────────────────────────────────────────────────────

use axum::{
    extract::{Query, State},
    http::HeaderMap,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse,
    },
    routing::{get, post},
    Json, Router,
};
use base64::Engine;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::convert::Infallible;
use tokio_stream::wrappers::BroadcastStream;

/// A one-time pairing grant binds a human owner to a device label.
///
/// `owner_id` is the canonical identity used for memories, goals and sessions.
/// `device_name` is presentation/audit metadata only and MUST NOT become an owner.
#[derive(Clone, Debug)]
pub struct PairingGrant {
    pub owner_id: String,
    pub device_name: String,
    pub issued_at: std::time::Instant,
}

/// Public type alias for the OTC store shared between the webhook server and commands.
///
/// Skill commands insert a code like this:
///   otc_store.write().await.insert(
///       "BAST-XXXX".to_string(),
///       PairingGrant {
///           owner_id: "mario".to_string(),
///           device_name: "laptop".to_string(),
///           issued_at: std::time::Instant::now(),
///       },
///   );
/// The code is consumed by /auth/exchange or /mesh/pair within 5 minutes (CR-02).
pub type OtcStore = Arc<RwLock<std::collections::HashMap<String, PairingGrant>>>;

/// Create a new empty OtcStore. Pass to serve_with_mesh so skills can insert codes.
pub fn new_otc_store() -> OtcStore {
    Arc::new(RwLock::new(std::collections::HashMap::new()))
}

/// JWT claims — sub is the device_name / owner identifier issued at /auth/exchange.
/// Used both for signing (auth_exchange_handler) and verification (resolve_owner_or_401).
#[derive(serde::Serialize, serde::Deserialize)]
struct Claims {
    sub: String,
    device: String,
    exp: u64,
}

/// Webhook request body. Owner is NOT accepted here — use the auth-token header (CR-03).
#[derive(Deserialize)]
struct In {
    text: String,
}

#[derive(Serialize)]
struct Out {
    reply: String,
}

/// Shared state threaded through the axum handlers.
#[derive(Clone)]
struct AppState {
    agent: AgentHandle,
    owner_map: Arc<OwnerMap>,
    /// SSE broadcast channel — capacity=128.
    events_tx: broadcast::Sender<String>,
    /// Registry of known mesh peers (owner_id → peer). Populated from bastion.toml at startup.
    mesh_peer_map: Arc<RwLock<MeshPeerMap>>,
    /// OTC store: token → canonical owner + device metadata. 5-min TTL.
    otc_store: OtcStore,
    /// JWT signing secret for /auth/exchange (HS256).
    jwt_secret: String,
    /// Pluggable mesh transport (P2PTransport or relay). None if mesh not configured.
    mesh_transport: Option<bastion_mesh::mesh::SharedMeshTransport>,
    /// In-memory store of received mesh slices, keyed by from_owner.
    /// Updated by ingest_handler; read by MeshSliceProvider (SEAM #2).
    mesh_slice_store: Option<bastion_mesh::mesh::context_provider::MeshSliceStore>,
    /// Agent identity for Agent Card endpoint (SEC-06). None = /agent-card returns 404.
    agent_identity: Option<std::sync::Arc<bastion_mesh::identity::age_identity::AgeIdentity>>,
    /// Human-readable agent name for Agent Card.
    agent_name: String,
    /// WhatsApp Cloud API config (CHAN-01). None = /whatsapp/webhook routes reject
    /// with 404/403 rather than panicking — WhatsApp is opt-in per instance.
    whatsapp: Option<crate::channel::whatsapp::WhatsAppConfig>,
    /// Composio OAuth client (SEC-03). None = /auth/composio/callback rejects with
    /// 501 rather than panicking — Composio integration is opt-in per instance
    /// (requires COMPOSIO_API_KEY).
    composio_oauth: Option<std::sync::Arc<bastion_mcp::oauth::ComposioOAuth>>,
    /// Loop 3-D (`docs/revamp/C3-cloud-ready-design.md`): boot-sequence
    /// readiness gate backing `/readyz` — extracted via `FromRef` below so
    /// `operational::readiness_handler` (`State<Arc<ReadinessState>>`)
    /// mounts directly onto this SAME `Router<AppState>`.
    readiness: std::sync::Arc<crate::channel::operational::ReadinessState>,
    /// Loop 3-D: daemon-access-gated stop/reload control, extracted via
    /// `FromRef` for `operational::lifecycle_*_handler`.
    lifecycle: crate::channel::operational::LifecycleControl,
    /// Fase 2.9: backs `GET /status` (`operational::status_handler`) via the
    /// `FromRef<AppState> for operational::StatusState` impl below.
    runtime_registry: bastion_runtime::agent::backend::RuntimeRegistry,
    /// Fase 2.9: same `[auth.*]` table `AuthProfileRegistry`/`backend_command`
    /// use — `/status` probes it live (booleans only, never account detail).
    auth: crate::config::AuthConfig,
    /// Cached public-release status. It is informational only; no HTTP or
    /// channel request can apply an update from inside the container.
    updates: crate::update::SharedUpdateState,
}

impl axum::extract::FromRef<AppState> for crate::channel::operational::StatusState {
    fn from_ref(state: &AppState) -> Self {
        crate::channel::operational::StatusState {
            runtime_registry: state.runtime_registry.clone(),
            auth: state.auth.clone(),
            readiness: state.readiness.clone(),
            updates: state.updates.clone(),
        }
    }
}

impl axum::extract::FromRef<AppState>
    for std::sync::Arc<crate::channel::operational::ReadinessState>
{
    fn from_ref(state: &AppState) -> Self {
        state.readiness.clone()
    }
}

impl axum::extract::FromRef<AppState> for crate::channel::operational::LifecycleControl {
    fn from_ref(state: &AppState) -> Self {
        state.lifecycle.clone()
    }
}

/// Categorize an anyhow error for safe HTTP status mapping.
/// NEVER include the error message in the response body — only log it.
///
/// Matches typed BastionError variants — no string-prefix detection (WR-09).
pub fn error_status(e: &anyhow::Error) -> StatusCode {
    // Walk the error chain looking for BastionError variants
    if let Some(be) = e.downcast_ref::<BastionError>() {
        return match be {
            BastionError::PrivacyEgressBlocked => StatusCode::FORBIDDEN,
            BastionError::BudgetExceeded => StatusCode::TOO_MANY_REQUESTS,
            BastionError::InputGuardrailRejected(_) => StatusCode::BAD_REQUEST,
            BastionError::ApprovalDenied { .. } => StatusCode::FORBIDDEN,
            BastionError::BackendUnavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
    }
    StatusCode::INTERNAL_SERVER_ERROR
}

/// Fase 2.8: the response BODY companion to `error_status` — a hand-picked
/// whitelist by TYPED variant (never a string-prefix check, same discipline
/// as `error_status`/WR-09) of which errors are safe to echo verbatim to the
/// client. `BackendUnavailable`/`BudgetExceeded`/`PrivacyEgressBlocked`/
/// `ApprovalDenied` carry no secret material and are actionable ("switch
/// backend", "wait for budget reset", ...) — everything else, INCLUDING
/// `InputGuardrailRejected` (whose detail string is explicitly documented as
/// "MUST NOT be echoed to the client" in `bastion-types`), collapses to a
/// generic "internal error" so a future variant can never leak by omission.
pub fn error_body(e: &anyhow::Error) -> String {
    if let Some(be) = e.downcast_ref::<BastionError>() {
        match be {
            BastionError::BackendUnavailable(_)
            | BastionError::BudgetExceeded
            | BastionError::PrivacyEgressBlocked
            | BastionError::ApprovalDenied { .. } => return be.to_string(),
            _ => {}
        }
    }
    "internal error".to_string()
}

/// Resolve owner from x-bastion-token header. Returns None + 401 response on miss.
/// Pattern from CR-03. All protected routes MUST use this.
///
/// Resolution order:
///   1. Try JWT decode (HS256, signed with jwt_secret) → use sub claim as owner_id (CR-01).
///   2. Fall back to static owner_map lookup (pre-existing CLI/API tokens, backward compat).
///   3. Reject with 401 if both fail.
// Err is boxed: clippy::result_large_err — axum::response::Response is 128+ bytes,
// and the Ok path (a plain owner String) is the common case.
fn resolve_owner_or_401(
    headers: &HeaderMap,
    owner_map: &OwnerMap,
    jwt_secret: &str,
    event_name: &'static str,
) -> Result<String, Box<axum::response::Response>> {
    let token = headers
        .get("x-bastion-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    // First try JWT decode (mobile app tokens issued by /auth/exchange). CR-01.
    let validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::HS256);
    if let Ok(data) = jsonwebtoken::decode::<Claims>(
        token,
        &jsonwebtoken::DecodingKey::from_secret(jwt_secret.as_bytes()),
        &validation,
    ) {
        return Ok(data.claims.sub);
    }

    // Fallback: static owner_map (pre-existing non-JWT tokens — CLI / API keys).
    match owner_map.resolve(token) {
        Some(o) => Ok(o.to_owned()),
        None => {
            tracing::warn!(event = event_name, "unknown or missing x-bastion-token");
            Err(Box::new(
                (StatusCode::UNAUTHORIZED, Json(serde_json::json!({}))).into_response(),
            ))
        }
    }
}

/// Mock cockpit data for mobile validation. Opt-in via BASTION_MOCK_COCKPIT=1.
/// Lets the app's cockpit (drift/goals/memories) and chat HUD populate with
/// deterministic data without a fully wired goal/memory backend yet.
fn mock_cockpit_reply(text: &str) -> Option<String> {
    if std::env::var("BASTION_MOCK_COCKPIT").is_err() {
        return None;
    }
    let t = text.trim();
    match t {
        "/drift" => Some(
            "drift estável (75%) — sem sinais de deriva.\n2 metas ativas, nenhuma em risco."
                .to_string(),
        ),
        "/goals" => Some(
            "2/5 metas ativas\n- Lançar v1.0 no GitHub (80%)\n- Revisar orçamento mensal (30%)"
                .to_string(),
        ),
        "/memories" => Some(
            "1: Mario prefere café sem açúcar\n2: Trabalha com IA e agentes\n3: Acorda cedo pra treinar"
                .to_string(),
        ),
        _ if t.starts_with("/contest ") => Some(format!(
            "Memória '{}' contestada e revogada.",
            t.trim_start_matches("/contest ").trim()
        )),
        _ => None,
    }
}

/// POST /webhook — resolve owner from trusted token header, forward to AgentHandle.
async fn handle(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(p): Json<In>,
) -> impl IntoResponse {
    // CR-03: owner comes from a trusted header map, never from the request body.
    let owner = match resolve_owner_or_401(
        &headers,
        &state.owner_map,
        &state.jwt_secret,
        "webhook_unauthorized",
    ) {
        Ok(o) => o,
        Err(resp) => return *resp,
    };

    // Cockpit mock interceptor (opt-in) — return deterministic data for the
    // /drift, /goals, /memories, /contest commands the mobile cockpit sends.
    if let Some(reply) = mock_cockpit_reply(&p.text) {
        return Json(Out { reply }).into_response();
    }

    // CR-05: map errors to correct HTTP status; never return 200 on denial.
    match state.agent.ask(p.text, owner).await {
        Ok(reply) => Json(Out { reply }).into_response(),
        Err(e) => {
            let status = error_status(&e);
            // Fase 2.8: `error_body` is a hand-picked, typed-variant
            // whitelist (WR-09 discipline) — safe to include in the
            // response now, unlike the previous always-empty `{}` (problem
            // #12, "erro 500 vazio"). Full detail still only ever goes to
            // the log, never the client.
            let body = error_body(&e);
            tracing::warn!(event = "webhook_turn_error", status = %status, error = %e, "turn failed");
            (status, Json(serde_json::json!({ "error": body }))).into_response()
        }
    }
}

/// GET /events — real-time SSE feed.
/// CR-03: same x-bastion-token auth as /webhook.
/// BroadcastStream capacity=128; lagged receivers get Err which is filtered out.
async fn sse_handler(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let _owner = match resolve_owner_or_401(
        &headers,
        &state.owner_map,
        &state.jwt_secret,
        "sse_unauthorized",
    ) {
        Ok(o) => o,
        Err(resp) => return *resp,
    };
    let rx = state.events_tx.subscribe();
    let stream = BroadcastStream::new(rx)
        .filter_map(|r| async { r.ok() })
        .map(|msg| Ok::<_, Infallible>(Event::default().data(msg)));
    Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(std::time::Duration::from_secs(30)))
        .into_response()
}

/// POST /mesh/ingest — receive encrypted MeshEnvelope from a peer daemon.
///
/// Decrypts the envelope via transport.receive() (age E2E decrypt + from_owner verification).
/// On success, stores the slice in mesh_slice_store so MeshSliceProvider can inject it
/// into the system prompt on the next agent turn (SEAM #2).
/// CR-03: auth via x-bastion-token enforced — unauthenticated callers get 401.
async fn ingest_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    // CR-03: enforce auth BEFORE body deserialization. Taking the raw body (not
    // Json<...>) prevents Axum's Json extractor from rejecting an unauthenticated
    // request with 415 before resolve_owner_or_401 ever runs (#mesh-ingest-401).
    let _owner = match resolve_owner_or_401(
        &headers,
        &state.owner_map,
        &state.jwt_secret,
        "mesh_ingest_unauthorized",
    ) {
        Ok(o) => o,
        Err(resp) => return *resp,
    };
    let envelope: bastion_mesh::mesh::MeshEnvelope = match serde_json::from_slice(&body) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(event = "mesh_ingest_bad_body", error = %e);
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": format!("invalid envelope: {e}") })),
            )
                .into_response();
        }
    };

    // CR-06: reject envelopes not addressed to this node's owner.
    // local_owner is the value MESH_OWNER_ID (or BASTION_OWNER_ID) was set to at startup.
    // This check is belt-and-suspenders: P2PTransport::receive() also asserts to_owner,
    // but we guard here to return 403 before spending CPU on decryption.
    if let Ok(local_owner) =
        std::env::var("MESH_OWNER_ID").or_else(|_| std::env::var("BASTION_OWNER_ID"))
    {
        if envelope.to_owner != local_owner {
            tracing::warn!(
                event = "mesh_ingest_wrong_owner",
                to_owner = %envelope.to_owner,
                local_owner = %local_owner,
            );
            return (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({ "error": "envelope addressed to wrong owner" })),
            )
                .into_response();
        }
    }

    let transport = match &state.mesh_transport {
        Some(t) => t.clone(),
        None => {
            tracing::warn!(
                event = "mesh_ingest_no_transport",
                "mesh transport not configured"
            );
            return (
                StatusCode::NOT_IMPLEMENTED,
                Json(serde_json::json!({ "error": "mesh not configured" })),
            )
                .into_response();
        }
    };
    match transport.receive(envelope).await {
        Ok(slice) => {
            tracing::info!(event = "mesh_ingest_ok", from_owner = %slice.from_owner, count = slice.beliefs.len());
            // Update MeshSliceStore so MeshSliceProvider picks it up on next turn (SEAM #2)
            if let Some(store) = &state.mesh_slice_store {
                let mut s = store.write().await;
                s.insert(slice.from_owner.clone(), slice.beliefs.clone());
            }
            (
                StatusCode::OK,
                Json(serde_json::json!({ "status": "accepted", "beliefs": slice.beliefs.len() })),
            )
                .into_response()
        }
        Err(e) => {
            tracing::warn!(event = "mesh_ingest_error", error = %e);
            (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response()
        }
    }
}

/// POST /auth/exchange { otc: "BAST-XXXX" } → { jwt, device_name }
///
/// Exchange a one-time code (generated by /connect-app command) for a JWT.
/// OTC TTL: 5 minutes. OTC validated against otc_store; deleted on successful exchange.
/// JWT signed with jwt_secret (HS256). No x-bastion-token required — this IS the auth entry point.
async fn auth_exchange_handler(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let otc = match body.get("otc").and_then(|v| v.as_str()) {
        Some(s) => s.to_string(),
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "missing otc" })),
            )
                .into_response()
        }
    };

    // Validate OTC against store (5-min TTL)
    let result = {
        let store = state.otc_store.read().await;
        store.get(&otc).map(|grant| {
            let elapsed = grant.issued_at.elapsed();
            (grant.clone(), elapsed)
        })
    };

    match result {
        Some((grant, elapsed)) if elapsed.as_secs() < 300 => {
            // OTC valid — consume it (delete from store)
            state.otc_store.write().await.remove(&otc);

            // Issue JWT (HS256, 90-day expiry).
            // JWT subject is always the canonical owner. The device is metadata.
            // The issued JWT IS the x-bastion-token used on subsequent requests.
            use jsonwebtoken::{encode, EncodingKey, Header};
            let exp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
                + 90 * 24 * 3600; // 90 days
            let claims = Claims {
                sub: grant.owner_id.clone(),
                device: grant.device_name.clone(),
                exp,
            };
            match encode(
                &Header::default(),
                &claims,
                &EncodingKey::from_secret(state.jwt_secret.as_bytes()),
            ) {
                Ok(jwt) => (
                    StatusCode::OK,
                    Json(serde_json::json!({
                        "jwt": jwt,
                        "owner_id": &grant.owner_id,
                        "device_name": &grant.device_name
                    })),
                )
                    .into_response(),
                Err(e) => {
                    tracing::error!(event = "auth_exchange_jwt_error", error = %e);
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({ "error": "jwt encoding failed" })),
                    )
                        .into_response()
                }
            }
        }
        Some(_) => {
            // OTC expired — consume it anyway to prevent retry.
            // WR-03: return same body as unknown-OTC to prevent enumeration oracle.
            state.otc_store.write().await.remove(&otc);
            tracing::warn!(event = "auth_exchange_expired_otc");
            (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({ "error": "invalid OTC" })),
            )
                .into_response()
        }
        None => {
            tracing::warn!(event = "auth_exchange_invalid_otc");
            (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({ "error": "invalid OTC" })),
            )
                .into_response()
        }
    }
}

/// POST /auth/composio/callback body — mirrors the OTC-exchange precedent (D-06):
/// Composio's own redirect flow calls this endpoint with the resulting
/// `connected_account_id` once the owner authorizes in their browser.
///
/// SEC-03 forgery fix (T-11-06-01, security review finding): this body deliberately
/// carries NO `owner`/`toolkit` fields. Earlier revisions accepted them directly from
/// the request body and trusted them verbatim — meaning anyone who could reach this
/// endpoint could POST an arbitrary `{owner, toolkit, connected_account_id}` and bind
/// a connection to any owner of their choosing (OAuth callback forgery / IDOR). `state`
/// is the CSPRNG nonce `ComposioOAuth::initiate()` minted and persisted server-side;
/// `owner`/`toolkit` are now derived EXCLUSIVELY from that server-side record via
/// `ComposioOAuth::consume_state` (single-use, delete-on-consume) — never from
/// anything the caller supplies.
#[derive(Deserialize)]
struct ComposioCallbackBody {
    state: String,
    connected_account_id: String,
}

/// POST /auth/composio/callback { state, connected_account_id } → 200.
///
/// Persists ONLY Composio's own `connected_account_id` reference (never a raw
/// third-party OAuth token — T-11-06-02) via `ComposioOAuth::store_connection`.
/// Mirrors `auth_exchange_handler`'s shape: validate → act → respond, never leaking
/// internal error detail in the body (CR-05), always logging via `tracing`.
///
/// T-11-06-01 (OAuth callback forgery — security review finding, HIGH): `owner`/
/// `toolkit` are resolved via `ComposioOAuth::consume_state(&payload.state)` — a
/// single-use, CSPRNG-bound, TTL-limited lookup — NEVER trusted from the request
/// body. A missing/unknown/expired/already-consumed state is indistinguishable from
/// any other invalid state in the response (mirrors the OTC enumeration-oracle guard,
/// WR-03) and returns 401, never leaking which case it was. This route still carries
/// no owner-token auth gate of its own — like `/auth/exchange`, it IS an auth entry
/// point (Composio calling back, not an already-authenticated owner client) — so
/// production deployments should still front it with the same network-boundary
/// discipline as other webhook endpoints; the state-nonce binding is the primary
/// mitigation, network placement is defense in depth on top of it.
///
/// Raw `Bytes` (not the `Json<...>` extractor) so a malformed body always resolves
/// to a deliberate 400 here rather than axum's default 422 rejection response —
/// mirrors `ingest_handler`'s same raw-body-then-manual-parse idiom.
async fn composio_callback_handler(
    State(state): State<AppState>,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    let payload: ComposioCallbackBody = match serde_json::from_slice(&body) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(event = "composio_callback_bad_body", error = %e);
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "invalid callback body" })),
            )
                .into_response();
        }
    };

    let oauth = match &state.composio_oauth {
        Some(o) => o.clone(),
        None => {
            tracing::warn!(event = "composio_callback_not_configured");
            return (
                StatusCode::NOT_IMPLEMENTED,
                Json(serde_json::json!({ "error": "composio oauth not configured" })),
            )
                .into_response();
        }
    };

    // T-11-06-01: owner/toolkit come ONLY from the server-side state record — the
    // request body has no owner/toolkit fields to forge in the first place.
    let (owner, toolkit) = match oauth.consume_state(&payload.state).await {
        Ok(Some(pair)) => pair,
        Ok(None) => {
            tracing::warn!(event = "composio_callback_invalid_state");
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({ "error": "invalid or expired state" })),
            )
                .into_response();
        }
        Err(e) => {
            tracing::warn!(event = "composio_callback_state_lookup_failed", error = %e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "failed to validate callback" })),
            )
                .into_response();
        }
    };

    match oauth
        .store_connection(&owner, &toolkit, &payload.connected_account_id)
        .await
    {
        Ok(()) => {
            tracing::info!(
                event = "composio_connection_stored",
                owner = %owner,
                toolkit = %toolkit,
            );
            (
                StatusCode::OK,
                Json(serde_json::json!({ "status": "connected" })),
            )
                .into_response()
        }
        Err(e) => {
            tracing::warn!(event = "composio_callback_store_failed", error = %e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "failed to persist connection" })),
            )
                .into_response()
        }
    }
}

/// SEC-02: returns true for IP addresses that must not be reachable via mesh peer_url.
/// Blocks loopback (127.x, ::1), unspecified (0.0.0.0), RFC1918 private ranges,
/// and IPv6 ULA (fc00::/7).
fn is_private_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            v4.is_loopback()    // 127.0.0.0/8
            || v4.is_private()  // 10.x, 172.16-31.x, 192.168.x
            || v4.is_link_local() // 169.254.x
            || v4.is_unspecified() // 0.0.0.0
            || v4.is_broadcast() // 255.255.255.255
        }
        std::net::IpAddr::V6(v6) => {
            // IPv4-mapped (::ffff:a.b.c.d) and IPv4-compatible addresses must be
            // re-checked as V4 or an attacker bypasses the allowlist with e.g.
            // [::ffff:127.0.0.1] / [::ffff:169.254.169.254].
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_private_ip(std::net::IpAddr::V4(v4));
            }
            v6.is_loopback()   // ::1
            || v6.is_unspecified() // ::
            || (v6.segments()[0] & 0xfe00) == 0xfc00 // fc00::/7 ULA
            || (v6.segments()[0] & 0xffc0) == 0xfe80 // link-local fe80::/10
            || (v6.segments()[0] & 0xffc0) == 0xfec0 // deprecated site-local fec0::/10
        }
    }
}

/// POST /mesh/pair body.
#[derive(Deserialize)]
struct MeshPairBody {
    token: String,
    peer_url: String,
    age_pubkey: String,
}

/// POST /mesh/pair { token: "BAST-PEER-XXXX", peer_url: "http://...", age_pubkey: "age1..." }
///
/// Validate pairing OTC TTL, register peer in MeshPeerMap, persist to bastion.toml.
/// CR-03: requires x-bastion-token (the pairing initiator must be authenticated).
async fn mesh_pair_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<MeshPairBody>,
) -> impl IntoResponse {
    let _owner = match resolve_owner_or_401(
        &headers,
        &state.owner_map,
        &state.jwt_secret,
        "mesh_pair_unauthorized",
    ) {
        Ok(o) => o,
        Err(resp) => return *resp,
    };

    // Validate pairing token (same OTC store used by /connect-app pairing flow)
    let result = {
        let store = state.otc_store.read().await;
        store
            .get(&body.token)
            .map(|grant| (grant.device_name.clone(), grant.issued_at.elapsed()))
    };

    match result {
        Some((peer_owner_id, elapsed)) if elapsed.as_secs() < 300 => {
            // Token valid — consume it
            state.otc_store.write().await.remove(&body.token);

            // SEC-01: validate age_pubkey format before registering or persisting
            {
                let re = regex::Regex::new(r"^age1[0-9a-z]+$").expect("static regex");
                if !re.is_match(&body.age_pubkey) {
                    tracing::warn!(event = "mesh_pair_invalid_age_pubkey", age_pubkey = %body.age_pubkey);
                    return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "invalid age_pubkey format — must match ^age1[0-9a-z]+$" }))).into_response();
                }
            }

            // SEC-02: validate peer_url before registering — prevent SSRF to loopback/RFC1918/link-local.
            // The validated address is captured (`pinned_addr`) and reused below to build the
            // WR-01 fetch client — resolving DNS a second time at request time would reopen
            // exactly the SSRF window this block closes (DNS rebinding: the attacker's
            // nameserver answers a public IP here, then a private one on the real connect).
            let pinned_addr: std::net::SocketAddr;
            let host: String;
            {
                use url::Url;
                let parsed = Url::parse(&body.peer_url)
                    .ok()
                    .filter(|u| u.scheme() == "https");
                let parsed = match parsed {
                    Some(u) => u,
                    None => {
                        tracing::warn!(event = "mesh_pair_invalid_url", url = %body.peer_url);
                        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "peer_url must be a valid https:// URL" }))).into_response();
                    }
                };
                // DNS-resolve and reject private/loopback/link-local addresses
                host = parsed.host_str().unwrap_or("").to_string();
                match tokio::net::lookup_host(format!("{}:443", host)).await {
                    Ok(addrs) => {
                        let mut chosen: Option<std::net::SocketAddr> = None;
                        for addr in addrs {
                            let ip = addr.ip();
                            if is_private_ip(ip) {
                                tracing::warn!(
                                    event = "mesh_pair_ssrf_blocked",
                                    url = %body.peer_url,
                                    ip = %ip,
                                );
                                return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "peer_url resolves to a private/loopback address" }))).into_response();
                            }
                            if chosen.is_none() {
                                chosen = Some(addr);
                            }
                        }
                        pinned_addr = match chosen {
                            Some(a) => a,
                            None => {
                                tracing::warn!(event = "mesh_pair_dns_empty", url = %body.peer_url);
                                return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "peer_url DNS resolution returned no addresses" }))).into_response();
                            }
                        };
                    }
                    Err(e) => {
                        // DNS failure — reject (fail-closed; attacker might be testing internal names)
                        tracing::warn!(event = "mesh_pair_dns_failed", url = %body.peer_url, error = %e);
                        return (
                            StatusCode::BAD_REQUEST,
                            Json(serde_json::json!({ "error": "peer_url DNS resolution failed" })),
                        )
                            .into_response();
                    }
                }
            }

            // WR-01: verify the peer actually controls the age/Ed25519 keypair it
            // claims before trusting it — a valid OTC only proves the caller is the
            // intended pairing target, not that `body.age_pubkey` is genuine. Fetch
            // the peer's own signed Agent Card and check the signature + pubkey match
            // BEFORE registering. Without this, signing an Agent Card is pure theater:
            // nothing in the mesh trust flow ever verifies one.
            {
                // Pin the hostname to the exact address SEC-02 just validated — prevents
                // a DNS-rebinding SSRF where a second, independent resolution at connect
                // time returns a private/internal address instead.
                let client = reqwest::Client::builder()
                    .redirect(reqwest::redirect::Policy::none())
                    .resolve(&host, pinned_addr)
                    .build()
                    .expect("failed to build reqwest client");
                let card_url = format!("{}/agent-card", body.peer_url.trim_end_matches('/'));

                let card: bastion_mesh::identity::AgentCard = match client
                    .get(&card_url)
                    .send()
                    .await
                {
                    Ok(resp) => match resp.json().await {
                        Ok(c) => c,
                        Err(e) => {
                            tracing::warn!(event = "mesh_pair_agent_card_parse_failed", url = %card_url, error = %e);
                            return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "peer /agent-card returned an invalid Agent Card" }))).into_response();
                        }
                    },
                    Err(e) => {
                        tracing::warn!(event = "mesh_pair_agent_card_fetch_failed", url = %card_url, error = %e);
                        return (
                            StatusCode::BAD_REQUEST,
                            Json(
                                serde_json::json!({ "error": "could not fetch peer /agent-card" }),
                            ),
                        )
                            .into_response();
                    }
                };

                if card.pubkey_age != body.age_pubkey {
                    tracing::warn!(event = "mesh_pair_agent_card_pubkey_mismatch", claimed = %body.age_pubkey, card = %card.pubkey_age);
                    return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "peer /agent-card pubkey_age does not match the pairing request" }))).into_response();
                }

                let sig_b64 = match card.signature.as_deref() {
                    Some(s) => s,
                    None => {
                        tracing::warn!(event = "mesh_pair_agent_card_unsigned", url = %card_url);
                        return (
                            StatusCode::BAD_REQUEST,
                            Json(serde_json::json!({ "error": "peer /agent-card is unsigned" })),
                        )
                            .into_response();
                    }
                };
                let sig_bytes = match base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .decode(sig_b64)
                {
                    Ok(b) => b,
                    Err(_) => {
                        tracing::warn!(event = "mesh_pair_agent_card_bad_signature_encoding", url = %card_url);
                        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "peer /agent-card signature is not valid base64url" }))).into_response();
                    }
                };

                match bastion_mesh::identity::age_identity::AgeIdentity::verify_agent_card(
                    &card, &sig_bytes,
                ) {
                    Ok(true) => {}
                    Ok(false) => {
                        tracing::warn!(event = "mesh_pair_agent_card_signature_invalid", url = %card_url);
                        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "peer /agent-card signature is invalid" }))).into_response();
                    }
                    Err(e) => {
                        tracing::warn!(event = "mesh_pair_agent_card_verify_error", url = %card_url, error = %e);
                        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "peer /agent-card could not be verified" }))).into_response();
                    }
                }

                tracing::info!(event = "mesh_pair_agent_card_verified", url = %card_url);
            }

            // Register peer in MeshPeerMap
            let peer = MeshPeer {
                peer_url: body.peer_url.clone(),
                age_pubkey: body.age_pubkey.clone(),
                allowed_tags: vec![], // set after pairing via config update
            };
            state
                .mesh_peer_map
                .write()
                .await
                .register(peer_owner_id.clone(), peer);

            // Persist to bastion.toml [[mesh.peer]] (best-effort; full persistence in config.rs)
            // allowed_tags starts empty — set post-pairing via config update
            if let Err(e) = crate::config::append_mesh_peer(
                &peer_owner_id,
                &body.peer_url,
                &body.age_pubkey,
                &[],
            )
            .await
            {
                tracing::warn!(event = "mesh_pair_persist_failed", error = %e, "peer registered in memory but toml persist failed");
            }

            tracing::info!(event = "mesh_pair_ok", peer_owner = %peer_owner_id, peer_url = %body.peer_url);
            (
                StatusCode::OK,
                Json(serde_json::json!({ "status": "paired", "peer_owner": peer_owner_id })),
            )
                .into_response()
        }
        Some(_) => {
            // WR-03: return same body as unknown-token to prevent enumeration oracle.
            state.otc_store.write().await.remove(&body.token);
            tracing::warn!(event = "mesh_pair_expired_token");
            (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({ "error": "invalid pairing token" })),
            )
                .into_response()
        }
        None => (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": "invalid pairing token" })),
        )
            .into_response(),
    }
}

/// GET /agent-card — return signed Agent Card JSON (SEC-06).
///
/// Returns 404 if agent identity is not configured (MESH_IDENTITY_KEY not set).
/// Returns a signed `AgentCard` document with age pubkey, Ed25519 pubkey,
/// capabilities, and mesh/MCP URLs.
async fn agent_card_handler(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let identity = state.agent_identity.as_ref().ok_or(StatusCode::NOT_FOUND)?;

    // WR-07: BASTION_WEBHOOK_ADDR is a bind address (e.g. "0.0.0.0:8080") — not a
    // scheme-qualified, externally-routable URL. Publishing it verbatim on a SIGNED
    // Agent Card means any peer that trusted it would dial an address nobody can
    // reach. BASTION_PUBLIC_URL is the operator-declared externally-reachable base
    // (e.g. "https://bastion.example.com"), independent of the socket bind address.
    let public_url = std::env::var("BASTION_PUBLIC_URL").unwrap_or_default();
    let mesh_url = if public_url.is_empty() {
        None
    } else {
        Some(public_url.clone())
    };
    let mcp_url = if public_url.is_empty() {
        None
    } else {
        Some(format!("{}/mcp", public_url))
    };

    let mut card = bastion_mesh::identity::AgentCard {
        version: bastion_mesh::identity::AGENT_CARD_VERSION,
        name: state.agent_name.clone(),
        pubkey_age: identity.pubkey_age(),
        pubkey_ed25519: identity.pubkey_ed25519(),
        capabilities: vec![
            "memory_retrieve".to_string(),
            "memory_search".to_string(),
            "personas_list".to_string(),
            "goals_list".to_string(),
            "tools_invoke".to_string(),
        ],
        allowed_tags: vec![],
        mesh_url,
        mcp_url,
        signature: None,
    };

    let signature = identity
        .sign_agent_card(&card)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    card.signature = Some(engine.encode(&signature));

    Ok(Json(
        serde_json::to_value(&card).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    ))
}

/// GET /whatsapp/webhook — Meta's one-time verification handshake (CHAN-01, D-04
/// onboarding). Echoes `hub.challenge` verbatim only when `hub.mode == "subscribe"`
/// AND `hub.verify_token` matches the configured token; 403 otherwise (including
/// when WhatsApp isn't configured at all — T-10-04-03).
async fn whatsapp_verify_handler(
    State(state): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let expected_token = state
        .whatsapp
        .as_ref()
        .map(|w| w.sender.verify_token.clone());
    match (
        q.get("hub.mode"),
        q.get("hub.verify_token"),
        q.get("hub.challenge"),
        expected_token,
    ) {
        (Some(mode), Some(token), Some(challenge), Some(expected))
            if mode == "subscribe" && *token == expected =>
        {
            (StatusCode::OK, challenge.clone()).into_response()
        }
        _ => StatusCode::FORBIDDEN.into_response(),
    }
}

/// POST /whatsapp/webhook — inbound WhatsApp message delivery (CHAN-01).
///
/// Follows the EXACT ordering of `ingest_handler` (Pitfall 1 / `#mesh-ingest-401`):
/// raw `axum::body::Bytes` (never a `Json<T>` extractor) so the HMAC signature check
/// runs BEFORE any JSON parsing — a forged payload never reaches `serde_json::from_slice`,
/// `handle_whatsapp_message`, or the AgentLoop (T-10-04-01).
///
/// Always returns 200 to Meta once past signature verification (delivery-receipt
/// `statuses` webhooks, non-text messages, unmapped senders, and turn errors are all
/// swallowed here — Meta only cares that the webhook was received, not that a reply
/// was sent, and a non-200 here would trigger Meta's retry-storm behavior).
async fn whatsapp_receive_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    let whatsapp = match state.whatsapp.as_ref() {
        Some(w) => w,
        None => return StatusCode::NOT_FOUND.into_response(),
    };

    let signature = headers
        .get("X-Hub-Signature-256")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if !whatsapp.sender.verify_signature(&body, signature) {
        tracing::warn!(event = "whatsapp_receive_bad_signature");
        return StatusCode::FORBIDDEN.into_response();
    }

    let payload: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(event = "whatsapp_receive_bad_body", error = %e);
            // Signature was valid but the body wasn't the JSON shape we expect —
            // still 200 (Meta expects a fast ack for every webhook delivery).
            return StatusCode::OK.into_response();
        }
    };

    let message = payload
        .get("entry")
        .and_then(|e| e.get(0))
        .and_then(|e| e.get("changes"))
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("value"))
        .and_then(|v| v.get("messages"))
        .and_then(|m| m.get(0));

    let Some(message) = message else {
        // e.g. a `statuses` delivery-receipt webhook — no `messages` key present.
        // Meta expects a fast 200 for every webhook type, not just message events.
        return StatusCode::OK.into_response();
    };

    let (Some(from), Some(text)) = (
        message.get("from").and_then(|v| v.as_str()),
        message
            .get("text")
            .and_then(|t| t.get("body"))
            .and_then(|v| v.as_str()),
    ) else {
        // Non-text message type (image/audio/etc) — skip reply attempt, still 200.
        return StatusCode::OK.into_response();
    };

    match crate::channel::whatsapp::handle_whatsapp_message(
        from.to_string(),
        text.to_string(),
        &state.agent,
        &whatsapp.owner_map,
    )
    .await
    {
        Ok(reply) => {
            if let Err(e) = whatsapp.sender.send_text(from, &reply).await {
                tracing::warn!(event = "whatsapp_send_failed", error = %e, "reply send failed");
            }
        }
        Err(e) => {
            if e.to_string().contains("not in owner map") {
                // CR-03: unknown sender — warn and skip silently (no reply attempt).
                tracing::warn!(event = "whatsapp_handle_message_error", from = %from, error = %e);
            } else {
                // M3: log turn_error WITHOUT conversation content.
                tracing::error!(event = "whatsapp_turn_error", from = %from);
            }
        }
    }

    StatusCode::OK.into_response()
}

pub async fn serve(
    agent: AgentHandle,
    addr: &str,
    owner_map: OwnerMap,
    events_tx: broadcast::Sender<String>,
    mesh_peer_map: Arc<RwLock<MeshPeerMap>>,
    jwt_secret: String,
) -> anyhow::Result<()> {
    // Self-contained entry point (no daemon_loop boot sequence around it) —
    // reports itself ready immediately, and lifecycle stays disabled
    // (fail-closed, no token configured) unless a caller uses
    // `serve_with_mesh` directly with its own `LifecycleControl`.
    let readiness = crate::channel::operational::ReadinessState::new();
    readiness.mark_session_ready();
    readiness.mark_memory_ready();
    readiness.mark_provider_ready();
    readiness.mark_channels_ready();
    let lifecycle = crate::channel::operational::LifecycleControl::new(
        crate::channel::operational::DaemonAccessAuth::new(None),
    );
    // Self-contained entry point has no daemon_loop composition root to build
    // these from — an empty registry + default (no `[auth.*]`) config mean
    // `/status` reports zero runtimes rather than lying about what's wired.
    let runtime_registry = bastion_runtime::agent::backend::RuntimeRegistry::new();
    let auth = crate::config::AuthConfig::default();
    let updates = Arc::new(tokio::sync::RwLock::new(
        crate::update::UpdateSnapshot::current(),
    ));
    serve_with_mesh(
        agent,
        addr,
        owner_map,
        events_tx,
        mesh_peer_map,
        jwt_secret,
        None,
        None,
        new_otc_store(),
        None,
        "bastion".to_string(),
        None,
        None,
        None,
        readiness,
        lifecycle,
        runtime_registry,
        auth,
        updates,
    )
    .await
}

/// Extended serve function that accepts optional mesh transport and slice store.
/// Called by daemon startup when MESH_IDENTITY_KEY is configured.
///
/// `otc_store`: shared OTC store — pass a handle to skill commands so they can insert
/// BAST-XXXX codes for /auth/exchange and /mesh/pair. Use `new_otc_store()` to create one.
// Wires 9 independent server dependencies from daemon startup; a params struct would be
// a single-call-site bag (only main.rs constructs this) with no reusable shape.
#[allow(clippy::too_many_arguments)]
pub async fn serve_with_mesh(
    agent: AgentHandle,
    addr: &str,
    owner_map: OwnerMap,
    events_tx: broadcast::Sender<String>,
    mesh_peer_map: Arc<RwLock<MeshPeerMap>>,
    jwt_secret: String,
    mesh_transport: Option<bastion_mesh::mesh::SharedMeshTransport>,
    mesh_slice_store: Option<bastion_mesh::mesh::context_provider::MeshSliceStore>,
    otc_store: OtcStore,
    agent_identity: Option<std::sync::Arc<bastion_mesh::identity::age_identity::AgeIdentity>>,
    agent_name: String,
    // Optional pre-built axum Router to mount alongside the webhook routes.
    // Used by the MCP server to expose its Streamable HTTP service at the
    // configured mount path (e.g. `/mcp`).
    mcp_routes: Option<axum::Router>,
    // WhatsApp Cloud API config (CHAN-01). None = WhatsApp routes are mounted but
    // reject with 404/403 rather than panicking (daemon startup wiring lands in
    // Plan 10-09).
    whatsapp: Option<crate::channel::whatsapp::WhatsAppConfig>,
    // Composio OAuth client (SEC-03). None = /auth/composio/callback rejects with
    // 501 rather than panicking — opt-in, requires COMPOSIO_API_KEY.
    composio_oauth: Option<std::sync::Arc<bastion_mcp::oauth::ComposioOAuth>>,
    // Loop 3-D (`docs/revamp/C3-cloud-ready-design.md`): boot-sequence
    // readiness gate backing `/readyz` — built and threaded by the caller
    // (`daemon_loop`/`serve`) so ITS OWN startup sequence decides when each
    // component is actually ready, never this function.
    readiness: std::sync::Arc<crate::channel::operational::ReadinessState>,
    // Daemon-access-gated stop/reload control backing `/lifecycle/*`.
    lifecycle: crate::channel::operational::LifecycleControl,
    // Fase 2.9: backs `GET /status` — see `AppState.runtime_registry` doc.
    runtime_registry: bastion_runtime::agent::backend::RuntimeRegistry,
    auth: crate::config::AuthConfig,
    updates: crate::update::SharedUpdateState,
) -> anyhow::Result<()> {
    let state = AppState {
        agent,
        owner_map: Arc::new(owner_map),
        events_tx,
        mesh_peer_map,
        otc_store,
        jwt_secret,
        mesh_transport,
        mesh_slice_store,
        agent_identity,
        agent_name,
        whatsapp,
        composio_oauth,
        readiness,
        lifecycle,
        runtime_registry,
        auth,
        updates,
    };
    let mut app = Router::new()
        .route("/webhook", post(handle))
        .route("/events", axum::routing::get(sse_handler))
        .route("/agent-card", get(agent_card_handler))
        .route("/mesh/ingest", post(ingest_handler))
        .route("/auth/exchange", post(auth_exchange_handler))
        .route("/mesh/pair", post(mesh_pair_handler))
        .route(
            "/whatsapp/webhook",
            get(whatsapp_verify_handler).post(whatsapp_receive_handler),
        )
        .route("/auth/composio/callback", post(composio_callback_handler))
        // Loop 3-D operational contract — liveness/readiness/lifecycle. Same
        // axum server, no new port; `FromRef<AppState>` above lets these
        // handlers (defined against their own narrower state types in
        // `operational.rs`) mount directly onto this `Router<AppState>`.
        .route(
            "/healthz",
            axum::routing::get(crate::channel::operational::liveness_handler),
        )
        .route(
            "/readyz",
            axum::routing::get(crate::channel::operational::readiness_handler),
        )
        // Fase 2.9: booleans-only runtime/login status — see module doc.
        .route(
            "/status",
            axum::routing::get(crate::channel::operational::status_handler),
        )
        .route(
            "/lifecycle/stop",
            post(crate::channel::operational::lifecycle_stop_handler),
        )
        .route(
            "/lifecycle/reload",
            post(crate::channel::operational::lifecycle_reload_handler),
        )
        .with_state(state);
    if let Some(mcp) = mcp_routes {
        app = app.merge(mcp);
    }
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

// ─── tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::OwnerMap;
    use axum::body::Body;
    use bastion_runtime::agent::handle;
    use http::{Request, StatusCode};
    use tokio::sync::mpsc;
    use tower::ServiceExt;

    fn stub_consumer(mut rx: mpsc::Receiver<bastion_runtime::agent::handle::AgentRequest>) {
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                let _ = req.reply.send(Ok(format!("echo:{}", req.text)));
            }
        });
    }

    /// Fresh, fully-ready `(readiness, lifecycle)` pair for tests unrelated
    /// to the Loop 3-D operational contract itself — `operational.rs` has
    /// its own dedicated unit tests for the not-ready/unauthorized cases.
    fn test_operational_state() -> (
        Arc<crate::channel::operational::ReadinessState>,
        crate::channel::operational::LifecycleControl,
    ) {
        let readiness = crate::channel::operational::ReadinessState::new();
        readiness.mark_session_ready();
        readiness.mark_memory_ready();
        readiness.mark_provider_ready();
        readiness.mark_channels_ready();
        let lifecycle = crate::channel::operational::LifecycleControl::new(
            crate::channel::operational::DaemonAccessAuth::new(None),
        );
        (readiness, lifecycle)
    }

    /// Builds a full test Router + an atomic counter incremented once per turn the
    /// stub agent consumer actually processes — used by the WhatsApp bad-signature
    /// test to assert `handle_whatsapp_message`/`AgentHandle::ask` was never reached.
    fn build_router_with_map_and_whatsapp(
        map: OwnerMap,
        whatsapp: Option<crate::channel::whatsapp::WhatsAppConfig>,
    ) -> (Router, Arc<std::sync::atomic::AtomicUsize>) {
        let (h, mut rx) = handle::channel();
        let turn_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let turn_count_clone = turn_count.clone();
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                turn_count_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let _ = req.reply.send(Ok(format!("echo:{}", req.text)));
            }
        });
        let (events_tx, _) = broadcast::channel::<String>(128);
        let mesh_peer_map = Arc::new(RwLock::new(MeshPeerMap::new()));
        let otc_store = Arc::new(RwLock::new(std::collections::HashMap::new()));
        let (readiness, lifecycle) = test_operational_state();
        let state = AppState {
            agent: h,
            owner_map: Arc::new(map),
            events_tx,
            mesh_peer_map,
            otc_store,
            jwt_secret: "test-secret".to_string(),
            mesh_transport: None,
            mesh_slice_store: None,
            agent_identity: None,
            agent_name: "test".to_string(),
            whatsapp,
            composio_oauth: None,
            readiness,
            lifecycle,
            runtime_registry: bastion_runtime::agent::backend::RuntimeRegistry::new(),
            auth: crate::config::AuthConfig::default(),
            updates: Arc::new(tokio::sync::RwLock::new(
                crate::update::UpdateSnapshot::current(),
            )),
        };
        let router = Router::new()
            .route("/webhook", post(handle))
            .route("/events", axum::routing::get(sse_handler))
            .route("/agent-card", get(agent_card_handler))
            .route("/mesh/ingest", post(ingest_handler))
            .route("/auth/exchange", post(auth_exchange_handler))
            .route("/mesh/pair", post(mesh_pair_handler))
            .route(
                "/whatsapp/webhook",
                get(whatsapp_verify_handler).post(whatsapp_receive_handler),
            )
            .route("/auth/composio/callback", post(composio_callback_handler))
            .with_state(state);
        (router, turn_count)
    }

    fn build_router_with_map(map: OwnerMap) -> Router {
        build_router_with_map_and_whatsapp(map, None).0
    }

    fn build_router() -> Router {
        build_router_with_map(OwnerMap::from_pairs(&[("token-mario", "mario")]))
    }

    /// Loop 3-D: builds a router with the SAME operational routes
    /// `serve_with_mesh` mounts in production (`/healthz`, `/readyz`,
    /// `/lifecycle/stop`, `/lifecycle/reload`), over caller-supplied
    /// readiness/lifecycle state — proves the `FromRef<AppState>` wiring
    /// actually works end to end, not just the handlers in isolation
    /// (`operational.rs`'s own unit tests already cover those).
    fn build_operational_router(
        readiness: Arc<crate::channel::operational::ReadinessState>,
        lifecycle: crate::channel::operational::LifecycleControl,
    ) -> Router {
        let (h, rx) = handle::channel();
        stub_consumer(rx);
        let (events_tx, _) = broadcast::channel::<String>(128);
        let mesh_peer_map = Arc::new(RwLock::new(MeshPeerMap::new()));
        let state = AppState {
            agent: h,
            owner_map: Arc::new(OwnerMap::default()),
            events_tx,
            mesh_peer_map,
            otc_store: new_otc_store(),
            jwt_secret: "test-secret".to_string(),
            mesh_transport: None,
            mesh_slice_store: None,
            agent_identity: None,
            agent_name: "test".to_string(),
            whatsapp: None,
            composio_oauth: None,
            readiness,
            lifecycle,
            runtime_registry: bastion_runtime::agent::backend::RuntimeRegistry::new(),
            auth: crate::config::AuthConfig::default(),
            updates: Arc::new(tokio::sync::RwLock::new(
                crate::update::UpdateSnapshot::current(),
            )),
        };
        Router::new()
            .route(
                "/healthz",
                axum::routing::get(crate::channel::operational::liveness_handler),
            )
            .route(
                "/readyz",
                axum::routing::get(crate::channel::operational::readiness_handler),
            )
            .route(
                "/lifecycle/stop",
                post(crate::channel::operational::lifecycle_stop_handler),
            )
            .route(
                "/lifecycle/reload",
                post(crate::channel::operational::lifecycle_reload_handler),
            )
            .with_state(state)
    }

    #[tokio::test]
    async fn mounted_healthz_always_200() {
        let (readiness, lifecycle) = test_operational_state();
        let app = build_operational_router(readiness, lifecycle);
        let req = Request::builder()
            .uri("/healthz")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn mounted_readyz_503_before_ready_200_after() {
        let readiness = crate::channel::operational::ReadinessState::new();
        let lifecycle = crate::channel::operational::LifecycleControl::new(
            crate::channel::operational::DaemonAccessAuth::new(None),
        );
        let app = build_operational_router(readiness.clone(), lifecycle);

        let req = Request::builder()
            .uri("/readyz")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);

        readiness.mark_session_ready();
        readiness.mark_memory_ready();
        readiness.mark_provider_ready();
        readiness.mark_channels_ready();

        let req = Request::builder()
            .uri("/readyz")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn mounted_lifecycle_stop_requires_daemon_access_token() {
        let readiness = crate::channel::operational::ReadinessState::new();
        let lifecycle = crate::channel::operational::LifecycleControl::new(
            crate::channel::operational::DaemonAccessAuth::new(Some("op-token".to_string())),
        );
        let shutdown = lifecycle.shutdown.clone();
        let app = build_operational_router(readiness, lifecycle);

        // No token at all — refused.
        let req = Request::builder()
            .method("POST")
            .uri("/lifecycle/stop")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        // Correct token — accepted, and the SAME Notify daemon_loop's
        // select! arm awaits actually fires.
        let req = Request::builder()
            .method("POST")
            .uri("/lifecycle/stop")
            .header("authorization", "Bearer op-token")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        tokio::time::timeout(std::time::Duration::from_secs(1), shutdown.notified())
            .await
            .expect("shutdown notify must fire through the mounted route");
    }

    /// Test helper: a WhatsAppConfig with a known test app_secret/verify_token so
    /// tests can compute matching signatures / verify tokens.
    fn test_whatsapp_config(
        map: OwnerMap,
        app_secret: &str,
        verify_token: &str,
    ) -> crate::channel::whatsapp::WhatsAppConfig {
        crate::channel::whatsapp::WhatsAppConfig {
            owner_map: map,
            sender: Arc::new(crate::channel::whatsapp::WhatsAppSender::new(
                "test-phone-id",
                "test-access-token",
                app_secret,
                verify_token,
            )),
        }
    }

    /// Test helper: compute a valid `sha256=<hex>` signature the same way Meta does.
    fn sign_whatsapp(secret: &str, body: &[u8]) -> String {
        use hmac::{Hmac, Mac};
        use sha2::Sha256;
        type HmacSha256 = Hmac<Sha256>;
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("hmac key");
        mac.update(body);
        let digest = mac.finalize().into_bytes();
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        format!("sha256={hex}")
    }

    #[tokio::test]
    async fn post_webhook_valid_token_returns_json_reply() {
        let app = build_router();

        let body = serde_json::json!({ "text": "hello" }).to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/webhook")
            .header("content-type", "application/json")
            .header("x-bastion-token", "token-mario")
            .body(Body::from(body))
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let out: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(out["reply"], "echo:hello");
    }

    #[tokio::test]
    async fn post_webhook_unknown_token_returns_401() {
        let app = build_router();

        let body = serde_json::json!({ "text": "ping" }).to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/webhook")
            .header("content-type", "application/json")
            .header("x-bastion-token", "unknown-token")
            .body(Body::from(body))
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn post_webhook_missing_token_returns_401() {
        let app = build_router();

        let body = serde_json::json!({ "text": "ping" }).to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/webhook")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    /// Verify that error replies have no content leak — body must not contain internal detail.
    #[tokio::test]
    async fn error_response_has_no_content_leak() {
        // Use an empty OwnerMap so ALL requests get 401 — no stub consumer needed.
        let app = build_router_with_map(OwnerMap::default());

        let body = serde_json::json!({ "text": "ping" }).to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/webhook")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_ne!(
            response.status(),
            StatusCode::OK,
            "error must not return 200"
        );

        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&bytes);
        // Body must not contain any stack trace, error message, or internal token detail
        assert!(!text.contains("thread"), "stack trace in response: {text}");
        assert!(!text.contains("panicked"), "panic in response: {text}");
    }

    /// Verify error_status maps BastionError variants correctly (WR-09: typed, no string prefix).
    #[test]
    fn error_status_maps_variants() {
        let egress_err = anyhow::anyhow!(BastionError::PrivacyEgressBlocked);
        assert_eq!(error_status(&egress_err), StatusCode::FORBIDDEN);

        let budget_err = anyhow::anyhow!(BastionError::BudgetExceeded);
        assert_eq!(error_status(&budget_err), StatusCode::TOO_MANY_REQUESTS);

        // Guardrail errors are now typed BastionError::InputGuardrailRejected (WR-09)
        let guard_err = anyhow::anyhow!(BastionError::InputGuardrailRejected(
            "input is empty".to_owned()
        ));
        assert_eq!(error_status(&guard_err), StatusCode::BAD_REQUEST);

        // Unknown errors → 500
        let other = anyhow::anyhow!("something exploded");
        assert_eq!(error_status(&other), StatusCode::INTERNAL_SERVER_ERROR);

        // Fase 2.8: BackendUnavailable/ApprovalDenied also get specific statuses now.
        let backend_err = anyhow::anyhow!(BastionError::BackendUnavailable(
            "codex_app_server: not logged in".to_string()
        ));
        assert_eq!(error_status(&backend_err), StatusCode::SERVICE_UNAVAILABLE);

        let denied_err = anyhow::anyhow!(BastionError::ApprovalDenied {
            capability: "shell_exec".to_string(),
            scope: bastion_types::DenyScope::Turn,
        });
        assert_eq!(error_status(&denied_err), StatusCode::FORBIDDEN);
    }

    /// Fase 2.8: `error_body` whitelist — the four safe-to-echo variants
    /// return their own `Display`, everything else (INCLUDING
    /// `InputGuardrailRejected`, whose detail is explicitly documented as
    /// never-echo) collapses to a generic "internal error".
    #[test]
    fn error_body_whitelists_typed_variants_only() {
        let backend_err = anyhow::anyhow!(BastionError::BackendUnavailable(
            "codex_app_server: not logged in".to_string()
        ));
        assert!(error_body(&backend_err).contains("codex_app_server"));

        let budget_err = anyhow::anyhow!(BastionError::BudgetExceeded);
        assert_eq!(
            error_body(&budget_err),
            BastionError::BudgetExceeded.to_string()
        );

        let egress_err = anyhow::anyhow!(BastionError::PrivacyEgressBlocked);
        assert_eq!(
            error_body(&egress_err),
            BastionError::PrivacyEgressBlocked.to_string()
        );

        let denied_err = anyhow::anyhow!(BastionError::ApprovalDenied {
            capability: "shell_exec".to_string(),
            scope: bastion_types::DenyScope::Turn,
        });
        assert!(error_body(&denied_err).contains("shell_exec"));

        // MUST NOT echo the guardrail detail, even though it's a typed variant.
        let guard_err = anyhow::anyhow!(BastionError::InputGuardrailRejected(
            "sensitive detail that must never reach the client".to_string()
        ));
        assert_eq!(error_body(&guard_err), "internal error");

        let other = anyhow::anyhow!("something exploded with a stack trace maybe");
        assert_eq!(error_body(&other), "internal error");
    }

    /// GET /events without token returns 401.
    #[tokio::test]
    async fn get_events_no_token_returns_401() {
        let app = build_router();
        let req = Request::builder()
            .method("GET")
            .uri("/events")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    /// POST /mesh/ingest with valid token and valid envelope returns 501 (no transport configured).
    #[tokio::test]
    async fn post_mesh_ingest_returns_501_when_no_transport() {
        let app = build_router();
        // Send a valid MeshEnvelope body — transport check happens after JSON parse
        let envelope = serde_json::json!({
            "from_owner": "peer-owner",
            "to_owner": "mario",
            "ciphertext": [],
            "recipient_hint": "age1test"
        });
        let req = Request::builder()
            .method("POST")
            .uri("/mesh/ingest")
            .header("content-type", "application/json")
            .header("x-bastion-token", "token-mario")
            .body(Body::from(envelope.to_string()))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    }

    /// POST /mesh/ingest without token returns 401 (not 501).
    #[tokio::test]
    async fn post_mesh_ingest_no_token_returns_401() {
        let app = build_router();
        let req = Request::builder()
            .method("POST")
            .uri("/mesh/ingest")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    /// POST /auth/exchange with missing otc returns 400.
    #[tokio::test]
    async fn post_auth_exchange_missing_otc_returns_400() {
        let app = build_router();
        let req = Request::builder()
            .method("POST")
            .uri("/auth/exchange")
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    /// POST /auth/exchange with invalid otc returns 401.
    #[tokio::test]
    async fn post_auth_exchange_invalid_otc_returns_401() {
        let app = build_router();
        let body = serde_json::json!({ "otc": "BAST-INVALID-OTC" }).to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/auth/exchange")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    /// POST /mesh/pair without token returns 401.
    #[tokio::test]
    async fn post_mesh_pair_no_token_returns_401() {
        let app = build_router();
        let body = serde_json::json!({
            "token": "BAST-PEER-INVALID",
            "peer_url": "http://peer:8080",
            "age_pubkey": "age1test"
        })
        .to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/mesh/pair")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    // ── CR-02 OtcStore tests ─────────────────────────────────────────────────

    /// CR-02: build_router_with_otc helper — inserts a live OTC into the shared store.
    fn build_router_with_otc(otc: &str, device_name: &str) -> Router {
        let (h, rx) = handle::channel();
        stub_consumer(rx);
        let (events_tx, _) = broadcast::channel::<String>(128);
        let mesh_peer_map = Arc::new(RwLock::new(MeshPeerMap::new()));
        let store = new_otc_store();
        // Pre-insert the OTC so /auth/exchange can consume it
        store.try_write().unwrap().insert(
            otc.to_string(),
            PairingGrant {
                owner_id: "mario".to_string(),
                device_name: device_name.to_string(),
                issued_at: std::time::Instant::now(),
            },
        );
        let (readiness, lifecycle) = test_operational_state();
        let state = AppState {
            agent: h,
            owner_map: Arc::new(OwnerMap::default()),
            events_tx,
            mesh_peer_map,
            otc_store: store,
            jwt_secret: "test-secret".to_string(),
            mesh_transport: None,
            mesh_slice_store: None,
            agent_identity: None,
            agent_name: "test".to_string(),
            whatsapp: None,
            composio_oauth: None,
            readiness,
            lifecycle,
            runtime_registry: bastion_runtime::agent::backend::RuntimeRegistry::new(),
            auth: crate::config::AuthConfig::default(),
            updates: Arc::new(tokio::sync::RwLock::new(
                crate::update::UpdateSnapshot::current(),
            )),
        };
        Router::new()
            .route("/webhook", post(handle))
            .route("/events", axum::routing::get(sse_handler))
            .route("/agent-card", get(agent_card_handler))
            .route("/auth/exchange", post(auth_exchange_handler))
            .route("/mesh/pair", post(mesh_pair_handler))
            .route(
                "/whatsapp/webhook",
                get(whatsapp_verify_handler).post(whatsapp_receive_handler),
            )
            .route("/auth/composio/callback", post(composio_callback_handler))
            .with_state(state)
    }

    // ── SEC-03 Composio callback tests ──────────────────────────────────────────

    /// Builds a router with the given (optional) ComposioOAuth wired into AppState —
    /// `None` exercises the "not configured" 501 path, `Some` the real persist path.
    fn build_router_with_composio(
        composio_oauth: Option<std::sync::Arc<bastion_mcp::oauth::ComposioOAuth>>,
    ) -> Router {
        let (h, rx) = handle::channel();
        stub_consumer(rx);
        let (events_tx, _) = broadcast::channel::<String>(128);
        let mesh_peer_map = Arc::new(RwLock::new(MeshPeerMap::new()));
        let (readiness, lifecycle) = test_operational_state();
        let state = AppState {
            agent: h,
            owner_map: Arc::new(OwnerMap::default()),
            events_tx,
            mesh_peer_map,
            otc_store: new_otc_store(),
            jwt_secret: "test-secret".to_string(),
            mesh_transport: None,
            mesh_slice_store: None,
            agent_identity: None,
            agent_name: "test".to_string(),
            whatsapp: None,
            composio_oauth,
            readiness,
            lifecycle,
            runtime_registry: bastion_runtime::agent::backend::RuntimeRegistry::new(),
            auth: crate::config::AuthConfig::default(),
            updates: Arc::new(tokio::sync::RwLock::new(
                crate::update::UpdateSnapshot::current(),
            )),
        };
        Router::new()
            .route("/auth/composio/callback", post(composio_callback_handler))
            .with_state(state)
    }

    /// Valid callback body (a state minted by the server) persists the connection
    /// under the owner/toolkit BOUND TO THAT STATE (verified via `current_connection`)
    /// and returns 200.
    #[tokio::test]
    async fn post_composio_callback_persists_connection_and_returns_200() {
        let f = tempfile::NamedTempFile::new().unwrap();
        let path = f.path().to_str().unwrap().to_owned();
        let session = bastion_runtime::session::SessionManager::new(&path);
        session.init_schema().await.expect("init_schema");
        let oauth = Arc::new(bastion_mcp::oauth::ComposioOAuth::new_for_test(
            &path,
            "http://unused.invalid",
        ));
        oauth
            .insert_state_for_test("valid-state-1", "alice", "gmail", 900)
            .await
            .expect("insert_state_for_test");

        let app = build_router_with_composio(Some(oauth.clone()));
        let body = serde_json::json!({
            "state": "valid-state-1",
            "connected_account_id": "ca_123"
        })
        .to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/auth/composio/callback")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let current = oauth
            .current_connection("alice", "gmail")
            .await
            .expect("current_connection");
        assert_eq!(current, Some("ca_123".to_string()));
    }

    /// T-11-06-01 regression: the request body has no `owner`/`toolkit` fields to
    /// forge in the first place, but even extra/unknown JSON fields on the body
    /// (an attacker trying to smuggle a spoofed owner) are silently ignored by serde
    /// and never influence which owner/toolkit the connection is stored under — that
    /// comes exclusively from the server-side state record.
    #[tokio::test]
    async fn post_composio_callback_ignores_spoofed_owner_field_in_body() {
        let f = tempfile::NamedTempFile::new().unwrap();
        let path = f.path().to_str().unwrap().to_owned();
        let session = bastion_runtime::session::SessionManager::new(&path);
        session.init_schema().await.expect("init_schema");
        let oauth = Arc::new(bastion_mcp::oauth::ComposioOAuth::new_for_test(
            &path,
            "http://unused.invalid",
        ));
        oauth
            .insert_state_for_test("valid-state-2", "real-owner", "slack", 900)
            .await
            .expect("insert_state_for_test");

        let app = build_router_with_composio(Some(oauth.clone()));
        // Attacker-supplied "owner"/"toolkit" fields — must be ignored entirely.
        let body = serde_json::json!({
            "state": "valid-state-2",
            "connected_account_id": "ca_999",
            "owner": "attacker",
            "toolkit": "gmail"
        })
        .to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/auth/composio/callback")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let attacker_conn = oauth
            .current_connection("attacker", "gmail")
            .await
            .expect("current_connection");
        assert_eq!(
            attacker_conn, None,
            "spoofed owner/toolkit body fields must never create a connection"
        );
        let real_conn = oauth
            .current_connection("real-owner", "slack")
            .await
            .expect("current_connection");
        assert_eq!(
            real_conn,
            Some("ca_999".to_string()),
            "connection must be bound to the state's owner/toolkit, not the body's"
        );
    }

    /// T-11-06-01 regression: missing, unknown, expired, or already-consumed state
    /// tokens are all rejected with 401 — never a panic, never a silent bind.
    #[tokio::test]
    async fn post_composio_callback_invalid_state_returns_401() {
        let f = tempfile::NamedTempFile::new().unwrap();
        let path = f.path().to_str().unwrap().to_owned();
        let session = bastion_runtime::session::SessionManager::new(&path);
        session.init_schema().await.expect("init_schema");
        let oauth = Arc::new(bastion_mcp::oauth::ComposioOAuth::new_for_test(
            &path,
            "http://unused.invalid",
        ));
        // Already-expired state (negative TTL).
        oauth
            .insert_state_for_test("expired-state", "alice", "gmail", -60)
            .await
            .expect("insert_state_for_test");
        // A state that will be consumed once, then retried (replay).
        oauth
            .insert_state_for_test("single-use-state", "alice", "gmail", 900)
            .await
            .expect("insert_state_for_test");

        let app = build_router_with_composio(Some(oauth.clone()));

        // Case 1: unknown state — never issued.
        let req = Request::builder()
            .method("POST")
            .uri("/auth/composio/callback")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({ "state": "never-issued", "connected_account_id": "ca_1" })
                    .to_string(),
            ))
            .unwrap();
        assert_eq!(
            app.clone().oneshot(req).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );

        // Case 2: expired state.
        let req = Request::builder()
            .method("POST")
            .uri("/auth/composio/callback")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({ "state": "expired-state", "connected_account_id": "ca_2" })
                    .to_string(),
            ))
            .unwrap();
        assert_eq!(
            app.clone().oneshot(req).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );

        // Case 3: valid state, first use — succeeds.
        let req = Request::builder()
            .method("POST")
            .uri("/auth/composio/callback")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({ "state": "single-use-state", "connected_account_id": "ca_3" })
                    .to_string(),
            ))
            .unwrap();
        assert_eq!(
            app.clone().oneshot(req).await.unwrap().status(),
            StatusCode::OK
        );

        // Case 4: same state, replayed — must now be rejected (already consumed).
        let req = Request::builder()
            .method("POST")
            .uri("/auth/composio/callback")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({ "state": "single-use-state", "connected_account_id": "ca_4" })
                    .to_string(),
            ))
            .unwrap();
        assert_eq!(
            app.oneshot(req).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
    }

    /// A malformed callback body (missing a required field) returns 400, never panics.
    #[tokio::test]
    async fn post_composio_callback_malformed_body_returns_400() {
        let f = tempfile::NamedTempFile::new().unwrap();
        let path = f.path().to_str().unwrap().to_owned();
        let session = bastion_runtime::session::SessionManager::new(&path);
        session.init_schema().await.expect("init_schema");
        let oauth = Arc::new(bastion_mcp::oauth::ComposioOAuth::new_for_test(
            &path,
            "http://unused.invalid",
        ));

        let app = build_router_with_composio(Some(oauth));
        // Missing "connected_account_id".
        let body = serde_json::json!({ "state": "some-state" }).to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/auth/composio/callback")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    /// composio_oauth: None (feature not configured) → 501, never a panic.
    #[tokio::test]
    async fn post_composio_callback_not_configured_returns_501() {
        let app = build_router_with_composio(None);
        let body = serde_json::json!({
            "state": "irrelevant-state",
            "connected_account_id": "ca_123"
        })
        .to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/auth/composio/callback")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    }

    /// CR-02: /auth/exchange with a freshly inserted OTC returns 200 + {jwt, device_name}.
    #[tokio::test]
    async fn post_auth_exchange_valid_otc_returns_jwt() {
        let app = build_router_with_otc("BAST-TEST-1234", "mario-phone");
        let body = serde_json::json!({ "otc": "BAST-TEST-1234" }).to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/auth/exchange")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let val: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(
            val.get("jwt").and_then(|v| v.as_str()).is_some(),
            "jwt field missing: {val}"
        );
        assert_eq!(val["device_name"], "mario-phone");
        assert_eq!(val["owner_id"], "mario");
    }

    /// CR-02: new_otc_store() is callable and returns a usable Arc<RwLock<HashMap>>.
    #[test]
    fn new_otc_store_is_accessible() {
        let store = new_otc_store();
        store.try_write().unwrap().insert(
            "BAST-XY".to_string(),
            PairingGrant {
                owner_id: "alice".to_string(),
                device_name: "dev".to_string(),
                issued_at: std::time::Instant::now(),
            },
        );
        assert!(store.try_read().unwrap().contains_key("BAST-XY"));
    }

    // ── CR-01 / WR-01 JWT tests ──────────────────────────────────────────────

    /// Helper: mint a valid HS256 JWT signed with the test secret.
    fn mint_jwt(secret: &str, sub: &str, exp_offset_secs: i64) -> String {
        use jsonwebtoken::{encode, EncodingKey, Header};
        #[derive(serde::Serialize)]
        struct C {
            sub: String,
            device: String,
            exp: u64,
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let exp = if exp_offset_secs >= 0 {
            now + exp_offset_secs as u64
        } else {
            now.saturating_sub((-exp_offset_secs) as u64)
        };
        let claims = C {
            sub: sub.to_string(),
            device: sub.to_string(),
            exp,
        };
        encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(secret.as_bytes()),
        )
        .unwrap()
    }

    /// CR-01: valid JWT (signed with test-secret, sub="mario-phone") → 200 on /webhook.
    #[tokio::test]
    async fn post_webhook_valid_jwt_returns_200() {
        let app = build_router();
        let jwt = mint_jwt("test-secret", "mario-phone", 3600);
        let body = serde_json::json!({ "text": "hello" }).to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/webhook")
            .header("content-type", "application/json")
            .header("x-bastion-token", jwt)
            .body(Body::from(body))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    /// CR-01: JWT signed with a different key → 401.
    #[tokio::test]
    async fn post_webhook_jwt_wrong_key_returns_401() {
        let app = build_router();
        let jwt = mint_jwt("wrong-secret", "mario-phone", 3600);
        let body = serde_json::json!({ "text": "hello" }).to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/webhook")
            .header("content-type", "application/json")
            .header("x-bastion-token", jwt)
            .body(Body::from(body))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    /// CR-01: expired JWT → 401.
    #[tokio::test]
    async fn post_webhook_expired_jwt_returns_401() {
        let app = build_router();
        let jwt = mint_jwt("test-secret", "mario-phone", -3600);
        let body = serde_json::json!({ "text": "hello" }).to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/webhook")
            .header("content-type", "application/json")
            .header("x-bastion-token", jwt)
            .body(Body::from(body))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    /// CR-01: raw non-JWT string → 401 (backward compat — no match in owner_map either).
    #[tokio::test]
    async fn post_webhook_raw_non_jwt_returns_401() {
        let app = build_router();
        let body = serde_json::json!({ "text": "hello" }).to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/webhook")
            .header("content-type", "application/json")
            .header("x-bastion-token", "not-a-jwt-token-and-not-in-owner-map")
            .body(Body::from(body))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    /// Backward compat: static owner_map token still works after JWT decode addition.
    #[tokio::test]
    async fn post_webhook_static_owner_map_token_still_works() {
        let app = build_router(); // "token-mario" → "mario" in owner map
        let body = serde_json::json!({ "text": "hello" }).to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/webhook")
            .header("content-type", "application/json")
            .header("x-bastion-token", "token-mario")
            .body(Body::from(body))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    // ── CHAN-01 WhatsApp webhook tests ───────────────────────────────────────

    /// Test 1: GET verify handshake with the correct verify_token echoes hub.challenge.
    #[tokio::test]
    async fn get_whatsapp_webhook_correct_verify_token_returns_challenge() {
        let whatsapp =
            test_whatsapp_config(OwnerMap::default(), "test-app-secret", "test-verify-token");
        let (app, _turn_count) =
            build_router_with_map_and_whatsapp(OwnerMap::default(), Some(whatsapp));

        let req = Request::builder()
            .method("GET")
            .uri("/whatsapp/webhook?hub.mode=subscribe&hub.verify_token=test-verify-token&hub.challenge=CHALLENGE123")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&bytes), "CHALLENGE123");
    }

    /// Test 2: GET verify handshake with a wrong verify_token returns 403.
    #[tokio::test]
    async fn get_whatsapp_webhook_wrong_verify_token_returns_403() {
        let whatsapp =
            test_whatsapp_config(OwnerMap::default(), "test-app-secret", "test-verify-token");
        let (app, _turn_count) =
            build_router_with_map_and_whatsapp(OwnerMap::default(), Some(whatsapp));

        let req = Request::builder()
            .method("GET")
            .uri("/whatsapp/webhook?hub.mode=subscribe&hub.verify_token=WRONG&hub.challenge=CHALLENGE123")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    /// Test 3: correctly-signed POST with a text message from a phone in the
    /// WhatsApp owner map returns 200.
    #[tokio::test]
    async fn post_whatsapp_webhook_valid_signature_known_phone_returns_200() {
        let secret = "test-app-secret";
        let map = OwnerMap::from_pairs(&[("+5511999999999", "mario")]);
        let whatsapp = test_whatsapp_config(map, secret, "test-verify-token");
        let (app, _turn_count) =
            build_router_with_map_and_whatsapp(OwnerMap::default(), Some(whatsapp));

        let body = serde_json::json!({
            "entry": [{
                "changes": [{
                    "value": {
                        "messages": [{
                            "from": "+5511999999999",
                            "text": { "body": "oi" }
                        }]
                    }
                }]
            }]
        })
        .to_string();
        let sig = sign_whatsapp(secret, body.as_bytes());

        let req = Request::builder()
            .method("POST")
            .uri("/whatsapp/webhook")
            .header("content-type", "application/json")
            .header("X-Hub-Signature-256", sig)
            .body(Body::from(body))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    /// Test 4: an INCORRECT signature returns 403 and — via the turn counter — proves
    /// the agent turn (and therefore serde_json::from_slice / handle_whatsapp_message)
    /// was never reached (Pitfall 1 ordering).
    #[tokio::test]
    async fn post_whatsapp_webhook_bad_signature_returns_403_and_skips_turn() {
        let map = OwnerMap::from_pairs(&[("+5511999999999", "mario")]);
        let whatsapp = test_whatsapp_config(map, "test-app-secret", "test-verify-token");
        let (app, turn_count) =
            build_router_with_map_and_whatsapp(OwnerMap::default(), Some(whatsapp));

        let body = serde_json::json!({
            "entry": [{
                "changes": [{
                    "value": {
                        "messages": [{
                            "from": "+5511999999999",
                            "text": { "body": "oi" }
                        }]
                    }
                }]
            }]
        })
        .to_string();

        let req = Request::builder()
            .method("POST")
            .uri("/whatsapp/webhook")
            .header("content-type", "application/json")
            .header("X-Hub-Signature-256", "sha256=deadbeef")
            .body(Body::from(body))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            turn_count.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "agent turn must never be reached when the signature is invalid"
        );
    }

    /// Test 5: no WhatsApp configured (`state.whatsapp = None`) returns 404, not a panic.
    #[tokio::test]
    async fn post_whatsapp_webhook_not_configured_returns_404() {
        let (app, _turn_count) = build_router_with_map_and_whatsapp(OwnerMap::default(), None);

        let req = Request::builder()
            .method("POST")
            .uri("/whatsapp/webhook")
            .header("content-type", "application/json")
            .header("X-Hub-Signature-256", "sha256=deadbeef")
            .body(Body::from("{}"))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
