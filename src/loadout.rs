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

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

use crate::channel::webhook::resolve_owner_or_401;
use crate::channel::OwnerMap;
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
}

async fn loadout_handler(
    State(state): State<LoadoutState>,
    headers: HeaderMap,
) -> axum::response::Response {
    if let Err(resp) = resolve_owner_or_401(
        &headers,
        &state.owner_map,
        &state.jwt_secret,
        "loadout_unauthorized",
    ) {
        return *resp;
    }
    Json(state.snapshot.as_ref().clone()).into_response()
}

/// Build the `/loadout` sub-router. Merged into the webhook app after
/// `.with_state` — same slot as `control_plane_routes`.
pub fn router(
    snapshot: LoadoutSnapshot,
    owner_map: OwnerMap,
    jwt_secret: String,
) -> Router {
    Router::new()
        .route("/loadout", get(loadout_handler))
        .with_state(LoadoutState {
            snapshot: Arc::new(snapshot),
            owner_map: Arc::new(owner_map),
            jwt_secret,
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

    fn sample_router(owner_map: OwnerMap) -> Router {
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
        router(snap, owner_map, "test-secret".into())
    }

    #[tokio::test]
    async fn loadout_requires_owner_token() {
        let app = sample_router(OwnerMap::default());
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
        let app = sample_router(owner_map);
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
