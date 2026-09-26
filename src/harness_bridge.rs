//! Bastion's MCP server, reachable from inside a harness session.
//!
//! A runtime-backed turn runs Claude Code (or another ACP agent) with its own
//! tool loop; to keep Bastion's memory, personas and capabilities within its
//! reach, the agent loop hands each session an MCP endpoint
//! (`SessionSpec::mcp_bridge`). This module serves that endpoint: the same
//! [`BastionMcpServer`](crate::mcp::server::BastionMcpServer) the operator can
//! enable for external clients, on a loopback listener of its own, with one
//! token per owner minted on first use and kept only in memory.
//!
//! Every call the harness makes still goes through `CapabilityRegistry::invoke`
//! — egress policy and the approval queue included — as the token's owner. The
//! token's privacy tier is `CloudOk` because the harness is itself a cloud
//! destination the owner chose for this conversation; resources stay filtered
//! per item against an external destination, as for any MCP client.
//!
//! Loopback TCP, not a Unix socket, because MCP clients connect by URL. The
//! listener binds `127.0.0.1` on an ephemeral port and answers nothing without
//! a 256-bit token, which exists only in this process and in the session spec
//! handed to the harness.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use bastion_agent_runtime::{McpBridgeSpec, McpServerEndpoint};
use bastion_memory::{PrivacyTier, SharedMemory};
use bastion_personas::persona::PersonaRegistry;
use bastion_runtime::agent::runtime_turn::RuntimeMcpBridge;
use bastion_runtime::capability::CapabilityRegistry;
use rand::RngCore;

use crate::control_plane::scope::ScopeSet;
use crate::mcp::server::{build_mcp_axum_router, TokenPermissions, TokenStore};

/// Name the harness sees the server under.
pub const SERVER_NAME: &str = "bastion";

const MOUNT_PATH: &str = "/mcp";

/// A running bridge: its URL and the per-owner tokens it accepts.
pub struct HarnessBridge {
    url: String,
    store: TokenStore,
    by_owner: Mutex<HashMap<String, String>>,
}

impl HarnessBridge {
    /// Binds `127.0.0.1:0` and serves Bastion's MCP server there until the
    /// process exits.
    pub async fn start(
        registry: Arc<CapabilityRegistry>,
        control_plane_registry: Arc<CapabilityRegistry>,
        memory: SharedMemory,
        personas: Arc<PersonaRegistry>,
        goals: bastion_cognition::goal::GoalEngine,
    ) -> anyhow::Result<Arc<Self>> {
        let store: TokenStore = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let router = build_mcp_axum_router(
            registry,
            control_plane_registry,
            memory,
            personas,
            goals,
            store.clone(),
            crate::control_plane::rate_limit::RateLimiter::new(),
            MOUNT_PATH,
        );
        let router = router.layer(axum::middleware::from_fn(log_request));
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
        let addr = listener.local_addr()?;
        tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, router).await {
                tracing::error!(event = "harness_bridge_stopped", error = %e);
            }
        });
        tracing::info!(event = "harness_bridge_started", %addr);
        Ok(Arc::new(Self {
            url: format!("http://{addr}{MOUNT_PATH}"),
            store,
            by_owner: Mutex::new(HashMap::new()),
        }))
    }

    /// Where the bridge listens (`http://127.0.0.1:<port>/mcp`).
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The endpoint a session of `owner` connects to, minting the owner's
    /// token the first time.
    pub fn spec_for(&self, owner: &str) -> McpBridgeSpec {
        let token = {
            let mut by_owner = self.by_owner.lock().unwrap_or_else(|e| e.into_inner());
            by_owner
                .entry(owner.to_string())
                .or_insert_with(|| {
                    let token = new_token();
                    self.store
                        .write()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert(token.clone(), permissions_for(owner));
                    token
                })
                .clone()
        };
        McpBridgeSpec {
            servers: vec![McpServerEndpoint::Http {
                name: SERVER_NAME.to_string(),
                url: self.url.clone(),
                headers: BTreeMap::from([("x-bastion-token".to_string(), token)]),
            }],
        }
    }

    /// The agent loop's view of this bridge.
    pub fn as_runtime_bridge(self: &Arc<Self>) -> RuntimeMcpBridge {
        let bridge = self.clone();
        Arc::new(move |owner: &str| Some(bridge.spec_for(owner)))
    }
}

/// One debug line per request: enough to tell "the harness never connected"
/// from "it connected and was refused". Never logs headers (the token).
async fn log_request(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let response = next.run(request).await;
    tracing::debug!(event = "harness_bridge_request", %method, %path, status = response.status().as_u16());
    response
}

/// Read-write for capabilities (each still subject to its own approval), no
/// Control Plane scopes: a harness has no business creating or steering
/// Bastion tasks.
fn permissions_for(owner: &str) -> TokenPermissions {
    TokenPermissions {
        read_only: false,
        owner_id: owner.to_string(),
        privacy_tier: PrivacyTier::CloudOk,
        control_plane_scopes: ScopeSet::new([]),
        control_plane_project: None,
    }
}

fn new_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bridge() -> HarnessBridge {
        HarnessBridge {
            url: "http://127.0.0.1:1/mcp".to_string(),
            store: Arc::new(std::sync::RwLock::new(HashMap::new())),
            by_owner: Mutex::new(HashMap::new()),
        }
    }

    fn token_of(spec: &McpBridgeSpec) -> String {
        match &spec.servers[0] {
            McpServerEndpoint::Http { headers, .. } => headers["x-bastion-token"].clone(),
            other => panic!("unexpected endpoint {other:?}"),
        }
    }

    #[test]
    fn each_owner_gets_one_stable_token_bound_to_them() {
        let bridge = bridge();
        let alice = token_of(&bridge.spec_for("alice"));
        assert_eq!(alice.len(), 64);
        assert_eq!(
            token_of(&bridge.spec_for("alice")),
            alice,
            "stable per owner"
        );
        let bob = token_of(&bridge.spec_for("bob"));
        assert_ne!(alice, bob);

        let store = bridge.store.read().unwrap();
        assert_eq!(store[&alice].owner_id, "alice");
        assert_eq!(store[&bob].owner_id, "bob");
        assert!(store[&alice].control_plane_scopes.0.is_empty());
    }

    #[test]
    fn the_endpoint_debug_output_hides_the_token() {
        let bridge = bridge();
        let spec = bridge.spec_for("alice");
        let token = token_of(&spec);
        assert!(!format!("{spec:?}").contains(&token));
    }
}
