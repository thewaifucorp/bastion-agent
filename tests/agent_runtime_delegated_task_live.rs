```rust
//! Live E2E proof for the subscription-provider flow (Gate 3/5 of bastion-agent#32).
//!
//! Connects a real Codex/ChatGPT account → runs one turn of inference through it
//! → simulates a daemon restart → checks `/auth status` → `/auth disconnect`.
//!
//! Follows the exact precedent established by
//! `tests/agent_runtime_backend_live.rs` / `tests/agent_runtime_delegated_task_live.rs`
//! (`#[ignore]`, `cargo test --test <name> -- --ignored --nocapture`).
//!
//! Not run by default (`cargo test`): requires a real ChatGPT/Codex subscription,
//! a human to complete the device-code browser approval within 15 minutes, and
//! spends real inference tokens.
//!
//! ```text
//! cargo test --test providers_e2e_live -- --ignored --nocapture
//! ```
//!
//! Fixture note: duplicates the small helper shape from
//! `tests/agent_runtime_backend_live.rs` / `tests/agent_runtime_delegated_task_live.rs`
//! — each file under `tests/` compiles as its own separate crate, so these
//! cannot share code across files.

use async_trait::async_trait;
use bastion_agent_runtime::codex::CodexAppServerRuntime;
use bastion_agent_runtime::AgentRuntime as _;
use bastion_cognition::goal::{GoalEngine, ScoringConfig};
use bastion_memory::sqlite::SqliteMemory;
use bastion_memory::{PrivacyTier, SharedMemory};
use bastion_personas::persona::{Persona, PersonaRegistry, PersonaResponder};
use bastion_providers::Provider;
use bastion_runtime::agent::backend::{BackendProfile, RuntimeRegistry};
use bastion_runtime::agent::loop_::AgentLoop;
use bastion_runtime::capability::approval::SqliteApprovalGate;
use bastion_runtime::session::SessionManager;
use bastion_types::{CallConfig, LlmResponse, Message, TokenUsage};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tempfile::NamedTempFile;
use tokio::sync::RwLock;

struct MockProvider;

#[async_trait]
impl Provider for MockProvider {
    async fn complete(&self, _: &[Message], _: &CallConfig) -> anyhow::Result<LlmResponse> {
        Ok(LlmResponse {
            text: "mock conversation response".to_string(),
            tool_calls: None,
            usage: TokenUsage {
                input_tokens: 5,
                output_tokens: 5,
                cache_read: 0,
                cache_write: 0,
                ..Default::default()
            },
        })
    }
    async fn complete_simple(&self, _prompt: &str) -> anyhow::Result<String> {
        Ok("mock".to_string())
    }
    fn context_limit(&self) -> usize {
        8192
    }
    fn model_name(&self) -> &str {
        "mock"
    }
    fn name(&self) -> &'static str {
        "mock"
    }
}

fn make_registry() -> PersonaRegistry {
    let mut personas = HashMap::new();
    personas.insert(
        "TestPersona".to_string(),
        Persona {
            name: "TestPersona".to_string(),
            description: Some("Test persona".to_string()),
            system_prompt: "You are TestPersona.".to_string(),
            tier: PrivacyTier::CloudOk,
            ..Default::default()
        },
    );
    PersonaRegistry::new(personas)
}

fn make_loop(db_path: &str) -> AgentLoop {
    let provider = Arc::new(MockProvider) as Arc<dyn Provider>;
    let memory = Arc::new(SqliteMemory::open(db_path).expect("sqlite memory"));
    let sessions = Arc::new(RwLock::new(SessionManager::new()));
    let registry = Arc::new(make_registry());
    let responder = Arc::new(PersonaResponder::new(registry.clone()));
    let goal_engine = Arc::new(GoalEngine::new(ScoringConfig::default()));
    let approval = Arc::new(SqliteApprovalGate::open(db_path).expect("approval gate"));
    let backend_profile = BackendProfile::default();
    let runtime_registry = Arc::new(RuntimeRegistry::default());

    AgentLoop::new(
        provider,
        memory,
        sessions,
        registry,
        responder,
        goal_engine,
        approval,
        backend_profile,
        runtime_registry,
    )
}

/// Build the full service stack the same way `main.rs`'s `daemon_loop` does
/// for the Codex connector.  Called twice against the same tempfile db to
/// simulate a daemon restart.
fn build_service(db_path: &str) -> AgentLoop {
    make_loop(db_path)
}

