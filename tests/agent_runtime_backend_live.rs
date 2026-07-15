//! Ciclo 2.4 — A-06 live proof
//! (`docs/revamp/C2-backend-profile-design.md` §4): one turn of conversation
//! entirely served by `AcpxAgentRuntime`→Claude Code, through the REAL
//! daemon path (`AgentLoop::run_turn_for`, not just adapter-level
//! conformance). Placar em `docs/revamp/A-06-A-07-live.md`.
//!
//! Not run by default (`cargo test`): spawns a real `acpx`/`claude`
//! subprocess, costs real tokens, and depends on host state (`acpx` +
//! `claude` installed, `claude` already authenticated). Run manually:
//!
//! ```text
//! cargo test --test agent_runtime_backend_live -- --ignored --nocapture
//! ```
//!
//! Fixture note: this integration test binary cannot `use` the fixture
//! helpers in `tests/agent_loop_public.rs` (each file under `tests/` compiles
//! as its own separate crate) — the minimal `make_loop` below is a
//! deliberate, small duplication of that file's fixture shape, not a design
//! choice specific to this test.

use bastion_agent_runtime::acpx::AcpxAgentRuntime;
use bastion_agent_runtime::AgentRuntime as _;
use bastion_cognition::goal::{GoalEngine, ScoringConfig};
use bastion_memory::sqlite::SqliteMemory;
use bastion_memory::SharedMemory;
use bastion_personas::persona::{PersonaRegistry, PersonaResponder};
use bastion_providers::{Provider, SharedProvider};
use bastion_runtime::agent::backend::{BackendProfile, ConversationBackend, RuntimeRegistry};
use bastion_runtime::agent::loop_::AgentLoop;
use bastion_runtime::capability::approval::SqliteApprovalGate;
use bastion_runtime::session::SessionManager;
use bastion_types::{CallConfig, LlmResponse, Message, Role};
use std::sync::Arc;
use tempfile::NamedTempFile;
use tokio::sync::RwLock;

/// Never actually dispatched (the runtime-backed path bypasses `self.provider`
/// entirely) — only present because `AgentLoop::new` requires one.
struct UnusedProvider;

