//! The harness MCP bridge speaks Streamable HTTP to a standard MCP client:
//! the per-owner token travels as an HTTP header (the only place a client
//! like Claude Code can put it), and a request without it is refused.

use bastion_agent_runtime::McpServerEndpoint;
use bastion_cognition::goal::{GoalEngine, ScoringConfig};
use bastion_memory::sqlite::SqliteMemory;
use bastion_memory::SharedMemory;
use bastion_personas::persona::PersonaRegistry;
use bastion_runtime::capability::CapabilityRegistry;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::RwLock;

struct Echo;

#[async_trait::async_trait]
impl bastion_runtime::capability::Capability for Echo {
    fn name(&self) -> &str {
        "echo_owner"
    }
    fn description(&self) -> &str {
        "Echoes the calling owner."
    }
    fn input_schema(&self) -> &Value {
        static SCHEMA: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
        SCHEMA.get_or_init(|| json!({"type": "object", "properties": {}}))
    }
    async fn invoke(
        &self,
        _args: Value,
        ctx: &bastion_runtime::capability::InvokeCtx,
    ) -> anyhow::Result<Value> {
        Ok(json!({"owner": ctx.owner}))
    }
}

async fn rpc(
    client: &reqwest::Client,
    url: &str,
    token: Option<&str>,
    session: Option<&str>,
    body: Value,
) -> (reqwest::StatusCode, Option<String>, Option<Value>) {
    let mut request = client
        .post(url)
        .header("accept", "application/json, text/event-stream")
        .header("content-type", "application/json")
        .json(&body);
    if let Some(token) = token {
        request = request.header("x-bastion-token", token);
    }
    if let Some(session) = session {
        request = request.header("mcp-session-id", session);
    }
    let response = request.send().await.unwrap();
    let status = response.status();
    let session = response
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let text = response.text().await.unwrap();
    // SSE framing: the JSON-RPC message is on a `data:` line.
    let message = text
        .lines()
        .filter_map(|l| l.strip_prefix("data:"))
        .map(str::trim)
        .find(|l| !l.is_empty())
        .or_else(|| (!text.trim().is_empty()).then_some(text.trim()))
        .and_then(|l| serde_json::from_str(l).ok());
    (status, session, message)
}

#[tokio::test]
async fn a_standard_mcp_client_authenticates_with_the_header_token() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("b.db");
    let db = db.to_str().unwrap();
    let memory: SharedMemory = Arc::new(RwLock::new(
        Box::new(SqliteMemory::new(db)) as Box<dyn bastion_memory::Memory>
    ));
    let mut capabilities = CapabilityRegistry::new();
    capabilities.register(Arc::new(Echo)).unwrap();
    let bridge = bastion::harness_bridge::HarnessBridge::start(
        Arc::new(capabilities),
        Arc::new(CapabilityRegistry::new()),
        memory,
        Arc::new(PersonaRegistry::new_from_map(Default::default())),
        GoalEngine::new(db, ScoringConfig::default()),
    )
    .await
    .unwrap();

    let spec = bridge.spec_for("alice");
    let McpServerEndpoint::Http { url, headers, .. } = &spec.servers[0] else {
        panic!("http endpoint expected");
    };
    assert!(url.starts_with("http://127.0.0.1:"), "{url}");
    let token = headers["x-bastion-token"].as_str();

    let client = reqwest::Client::new();
    let (status, session, init) = rpc(
        &client,
        url,
        Some(token),
        None,
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion":"2025-06-18","capabilities":{},
            "clientInfo":{"name":"test","version":"0"}}}),
    )
    .await;
    assert!(status.is_success(), "initialize: {status} {init:?}");
    assert_eq!(init.unwrap()["result"]["serverInfo"]["name"], "bastion");
    let session = session.expect("session id");
    rpc(
        &client,
        url,
        Some(token),
        Some(&session),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    )
    .await;

    let (_, _, listed) = rpc(
        &client,
        url,
        Some(token),
        Some(&session),
        json!({"jsonrpc":"2.0","id":2,"method":"resources/list"}),
    )
    .await;
    let listed = listed.expect("resources/list reply");
    assert!(
        listed["result"]["resources"]
            .as_array()
            .is_some_and(|r| !r.is_empty()),
        "authorized listing: {listed}"
    );

    let (_, _, tools) = rpc(
        &client,
        url,
        Some(token),
        Some(&session),
        json!({"jsonrpc":"2.0","id":4,"method":"tools/list"}),
    )
    .await;
    let tools = tools.expect("tools/list reply");
    assert_eq!(tools["result"]["tools"][0]["name"], "echo_owner", "{tools}");

    let (_, _, called) = rpc(
        &client,
        url,
        Some(token),
        Some(&session),
        json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"echo_owner","arguments":{}}}),
    )
    .await;
    let called = called.expect("tools/call reply");
    let text = called["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default();
    assert!(
        text.contains("alice"),
        "runs as the token's owner: {called}"
    );

    let (_, _, refused) = rpc(
        &client,
        url,
        None,
        Some(&session),
        json!({"jsonrpc":"2.0","id":3,"method":"resources/list"}),
    )
    .await;
    let refused = refused.expect("resources/list reply");
    assert!(
        refused.get("error").is_some(),
        "no token must be refused: {refused}"
    );
}