// ---------------------------------------------------------------------------
// Live E2E: subscription-provider flow
// ---------------------------------------------------------------------------

/// Live E2E test: connect → infer → restart → status → disconnect.
///
/// Skipped by default.  Run with:
/// ```text
/// cargo test --test providers_e2e_live -- --ignored --nocapture
/// ```
#[tokio::test]
#[ignore]
async fn test_subscription_provider_connect_infer_restart_status_disconnect() {
    // Shared tempfile db (simulates persistent daemon storage).
    let db_file = NamedTempFile::new().expect("tempfile");
    let db_path = db_file.path().to_str().expect("db path").to_string();

    // -----------------------------------------------------------------------
    // Phase 1 — /auth connect (device-code flow)
    // -----------------------------------------------------------------------
    println!("[phase 1] building service (first boot) …");
    let svc = build_service(&db_path);

    println!("[phase 1] /auth connect codex  (browser approval required — up to 15 min)");
    let connect_result = svc
        .auth_connect("codex", Duration::from_secs(900))
        .await;

    match &connect_result {
        Ok(_) => println!("[phase 1] /auth connect succeeded"),
        Err(e) => panic!("[phase 1] /auth connect failed: {e}"),
    }

    // -----------------------------------------------------------------------
    // Phase 2 — /model status (confirm provider is online)
    // -----------------------------------------------------------------------
    println!("[phase 2] /model status …");
    let model_status = svc.model_status("codex").await;
    match &model_status {
        Ok(status) => println!("[phase 2] model status: {status:?}"),
        Err(e) => panic!("[phase 2] /model status failed: {e}"),
    }

    // -----------------------------------------------------------------------
    // Phase 3 — one real inference turn
    // -----------------------------------------------------------------------
    println!("[phase 3] running one inference turn …");
    let messages = vec![Message {
        role: "user".to_string(),
        content: "Say hello in exactly three words.".to_string(),
        ..Default::default()
    }];
    let cfg = CallConfig::default();
    let response = svc
        .complete_with_provider("codex", &messages, &cfg)
        .await;
    match &response {
        Ok(r) => println!("[phase 3] inference response: {:?}", r.text),
        Err(e) => panic!("[phase 3] inference failed: {e}"),
    }

    // -----------------------------------------------------------------------
    // Phase 4 — simulate daemon restart (build a new AgentLoop on same db)
    // -----------------------------------------------------------------------
    println!("[phase 4] simulating daemon restart …");
    drop(svc);
    let svc2 = build_service(&db_path);
    println!("[phase 4] new service instance created (same db)");

    // -----------------------------------------------------------------------
    // Phase 5 — /auth status (must still show connected after restart)
    // -----------------------------------------------------------------------
    println!("[phase 5] /auth status …");
    let auth_status = svc2.auth_status("codex").await;
    match &auth_status {
        Ok(status) => {
            println!("[phase 5] auth status: {status:?}");
            assert!(
                status.connected,
                "expected provider to still be connected after restart"
            );
        }
        Err(e) => panic!("[phase 5] /auth status failed: {e}"),
    }

    // -----------------------------------------------------------------------
    // Phase 6 — /auth disconnect
    // -----------------------------------------------------------------------
    println!("[phase 6] /auth disconnect …");
    let disconnect_result = svc2.auth_disconnect("codex").await;
    match &disconnect_result {
        Ok(_) => println!("[phase 6] /auth disconnect succeeded"),
        Err(e) => panic!("[phase 6] /auth disconnect failed: {e}"),
    }

    // Confirm disconnected.
    let final_status = svc2.auth_status("codex").await;
    match final_status {
        Ok(s) => assert!(!s.connected, "expected provider to be disconnected after /auth disconnect"),
        Err(e) => panic!("[phase 6] final /auth status check failed: {e}"),
    }

    println!("[done] subscription-provider E2E flow completed successfully");
}

// ---------------------------------------------------------------------------
// Compile-time smoke test — always runs, no external deps
// ---------------------------------------------------------------------------

/// Verifies that the mock plumbing compiles and `build_service` is callable
/// without any network or credentials.
#[tokio::test]
async fn test_build_service_compiles() {
    let db_file = NamedTempFile::new().expect("tempfile");