#[async_trait::async_trait]
impl Provider for UnusedProvider {
    async fn complete(&self, _: &[Message], _: &CallConfig) -> anyhow::Result<LlmResponse> {
        anyhow::bail!("UnusedProvider must never be called on the runtime-backed path")
    }
    async fn complete_simple(&self, _prompt: &str) -> anyhow::Result<String> {
        anyhow::bail!("UnusedProvider must never be called on the runtime-backed path")
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

fn make_unused_provider() -> SharedProvider {
    Arc::new(RwLock::new(Box::new(UnusedProvider) as Box<dyn Provider>))
}

async fn make_loop(db_path: &str) -> AgentLoop {
    let session = SessionManager::new(db_path);
    session.init_schema().await.expect("init_schema");
    let session_id = session.create_session().await.expect("create_session");

    let memory: SharedMemory = Arc::new(RwLock::new(
        Box::new(SqliteMemory::new(db_path)) as Box<dyn bastion_memory::Memory>
    ));

    let mcp = Arc::new(
        bastion_mcp::McpClient::connect_all("nonexistent_mcp.json")
            .await
            .expect("connect_all empty"),
    );

    AgentLoop::new(
        make_unused_provider(),
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
        Some(Arc::new(bastion_cognition::agent::dream::DreamFlush::new(
            memory.clone(),
        ))),
        Some(Arc::new(bastion::agent::skills::SkillReloadObserver)),
    )
}

const A06_MARKER: &str = "BASTION-A06-OK";

#[tokio::test]
#[ignore = "spawns real acpx+claude subprocesses, costs tokens; run manually with --ignored"]
async fn a06_runtime_backed_conversation_live() {
    let f = NamedTempFile::new().unwrap();
    let db_path = f.path().to_str().unwrap().to_owned();
    let mut agent = make_loop(&db_path).await;

    let acpx = AcpxAgentRuntime::new("claude").expect("acpx on PATH");
    let health = acpx.health().await.expect("health probe");
    eprintln!("health: {health:?}");
    assert!(health.ready, "acpx/claude not ready: {health:?}");

    let mut registry = RuntimeRegistry::new();
    registry.register(Arc::new(acpx));

    agent = agent
        .with_backend_profile(BackendProfile {
            conversation: ConversationBackend::Runtime("acpx_claude".to_string()),
            ..Default::default()
        })
        .with_runtime_registry(registry);

    let owner = "a06-live-owner";
    let response = agent
        .run_turn_for(
            &format!("Reply with exactly this and nothing else: {A06_MARKER}"),
            owner,
        )
        .await
        .expect("runtime-backed turn must succeed end-to-end through the daemon path");
    eprintln!("response: {response:?}");

    assert!(
        response.contains(A06_MARKER),
        "expected the marker word in the runtime-backed response (proves the response \
         actually came back through AgentLoop::run_turn_for, not a stub), got: {response:?}"
    );

    // "memória grava a resposta" (design doc §3): the assistant response is
    // persisted to the Bastion session — same conversation record the Model
    // path writes, even though the harness owned this turn's tool-loop.
    let session_id = agent
        .session
        .load_most_recent_id_for(owner)
        .await
        .expect("load session id")
        .expect("a session must exist for this owner after the turn");
    let history = agent
        .session
        .load_recent(&session_id)
        .await
        .expect("load history");
    let last_assistant_has_marker = history.iter().rev().any(|m| {
        m.role == Role::Assistant
            && matches!(&m.content, bastion_types::MessageContent::Text(t) if t.contains(A06_MARKER))
    });
    assert!(
        last_assistant_has_marker,
        "the runtime-backed response must be persisted to session history, got: {history:?}"
    );
}

const M4_07_MARKER: &str = "BASTION-M4-07-OK";

/// M4-07 acceptance criterion (`docs/revamp/BACKLOG.md`): "instalação
/// pessoal funciona SEM API key quando há assinatura suportada". This is
/// [`a06_runtime_backed_conversation_live`] reconfigured to prove the
/// specific M4-07 machinery on top of A-06's proof that the runtime-backed
/// path itself works: a `[auth.host-claude-login]`-shaped config profile is
/// verified by the real `AuthProfileRegistry` (spawns `claude auth status`,
/// a read-only, non-secret-revealing check), wired as the `AgentLoop`'s
/// `AuthResolver`, and a full turn completes successfully — all while this
/// process has ZERO `*_API_KEY`-suffixed environment variable set. Traditional
/// API-key auth is never touched or required by this path.
#[tokio::test]
#[ignore = "spawns real acpx+claude subprocesses, costs tokens; run manually with --ignored"]
async fn m4_07_subscription_backend_works_without_api_key_live() {
    // The acceptance criterion itself, checked first and loudly: this
    // process must not be carrying any *_API_KEY env var when this proof
    // runs, or the test would prove nothing about "works without one".
    let leaked_api_keys: Vec<String> = std::env::vars()
        .map(|(k, _)| k)
        .filter(|k| k.to_ascii_uppercase().ends_with("_API_KEY"))
        .collect();
    assert!(
        leaked_api_keys.is_empty(),
        "M4-07 proof requires a *_API_KEY-free environment to be meaningful; found: {leaked_api_keys:?}"
    );

    let f = NamedTempFile::new().unwrap();
    let db_path = f.path().to_str().unwrap().to_owned();
    let mut agent = make_loop(&db_path).await;

    let acpx = AcpxAgentRuntime::new("claude").expect("acpx on PATH");
    let health = acpx.health().await.expect("health probe");
    eprintln!("health: {health:?}");
    assert!(health.ready, "acpx/claude not ready: {health:?}");

    let mut registry = RuntimeRegistry::new();
    registry.register(Arc::new(acpx));

    // M4-07: a real, config-shaped subscription auth profile — verified via
    // the CLI's own read-only status surface, never a token.
    let profile_id = "host-claude-login";
    let mut profiles = std::collections::HashMap::new();
    profiles.insert(
        profile_id.to_string(),
        bastion::config::AuthProfileEntry::HostCli {
            cli: "claude".to_string(),
        },
    );
    let auth_config = bastion::config::AuthConfig { profiles };
    let auth_registry =
        bastion::auth_profile_registry::AuthProfileRegistry::build(&auth_config).await;

    agent = agent
        .with_backend_profile(BackendProfile {
            conversation: ConversationBackend::Runtime("acpx_claude".to_string()),
            auth: Some(bastion_agent_runtime::AuthProfileRef(
                profile_id.to_string(),
            )),
            ..Default::default()
        })
        .with_runtime_registry(registry)
        .with_auth_resolver(Arc::new(auth_registry));

    let owner = "m4-07-live-owner";
    let response = agent
        .run_turn_for(
            &format!("Reply with exactly this and nothing else: {M4_07_MARKER}"),
            owner,
        )
        .await
        .expect(
            "subscription-backed turn must succeed end-to-end with zero *_API_KEY in the \
             environment — this is the M4-07 acceptance criterion itself",
        );
    eprintln!("response: {response:?}");

    assert!(
        response.contains(M4_07_MARKER),
        "expected the marker word in the response, got: {response:?}"
    );
}

/// M4-07: an `AuthProfileRef` naming a profile that was never configured (or
/// failed host verification) must fail the turn with a typed error BEFORE
/// any harness process is even spawned — never a silent proceed, never a
/// hang. Uses a fake/never-registered auth profile id against the same
/// acpx/claude runtime so the ONLY variable under test is auth resolution.
#[tokio::test]
#[ignore = "spawns acpx health probe; run manually with --ignored (no LLM tokens spent — \
            resolution fails before any turn starts)"]
async fn m4_07_unconfigured_auth_profile_fails_closed_before_session_starts() {
    let f = NamedTempFile::new().unwrap();
    let db_path = f.path().to_str().unwrap().to_owned();
    let mut agent = make_loop(&db_path).await;

    let acpx = AcpxAgentRuntime::new("claude").expect("acpx on PATH");
    let health = acpx.health().await.expect("health probe");
    assert!(health.ready, "acpx/claude not ready: {health:?}");

    let mut registry = RuntimeRegistry::new();
    registry.register(Arc::new(acpx));

    // Empty auth config: nothing verified, so ANY AuthProfileRef fails to
    // resolve — the fail-closed default this registry establishes.
    let auth_registry = bastion::auth_profile_registry::AuthProfileRegistry::build(
        &bastion::config::AuthConfig::default(),
    )
    .await;

    agent = agent
        .with_backend_profile(BackendProfile {
            conversation: ConversationBackend::Runtime("acpx_claude".to_string()),
            auth: Some(bastion_agent_runtime::AuthProfileRef(
                "never-configured-profile".to_string(),
            )),
            ..Default::default()
        })
        .with_runtime_registry(registry)
        .with_auth_resolver(Arc::new(auth_registry));

    let err = agent
        .run_turn_for("this must never reach the model", "m4-07-fail-closed-owner")
        .await
        .expect_err("an unresolvable AuthProfileRef must fail the turn, never proceed");
    let msg = err.to_string();
    assert!(
        msg.to_ascii_lowercase().contains("auth") || msg.contains("never-configured-profile"),
        "error should be attributable to auth resolution, got: {msg}"
    );
}
