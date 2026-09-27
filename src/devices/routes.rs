//! The primary's HTTP surface for the owner's devices, merged into the
//! webhook server. Public routes let a device pair and poll and let any
//! client find the primary; the rest are gated by the daemon token.

use std::sync::Arc;

use async_trait::async_trait;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use base64::Engine;
use bastion_mesh::devices::reconcile::Resolution;
use bastion_mesh::devices::transport::FrameConn;
use bastion_mesh::devices::{CapabilityGrant, DeviceId, Platform};
use serde::{Deserialize, Serialize};

use super::primary::PrimaryDevices;
use crate::channel::operational::{require_daemon_access, DaemonAccessAuth};

type Shared = Arc<PrimaryDevices>;

/// A node's WebSocket seen as a [`FrameConn`].
struct AxumConn(WebSocket);

#[async_trait]
impl FrameConn for AxumConn {
    async fn send(&mut self, text: String) -> anyhow::Result<()> {
        self.0.send(Message::Text(text.into())).await?;
        Ok(())
    }

    async fn recv(&mut self) -> anyhow::Result<Option<String>> {
        loop {
            match self.0.recv().await {
                None => return Ok(None),
                Some(Err(e)) => return Err(e.into()),
                Some(Ok(Message::Text(text))) => return Ok(Some(text.to_string())),
                Some(Ok(Message::Close(_))) => return Ok(None),
                Some(Ok(_)) => continue,
            }
        }
    }

    async fn close(&mut self) {
        let _ = self.0.send(Message::Close(None)).await;
    }
}

fn err(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({ "error": message.into() }))).into_response()
}

/// Builds the device routes. `admin` is the same daemon-token check the
/// lifecycle routes use, layered over the owner-only half.
pub fn router(devices: Shared, admin: DaemonAccessAuth) -> Router {
    let public = Router::new()
        .route("/node", get(node_socket))
        .route("/devices/enroll", post(enroll))
        .route("/devices/enroll/{id}", get(enroll_status))
        .route("/devices/primary", get(primary))
        .with_state(devices.clone());
    let owner = Router::new()
        .route("/devices", get(list))
        .route("/devices/pairing-codes", post(new_code))
        .route("/devices/requests/{id}/approve", post(approve))
        .route("/devices/requests/{id}/refuse", post(refuse))
        .route("/devices/{id}/grants", put(set_grants))
        .route("/devices/{id}/address", put(set_address))
        .route("/devices/{id}/revoke", post(revoke))
        .route("/devices/conflicts", get(conflicts))
        .route("/devices/conflicts/{id}", post(resolve_conflict))
        .layer(axum::middleware::from_fn_with_state(
            admin,
            require_daemon_access,
        ))
        .with_state(devices);
    public.merge(owner)
}

// ─── public ──────────────────────────────────────────────────────────────

/// The node dials in here (BMD-09): upgrade to a WebSocket and hand it to
/// the hub, which authenticates the device before doing anything.
async fn node_socket(State(devices): State<Shared>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| async move {
        devices.hub().serve(AxumConn(socket)).await;
    })
}

#[derive(Deserialize)]
struct EnrollBody {
    code: String,
    device: String,
    /// Ed25519 public key, base64url.
    device_key: String,
    platform: Platform,
    #[serde(default)]
    holds_replica: bool,
}

#[derive(Serialize)]
struct EnrollAccepted {
    request_id: String,
}

