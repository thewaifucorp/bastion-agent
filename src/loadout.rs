//! `GET /loadout` — the daemon's assembled composition, for the web app's
//! Loadout view ("your bastion, piece by piece").
//!
//! Bastion's identity is *authority explicit*: you should be able to SEE
//! what your agent is assembled from and what each piece may do. This route
//! answers with the composition snapshot taken at boot — personas loaded
//! from `./personas/`, tools in the shared `CapabilityRegistry`, coding
//! runtimes, enabled channels, configured MCP servers, and installed
//! extension packs (honest empty until the `ExtensionHost` is wired into
//! the daemon: mechanism exists, product wiring is backlog).
//!
//! Owner-token authenticated (same `resolve_owner_or_401` as `/webhook`):
//! the composition fingerprints an installation — it is for its operator,
//! not the network. Snapshot semantics: values are captured when the daemon
//! boots; `POST /lifecycle/reload` reloads personas from disk for command
//! validation but this snapshot refreshes on restart (field `captured_at`
//! makes staleness visible instead of pretending liveness).

use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::channel::webhook::resolve_owner_or_401;
use crate::channel::OwnerMap;
use crate::proposals::{self, ProposalPayload, SqliteProposalStore};
use std::sync::Arc;

#[derive(Clone, Serialize)]
pub struct ChannelPiece {
    pub id: &'static str,
    pub enabled: bool,
}

#[derive(Clone, Serialize)]
pub struct RuntimePiece {
    pub id: String,
}

/// The assembled composition, captured at boot in `daemon_loop`.
#[derive(Clone, Serialize)]
pub struct LoadoutSnapshot {
    pub personas: Vec<String>,
    pub tools: Vec<String>,
    pub runtimes: Vec<RuntimePiece>,
    pub channels: Vec<ChannelPiece>,
    pub mcp_servers: Vec<String>,
    /// Always empty today: the sandboxed `ExtensionHost` mechanism exists
    /// (`src/extension/`) but nothing installs packs into the running
    /// daemon yet — reported honestly rather than omitted.
    pub extensions: Vec<String>,
    /// Nanoseconds since epoch when this snapshot was captured (boot time).
    pub captured_at: i64,
}

#[derive(Clone)]
struct LoadoutState {
    snapshot: Arc<LoadoutSnapshot>,
    owner_map: Arc<OwnerMap>,
    jwt_secret: String,
    /// A3: staged configuration proposals (web proposes, console approves).
    proposal_store: Arc<SqliteProposalStore>,
    events_tx: broadcast::Sender<String>,
}

fn auth(
    state: &LoadoutState,
    headers: &HeaderMap,
    event: &'static str,
) -> Result<String, Box<axum::response::Response>> {
    resolve_owner_or_401(headers, &state.owner_map, &state.jwt_secret, event)
}

async fn loadout_handler(
    State(state): State<LoadoutState>,
    headers: HeaderMap,
) -> axum::response::Response {
    if let Err(resp) = auth(&state, &headers, "loadout_unauthorized") {
        return *resp;
    }
    Json(state.snapshot.as_ref().clone()).into_response()
}

async fn personas_list_handler(
    State(state): State<LoadoutState>,
    headers: HeaderMap,
) -> axum::response::Response {
    if let Err(resp) = auth(&state, &headers, "personas_unauthorized") {
        return *resp;
    }
    let slugs = proposals::list_persona_slugs(&proposals::personas_root()).await;
    Json(serde_json::json!({ "items": slugs })).into_response()
}

async fn persona_read_handler(
    State(state): State<LoadoutState>,
    headers: HeaderMap,
    AxumPath(slug): AxumPath<String>,
) -> axum::response::Response {
    if let Err(resp) = auth(&state, &headers, "personas_unauthorized") {
        return *resp;
    }
    match proposals::read_persona(&proposals::personas_root(), &slug).await {
        Ok(Some(content)) => {
            Json(serde_json::json!({ "slug": slug, "content": content })).into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, "no such persona").into_response(),
        Err(_) => (StatusCode::BAD_REQUEST, "invalid persona slug").into_response(),
    }
}

async fn proposals_list_handler(
    State(state): State<LoadoutState>,
    headers: HeaderMap,
) -> axum::response::Response {
    let owner = match auth(&state, &headers, "proposals_unauthorized") {
        Ok(o) => o,
        Err(resp) => return *resp,
    };
    match state.proposal_store.list_for_owner(&owner).await {
        Ok(items) => Json(serde_json::json!({ "items": items })).into_response(),
        Err(e) => {
            tracing::warn!(event = "proposals_list_failed", error = %e);
            (StatusCode::INTERNAL_SERVER_ERROR, "proposal store error").into_response()
        }
    }
}

