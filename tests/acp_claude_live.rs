//! Live: Claude Code as the conversation runtime through `acp_claude`, on the
//! operator's own login, the way the daemon wires it — the registry probe,
//! OS confinement when this host has a backend, Bastion's MCP bridge, and a
//! permission request parked until the owner answers.
//!
//! Not run by default: spawns the real Claude Code ACP bridge and spends a few
//! turns of the operator's subscription. Needs `claude` logged in and `npx`
//! (or `claude-agent-acp`) on PATH:
//!
//! ```text
//! cargo test --test acp_claude_live -- --ignored --nocapture --test-threads=1
//! ```

use bastion_cognition::goal::{GoalEngine, ScoringConfig};
use bastion_memory::sqlite::SqliteMemory;
use bastion_memory::SharedMemory;
use bastion_personas::persona::{PersonaRegistry, PersonaResponder};
use bastion_providers::{Provider, SharedProvider};
use bastion_runtime::agent::backend::{BackendProfile, ConversationBackend};
use bastion_runtime::agent::loop_::AgentLoop;
use bastion_runtime::capability::approval::SqliteApprovalGate;
use bastion_runtime::capability::SqlitePermissionGate;
use bastion_runtime::session::SessionManager;
use bastion_types::{CallConfig, LlmResponse, Message};
use std::sync::Arc;
use tokio::sync::RwLock;

const MARKER: &str = "BASTION-BRIDGE-7431";

/// A Bastion capability the harness can only reach through the MCP bridge.
struct Marker;

#[async_trait::async_trait]
impl bastion_runtime::capability::Capability for Marker {
    fn name(&self) -> &str {
        "bastion_marker"
    }
    fn description(&self) -> &str {
        "Returns the Bastion bridge marker for the calling owner."
    }
    fn input_schema(&self) -> &serde_json::Value {
        static SCHEMA: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
        SCHEMA.get_or_init(|| serde_json::json!({"type": "object", "properties": {}}))
    }
    async fn invoke(
        &self,
        _args: serde_json::Value,
        ctx: &bastion_runtime::capability::InvokeCtx,
    ) -> anyhow::Result<serde_json::Value> {
        Ok(serde_json::json!({"marker": MARKER, "owner": ctx.owner}))
    }
}

struct UnusedProvider;

#[async_trait::async_trait]
impl Provider for UnusedProvider {
    async fn complete(&self, _: &[Message], _: &CallConfig) -> anyhow::Result<LlmResponse> {
        anyhow::bail!("the runtime-backed path never calls the provider")
    }
    async fn complete_simple(&self, _prompt: &str) -> anyhow::Result<String> {
        anyhow::bail!("the runtime-backed path never calls the provider")
    }
    fn context_limit(&self) -> usize {
        8192
    }
    fn model_name(&self) -> &str {
        "unused"
    }
    fn name(&self) -> &'static str {
        "unused"
    }
}

async fn make_loop(db_path: &str) -> AgentLoop {
    let session = SessionManager::new(db_path);
    session.init_schema().await.expect("init_schema");
    let session_id = session.create_session().await.expect("create_session");
    let memory: SharedMemory = Arc::new(RwLock::new(
        Box::new(SqliteMemory::new(db_path)) as Box<dyn bastion_memory::Memory>
    ));
    let mcp = Arc::new(
        bastion_mcp::McpClient::connect_from_config(&std::collections::HashMap::new())
            .await
            .expect("empty MCP config"),
    );
    let provider: SharedProvider = Arc::new(RwLock::new(Box::new(UnusedProvider)));
    AgentLoop::new(
        provider,
        session,
        Arc::new(bastion_mcp::McpToolSource::new(mcp)),
        session_id,
        10.0,
        Arc::new(PersonaResponder::new(PersonaRegistry::new_from_map(
            Default::default(),
        ))),
        memory.clone(),
        Some(Arc::new(GoalEngine::new(db_path, ScoringConfig::default()))),
        vec![],
        Arc::new(SqliteApprovalGate::new(db_path)),
        Arc::new(bastion_cognition::eval::failure_sink::EvalFailureSink),
        bastion::agent::default_context_providers(&memory),
        Arc::new(bastion_providers::registry::RegistryProviderResolver),
        None,
        None,
    )
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "spawns the real Claude Code ACP bridge and spends subscription turns"]
async fn claude_code_asks_bastion_before_writing_and_reaches_the_bridge() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("bastion=debug,bastion_agent_runtime=debug,rmcp=info")
        .with_writer(std::io::stderr)
        .try_init();
    let confined = bastion::sandbox::init_with_helper(env!("CARGO_BIN_EXE_bastion").into());
    eprintln!("sandbox: {:?}", confined.map(|s| s.backend()));

    let data = tempfile::tempdir().unwrap();
    let db = data.path().join("bastion.db");
    let db = db.to_str().unwrap();
    let workspace = data.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    let registry = bastion::agent_runtime_registry::build_runtime_registry(&workspace).await;
    let ids: Vec<&str> = registry.descriptors().iter().map(|d| d.id).collect();
    assert!(
        ids.contains(&"acp_claude"),
        "acp_claude not registered: {ids:?}"
    );

    let mut agent = make_loop(db)
        .await
        .with_backend_profile(BackendProfile {
            conversation: ConversationBackend::Runtime("acp_claude".to_string()),
            ..Default::default()
        })
        .with_runtime_registry(registry)
        .with_runtime_workspace_base(workspace.clone())
        .with_permission_gate(Arc::new(SqlitePermissionGate::new(db)));
    let mut capabilities = agent.capability_registry.clone();
    capabilities.register(Arc::new(Marker)).unwrap();
    let bridge = bastion::harness_bridge::HarnessBridge::start(
        Arc::new(capabilities),
        Arc::new(bastion_runtime::capability::CapabilityRegistry::new()),
        agent.memory.clone(),
        Arc::new(PersonaRegistry::new_from_map(Default::default())),
        GoalEngine::new(db, ScoringConfig::default()),
    )
    .await
    .unwrap();
    agent.runtime_mcp_bridge = Some(bridge.as_runtime_bridge());

    let owner = "live";
    let asked = agent
        .run_turn_for(
            "Create a file named notes.txt in the current directory whose entire content \
             is the single line: oi. Use your file-writing tool, not a shell command.",
            owner,
        )
        .await
        .expect("first turn");
    eprintln!("--- asked:\n{asked}");
    assert!(
        asked.contains("pede permissão"),
        "no parked request: {asked}"
    );
    let target = workspace.join(owner).join("notes.txt");
    assert!(!target.exists(), "written before the owner answered");

    let done = agent
        .run_turn_for("sim", owner)
        .await
        .expect("approval turn");
    eprintln!("--- done:\n{done}");
    assert_eq!(
        std::fs::read_to_string(&target)
            .map(|s| s.trim().to_string())
            .ok(),
        Some("oi".to_string()),
        "file after approval"
    );

    let mcp = agent
        .run_turn_for(
            "Call the bastion_marker tool from the MCP server named bastion and reply with \
             only the value of the marker field it returns.",
            owner,
        )
        .await
        .expect("bridge turn");
    eprintln!("--- mcp:\n{mcp}");
    assert!(mcp.contains(MARKER), "bridge not reached: {mcp}");
}
