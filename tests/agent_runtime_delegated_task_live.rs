//! Ciclo 2.4 — A-07 live proof
//! (`docs/revamp/C2-backend-profile-design.md` §4): a delegated coding task
//! against a real, host-authenticated `codex app-server`, with a concurrent
//! conversation turn, a second task cancelled mid-flight, and a genuine
//! restart-recovery resume — all through the REAL `AgentLoop` surface
//! (`delegate_task`/`cancel_delegated_task`/`resume_delegated_task`), not a
//! bypass. Placar em `docs/revamp/A-06-A-07-live.md`.
//!
//! Not run by default (`cargo test`): spawns real `codex app-server`
//! subprocesses, costs real tokens, and depends on host state (`codex`
//! installed and logged in). Run manually:
//!
//! ```text
//! cargo test --test agent_runtime_delegated_task_live -- --ignored --nocapture
//! ```
//!
//! Fixture note: duplicates the small `make_loop`/`MockProvider` shape from
//! `tests/agent_loop_public.rs`/`tests/agent_runtime_backend_live.rs` — each
//! file under `tests/` compiles as its own separate crate, so these cannot
//! share code across files.

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
            weight: 0.8,
            skills: vec![],
        },
    );
    PersonaRegistry::new_from_map(personas)
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
        Arc::new(RwLock::new(Box::new(MockProvider) as Box<dyn Provider>)),
        session,
        Arc::new(bastion_mcp::McpToolSource::new(mcp)),
        session_id,
        10.0,
        Arc::new(PersonaResponder::new(make_registry())),
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

/// Drains `pending_rx` until a message containing `needle` shows up, or the
/// overall deadline elapses (fails the test — a delegated task's completion
/// notification is the acceptance bar here, not merely "some message
/// arrived").
async fn recv_until_contains(
    rx: &mut tokio::sync::mpsc::Receiver<bastion_runtime::agent::loop_::PendingItem>,
    needle: &str,
    deadline: Duration,
) -> String {
    let start = tokio::time::Instant::now();
    loop {
        let remaining = deadline.saturating_sub(start.elapsed());
        if remaining.is_zero() {
            panic!("timed out waiting for a pending_tx message containing {needle:?}");
        }
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Some(item)) if item.text.contains(needle) => return item.text,
            Ok(Some(_other)) => continue, // some other task's notification — keep draining
            Ok(None) => panic!("pending_tx channel closed before {needle:?} arrived"),
            Err(_) => panic!("timed out waiting for a pending_tx message containing {needle:?}"),
        }
    }
}