#[derive(Deserialize)]
struct CreateProposalRequest {
    kind: String,
    slug: String,
    content: String,
}

async fn proposals_create_handler(
    State(state): State<LoadoutState>,
    headers: HeaderMap,
    Json(req): Json<CreateProposalRequest>,
) -> axum::response::Response {
    let owner = match auth(&state, &headers, "proposals_unauthorized") {
        Ok(o) => o,
        Err(resp) => return *resp,
    };
    if req.kind != "persona_edit" {
        return (StatusCode::BAD_REQUEST, "unknown proposal kind").into_response();
    }
    if !proposals::is_safe_slug(&req.slug) {
        return (StatusCode::BAD_REQUEST, "invalid persona slug").into_response();
    }
    if req.content.len() > proposals::MAX_CONTENT_BYTES {
        return (StatusCode::PAYLOAD_TOO_LARGE, "content too large").into_response();
    }
    let payload = ProposalPayload::PersonaEdit {
        slug: req.slug,
        content: req.content,
    };
    match state.proposal_store.create(&owner, "web", &payload).await {
        Ok(p) => {
            // Attention plumbing: the operator sees the request wherever they
            // watch — SSE (dashboard/TUI ledger) — and approves on console.
            let _ = state.events_tx.send(
                serde_json::json!({
                    "event": "config.change_requested",
                    "owner": p.owner_id,
                    "proposal": p.id,
                    "origin": p.origin,
                })
                .to_string(),
            );
            (StatusCode::CREATED, Json(serde_json::json!(p))).into_response()
        }
        Err(e) => {
            tracing::warn!(event = "proposal_create_failed", error = %e);
            (StatusCode::INTERNAL_SERVER_ERROR, "proposal store error").into_response()
        }
    }
}

/// Build the operator sub-router: `/loadout`, persona reads, and staged
/// proposals. Merged into the webhook app after `.with_state` — same slot
/// as `control_plane_routes`.
pub fn router(
    snapshot: LoadoutSnapshot,
    owner_map: OwnerMap,
    jwt_secret: String,
    proposal_store: Arc<SqliteProposalStore>,
    events_tx: broadcast::Sender<String>,
) -> Router {
    Router::new()
        .route("/loadout", get(loadout_handler))
        .route("/personas", get(personas_list_handler))
        .route("/personas/{slug}", get(persona_read_handler))
        .route(
            "/proposals",
            get(proposals_list_handler).post(proposals_create_handler),
        )
        .with_state(LoadoutState {
            snapshot: Arc::new(snapshot),
            owner_map: Arc::new(owner_map),
            jwt_secret,
            proposal_store,
            events_tx,
        })
}

/// Capture the composition from the pieces `daemon_loop` already holds.
pub fn snapshot(
    persona_names: Vec<String>,
    tool_names: Vec<String>,
    runtime_ids: Vec<String>,
    channels: Vec<ChannelPiece>,
    mcp_servers: Vec<String>,
) -> LoadoutSnapshot {
    LoadoutSnapshot {
        personas: persona_names,
        tools: tool_names,
        runtimes: runtime_ids.into_iter().map(|id| RuntimePiece { id }).collect(),
        channels,
        mcp_servers,
        extensions: Vec::new(),
        captured_at: now_nanos(),
    }
}

fn now_nanos() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_router(owner_map: OwnerMap) -> (tempfile::NamedTempFile, Router) {
        let snap = snapshot(
            vec!["ada".into()],
            vec!["create_task".into()],
            vec!["codex_app_server".into()],
            vec![ChannelPiece {
                id: "webhook",
                enabled: true,
            }],
            vec!["memupalace".into()],
        );
        let f = tempfile::NamedTempFile::new().unwrap();
        let store = Arc::new(SqliteProposalStore::new(
            f.path().to_str().unwrap().to_owned(),
        ));
        let (events_tx, _) = broadcast::channel(8);
        (
            f,
            router(snap, owner_map, "test-secret".into(), store, events_tx),
        )
    }

    #[tokio::test]
    async fn loadout_requires_owner_token() {
        let (_f, app) = sample_router(OwnerMap::default());
        let req = axum::http::Request::builder()
            .uri("/loadout")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = tower::ServiceExt::oneshot(app, req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn loadout_answers_composition_for_a_valid_token() {
        let owner_map = OwnerMap::from_pairs(&[("tok-alice", "alice")]);
        let (_f, app) = sample_router(owner_map);
        let req = axum::http::Request::builder()
            .uri("/loadout")
            .header("x-bastion-token", "tok-alice")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = tower::ServiceExt::oneshot(app, req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["personas"], serde_json::json!(["ada"]));
        assert_eq!(v["extensions"], serde_json::json!([]));
        assert!(v["captured_at"].as_i64().unwrap() > 0);
    }
}