/// A device asks to join, spending a one-time pairing code. It gets a
/// request id to poll; the owner still has to approve (BMD-08).
async fn enroll(State(devices): State<Shared>, Json(body): Json<EnrollBody>) -> Response {
    match devices.pending().take_code(&body.code) {
        Ok(true) => {}
        Ok(false) => return err(StatusCode::FORBIDDEN, "invalid or expired pairing code"),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
    if base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(&body.device_key)
        .ok()
        .filter(|k| k.len() == 32)
        .is_none()
    {
        return err(
            StatusCode::BAD_REQUEST,
            "device_key must be a 32-byte base64url key",
        );
    }
    match devices.pending().add_request(
        DeviceId::new(body.device),
        body.device_key,
        body.platform,
        body.holds_replica,
    ) {
        Ok(request) => (
            StatusCode::ACCEPTED,
            Json(EnrollAccepted {
                request_id: request.id,
            }),
        )
            .into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Serialize)]
struct EnrollStatus {
    status: super::pending::RequestStatus,
    /// The owner's public key (needed to verify the primary), once approved.
    owner_key: Option<String>,
    /// Where to dial the primary (`/node`), once approved.
    primary_address: Option<String>,
}

/// The pairing device polls its request. Once approved it learns the owner
/// key and the primary's address so it can connect as a node.
async fn enroll_status(State(devices): State<Shared>, Path(id): Path<String>) -> Response {
    let Some(request) = devices.pending().request(&id) else {
        return err(StatusCode::NOT_FOUND, "no such request");
    };
    let (owner_key, primary_address) = if request.status == super::pending::RequestStatus::Approved
    {
        let info = devices.primary_info().await;
        let key = devices
            .owner_public_key()
            .ok()
            .map(|k| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(k));
        (key, info.address)
    } else {
        (None, None)
    };
    Json(EnrollStatus {
        status: request.status,
        owner_key,
        primary_address,
    })
    .into_response()
}

/// Who the primary is and where — the discovery endpoint clients use (§5.6).
async fn primary(State(devices): State<Shared>) -> Response {
    Json(devices.primary_info().await).into_response()
}

// ─── owner (daemon token) ──────────────────────────────────────────────────

async fn list(State(devices): State<Shared>) -> Response {
    let registry = devices.registry_handle().read().await;
    let connected = devices.hub().connected().await;
    let devices_json: Vec<_> = registry
        .devices()
        .map(|r| {
            serde_json::json!({
                "device": r.enrollment.device,
                "platform": r.enrollment.platform,
                "role": r.role,
                "revoked": r.revoked,
                "holds_replica": r.enrollment.holds_replica,
                "address": r.address,
                "granted": r.enrollment.granted,
                "connected": connected.contains(&r.enrollment.device),
            })
        })
        .collect();
    Json(serde_json::json!({
        "owner": registry.owner(),
        "current_epoch": registry.current_epoch(),
        "devices": devices_json,
        "requests": devices.pending().requests(),
    }))
    .into_response()
}

#[derive(Serialize)]
struct NewCode {
    code: String,
    expires_at: i64,
}

async fn new_code(State(devices): State<Shared>) -> Response {
    match devices.pending().new_code() {
        Ok((code, expires_at)) => Json(NewCode { code, expires_at }).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn approve(State(devices): State<Shared>, Path(id): Path<String>) -> Response {
    match devices.approve_request(&id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

async fn refuse(State(devices): State<Shared>, Path(id): Path<String>) -> Response {
    match devices.refuse_request(&id) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

#[derive(Deserialize)]
struct GrantsBody {
    grants: Vec<CapabilityGrant>,
}

async fn set_grants(
    State(devices): State<Shared>,
    Path(id): Path<String>,
    Json(body): Json<GrantsBody>,
) -> Response {
    match devices.set_grants(&DeviceId::new(id), body.grants).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

#[derive(Deserialize)]
struct AddressBody {
    address: Option<String>,
}

async fn set_address(
    State(devices): State<Shared>,
    Path(id): Path<String>,
    Json(body): Json<AddressBody>,
) -> Response {
    match devices.set_address(&DeviceId::new(id), body.address).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

async fn revoke(State(devices): State<Shared>, Path(id): Path<String>) -> Response {
    match devices.revoke_device(&DeviceId::new(id)).await {
        Ok(rotate) => Json(serde_json::json!({ "rotate_secrets": rotate })).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

async fn conflicts(State(devices): State<Shared>) -> Response {
    match devices.conflicts().pending() {
        Ok(list) => Json(serde_json::json!({ "conflicts": list })).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Deserialize)]
struct ResolveBody {
    /// `keep_ours` or `take_theirs`.
    decision: String,
}

async fn resolve_conflict(
    State(devices): State<Shared>,
    Path(id): Path<i64>,
    Json(body): Json<ResolveBody>,
) -> Response {
    let resolution = match body.decision.as_str() {
        "keep_ours" => Resolution::KeepOurs,
        "take_theirs" => Resolution::TakeTheirs,
        other => {
            return err(
                StatusCode::BAD_REQUEST,
                format!("unknown decision {other:?}"),
            )
        }
    };
    match devices.resolve_conflict(id, resolution).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, e.to_string()),
    }
}