#[tokio::test]
#[ignore = "spawns real codex app-server subprocesses, costs tokens; run manually with --ignored"]
async fn a07_delegated_task_concurrent_cancel_and_resume_live() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new("debug"))
        .with_test_writer()
        .try_init();

    let f = NamedTempFile::new().unwrap();
    let db_path = f.path().to_str().unwrap().to_owned();
    let mut agent = make_loop(&db_path).await;

    let codex = CodexAppServerRuntime::new().expect("codex on PATH");
    let health = codex.health().await.expect("health probe");
    eprintln!("health: {health:?}");
    assert!(health.ready, "codex not ready: {health:?}");

    let mut registry = RuntimeRegistry::new();
    registry.register(Arc::new(codex));

    agent = agent
        // Conversation stays Model (default) — only task_runtime is set.
        // Proves delegation is independent of the conversation backend.
        .with_backend_profile(BackendProfile {
            task_runtime: Some("codex_app_server".to_string()),
            ..Default::default()
        })
        .with_runtime_registry(registry);

    let owner = "a07-live-owner";
    let mut pending_rx = agent.pending_rx.take().expect("pending_rx must be present");

    // ---- 1. Delegate a short task; must return near-instantly (it only
    // starts the harness session + submits, it does not wait for the task). ----
    let t0 = tokio::time::Instant::now();
    let task1_key = agent
        .delegate_task(
            owner,
            "Reply with exactly this and nothing else: BASTION-A07-TASK1-OK".to_string(),
        )
        .await
        .expect("delegate_task 1 must succeed");
    let delegate_elapsed = t0.elapsed();
    eprintln!("task1_key={task1_key} (delegate_task returned in {delegate_elapsed:?})");
    assert!(
        delegate_elapsed < Duration::from_secs(15),
        "delegate_task must return promptly (start+submit only), took {delegate_elapsed:?}"
    );

    // ---- 2. Conversation stays responsive: a normal Model-backend turn on
    // the SAME agent completes immediately while task1 runs in the background. ----
    let t1 = tokio::time::Instant::now();
    let convo_response = agent
        .run_turn_for("hello while a task runs in the background", owner)
        .await
        .expect("conversation turn must succeed while a task is delegated");
    let convo_elapsed = t1.elapsed();
    eprintln!("conversation response: {convo_response:?} (in {convo_elapsed:?})");
    assert_eq!(convo_response, "mock conversation response");
    assert!(
        convo_elapsed < Duration::from_secs(5),
        "conversation must stay responsive (not blocked on the delegated task), took {convo_elapsed:?}"
    );

    // ---- 3. Delegate a second, deliberately slow task, then cancel it. ----
    let task2_key = agent
        .delegate_task(
            owner,
            "Run the shell command `sleep 15 && echo done` and report the output. \
             Do not summarize or skip it — actually run it and wait."
                .to_string(),
        )
        .await
        .expect("delegate_task 2 must succeed");
    eprintln!("task2_key={task2_key}");

    // Give the harness a moment to actually start the turn before cancelling.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let cancelled = agent
        .cancel_delegated_task(&task2_key)
        .await
        .expect("cancel_delegated_task must not error");
    assert!(
        cancelled,
        "task2 must still be live when cancel is requested"
    );

    // ---- 4. Both tasks report back via the PROACT-05 pending_tx seam. ----
    let task1_msg = recv_until_contains(&mut pending_rx, &task1_key, Duration::from_secs(60)).await;
    eprintln!("task1 notification: {task1_msg}");
    assert!(task1_msg.contains("concluída"), "task1: {task1_msg}");
    assert!(
        task1_msg.contains("BASTION-A07-TASK1-OK"),
        "task1: {task1_msg}"
    );

    let task2_msg = recv_until_contains(&mut pending_rx, &task2_key, Duration::from_secs(90)).await;
    eprintln!("task2 notification: {task2_msg}");
    assert!(
        task2_msg.contains("cancelada"),
        "task2 must report as cancelled: {task2_msg}"
    );

    // ---- 5. Restart recovery: start a THIRD session directly (bypassing
    // delegate_task, mirroring codex_v2_resume_smoke's warm-up pattern —
    // codex only persists a resumable rollout once a turn has actually run),
    // persist its handle, kill the process (drop), then reattach via
    // AgentLoop::resume_delegated_task and prove the follow-up task completes. ----
    let task3_key = format!("task:{owner}:a07-resume-probe");
    {
        let runtime = agent
            .runtime_registry
            .resolve("codex_app_server")
            .await
            .expect("codex must still be resolvable");
        let spec = bastion_agent_runtime::SessionSpec {
            owner: owner.to_string(),
            workspace: bastion_agent_runtime::WorkspacePolicy {
                root: std::env::temp_dir().join("bastion-a07-resume-probe"),
                read_only: false,
                deny: Vec::new(),
            },
            sandbox: bastion_agent_runtime::SandboxProfile::Trusted,
            permissions: bastion_agent_runtime::PermissionProfile {
                allow: vec!["*".to_string()],
            },
            auth: bastion_agent_runtime::AuthProfileRef("host-chatgpt-login".to_string()),
            runtime_id: "codex".to_string(),
            timeout: bastion_agent_runtime::TimeoutPolicy {
                per_task: Duration::from_secs(60),
                idle: Duration::from_secs(120),
            },
            env: bastion_agent_runtime::EnvPolicy {
                allow: {
                    let mut m = std::collections::BTreeMap::new();
                    for var in ["HOME", "PATH"] {
                        if let Ok(v) = std::env::var(var) {
                            m.insert(var.to_string(), v);
                        }
                    }
                    m
                },
            },
            mcp_bridge: None,
            otel: bastion_agent_runtime::OtelContext::default(),
        };
        tokio::fs::create_dir_all(&spec.workspace.root)
            .await
            .expect("create workspace");
        let mut session = runtime.start(spec).await.expect("start warm-up session");
        agent
            .session
            .save_runtime_handle(&task3_key, &session.handle())
            .await
            .expect("persist warm-up handle");
        match tokio::time::timeout(Duration::from_secs(30), session.next_event()).await {
            Ok(Some(bastion_agent_runtime::RuntimeEvent::Started { .. })) => {}
            other => panic!("expected Started first, got {other:?}"),
        }
        let warm_up = session
            .submit(bastion_agent_runtime::TaskInput {
                prompt: "Reply with exactly: ok".to_string(),
                attachments: Vec::new(),
                expected: bastion_agent_runtime::TaskExpectation::Conversation,
            })
            .await
            .expect("warm-up submit before resume");
        loop {
            match tokio::time::timeout(Duration::from_secs(30), session.next_event())
                .await
                .expect("event before watchdog")
                .expect("event stream open")
            {
                bastion_agent_runtime::RuntimeEvent::Ended { task, .. } if task == warm_up => break,
                _ => {}
            }
        }
        drop(session); // kill_on_drop tears down the app-server process — simulates a restart.
    }

    agent
        .resume_delegated_task(
            &task3_key,
            owner,
            "Reply with exactly this and nothing else: BASTION-A07-RESUME-OK".to_string(),
        )
        .await
        .expect("resume_delegated_task must succeed after simulated restart");

    let task3_msg = recv_until_contains(&mut pending_rx, &task3_key, Duration::from_secs(60)).await;
    eprintln!("task3 (resumed) notification: {task3_msg}");
    assert!(task3_msg.contains("concluída"), "task3: {task3_msg}");
    assert!(
        task3_msg.contains("BASTION-A07-RESUME-OK"),
        "task3: {task3_msg}"
    );
}
