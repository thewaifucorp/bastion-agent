//! Public-API `AgentLoop` tests moved VERBATIM from the kernel crate's
//! `agent/loop_.rs` unit-test module (M2 step 3b, decision A4): they exercise
//! only public surface (`run_turn`, `run_turn_for`, `run_turn_for_with_trust`,
//! `build_system_prompt_parts`, `capability_registry`), but their fixtures are
//! product types (PersonaResponder / SqliteMemory / GoalEngine / McpClient)
//! the kernel crate cannot depend on. Asserts are untouched; only the fixture
//! paths changed (`crate::` -> `bastion::`).

use async_trait::async_trait;
use bastion_cognition::goal::{GoalEngine, ScoringConfig};
use bastion_memory::sqlite::SqliteMemory;
use bastion_memory::{PrivacyTier, SharedMemory};
use bastion_personas::persona::{Persona, PersonaRegistry, PersonaResponder};
use bastion_providers::{Provider, SharedProvider};
use bastion_runtime::agent::loop_::{AgentLoop, DEFAULT_OWNER};
use bastion_runtime::capability::approval::SqliteApprovalGate;
use bastion_runtime::session::SessionManager;
use bastion_types::{CallConfig, ContentPart, LlmResponse, Message, MessageContent, TokenUsage};
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::NamedTempFile;
use tokio::sync::RwLock;

// MockProvider: complete_simple echoes a persona response.
struct MockProvider {
    persona_name: String,
}

#[async_trait]
impl Provider for MockProvider {
    async fn complete(&self, _: &[Message], _: &CallConfig) -> anyhow::Result<LlmResponse> {
        Ok(LlmResponse {
            text: format!("response from {}", self.persona_name),
            tool_calls: None,
            usage: bastion_types::TokenUsage {
                input_tokens: 10,
                output_tokens: 10,
                cache_read: 0,
                cache_write: 0,
                ..Default::default()
            },
        })
    }
    async fn complete_simple(&self, _prompt: &str) -> anyhow::Result<String> {
        Ok(format!("simple:{}", self.persona_name))
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

fn make_provider(name: &str) -> SharedProvider {
    Arc::new(RwLock::new(Box::new(MockProvider {
        persona_name: name.to_string(),
    }) as Box<dyn Provider>))
}

fn make_registry(name: &str) -> PersonaRegistry {
    let mut personas = HashMap::new();
    personas.insert(
        name.to_string(),
        Persona {
            name: name.to_string(),
            description: Some("Test persona".to_string()),
            system_prompt: format!("You are {name}."),
            tier: PrivacyTier::CloudOk,
            weight: 0.8,
            skills: vec![],
            ..Default::default()
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

    // connect_from_config with an empty map returns an empty client
    let mcp = Arc::new(
        bastion_mcp::McpClient::connect_from_config(&std::collections::HashMap::new())
            .await
            .expect("empty MCP config"),
    );

    AgentLoop::new(
        make_provider("TestPersona"),
        session,
        Arc::new(bastion_mcp::McpToolSource::new(mcp)),
        session_id,
        10.0,
        Arc::new(PersonaResponder::new(make_registry("TestPersona"))),
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

#[tokio::test]
async fn run_turn_benign_message_returns_persona_response() {
    let f = NamedTempFile::new().unwrap();
    let path = f.path().to_str().unwrap().to_owned();
    let mut agent = make_loop(&path).await;

    let resp = agent
        .run_turn("hello world")
        .await
        .expect("run_turn failed");
    assert!(
        !resp.is_empty(),
        "response must not be empty; got: {resp:?}"
    );
}

// --- Plan 11-04 (SEC-01): run_turn_for's approval-resolution intercept ----

/// Stub capability with a call counter — proves whether `invoke()` actually
/// dispatched. Mirrors `capability::registry::tests::ApprovalStubCap`
/// exactly (that one is private to its own module, so this test module gets
/// its own copy).
struct ApprovalResolutionStubCap {
    cap_name: String,
    schema: serde_json::Value,
    calls: Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait]
impl bastion_runtime::capability::Capability for ApprovalResolutionStubCap {
    fn name(&self) -> &str {
        &self.cap_name
    }
    fn description(&self) -> &str {
        "approval-resolution stub"
    }
    fn input_schema(&self) -> &serde_json::Value {
        &self.schema
    }
    async fn invoke(
        &self,
        _args: serde_json::Value,
        _ctx: &bastion_runtime::capability::InvokeCtx,
    ) -> anyhow::Result<serde_json::Value> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(serde_json::json!({"dispatched": true}))
    }
    fn needs_approval(&self) -> bool {
        true
    }
}

fn cloudok_ctx(owner: &str) -> bastion_runtime::capability::InvokeCtx {
    // Clears Policy 1 (egress) so enqueue_or_reuse's own invoke() call
    // reaches Policy 2 (the approval gate) — same convention as
    // capability::registry::tests::ctx_for.
    bastion_runtime::capability::InvokeCtx {
        owner: owner.to_string(),
        privacy_tier: Some(PrivacyTier::CloudOk),
        allowed_tools: None,
    }
}

/// Queues one `needs_approval()==true` capability call for `owner` via the
/// real `invoke()` path (not a direct `ApprovalQueue` call) — proves the
/// row this test resolves is the SAME kind of row Policy 2 actually creates.
async fn queue_one_pending(
    agent: &mut AgentLoop,
    cap_name: &str,
    owner: &str,
) -> Arc<std::sync::atomic::AtomicUsize> {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    agent
        .capability_registry
        .register(Arc::new(ApprovalResolutionStubCap {
            cap_name: cap_name.to_string(),
            schema: serde_json::json!({}),
            calls: calls.clone(),
        }))
        .expect("register stub capability");
    agent
        .capability_registry
        .invoke(cap_name, serde_json::json!({"x": 1}), &cloudok_ctx(owner))
        .await
        .expect("first invoke must queue, not error");
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "queuing must not dispatch"
    );
    calls
}

/// Test 2: one pending row, owner sends "sim" -> approve + real dispatch +
/// a confirmation string; the LLM is never invoked for this turn (proven by
/// the response NOT being the mock provider's "response from ..." text).
#[tokio::test]
async fn approval_resolution_approves_and_dispatches_on_sim() {
    let f = NamedTempFile::new().unwrap();
    let path = f.path().to_str().unwrap().to_owned();
    let mut agent = make_loop(&path).await;
    let calls = queue_one_pending(&mut agent, "dangerous_action", "alice").await;

    let resp = agent
        .run_turn_for("sim, pode confirmar", "alice")
        .await
        .expect("run_turn_for must succeed");

    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the originally-queued capability must dispatch exactly once"
    );
    assert!(
        resp.contains("dangerous_action"),
        "confirmation must name the executed capability; got: {resp:?}"
    );
    assert!(
        !resp.contains("response from"),
        "the LLM mock must never be invoked for an approval-resolution turn; got: {resp:?}"
    );

    let queue = agent.capability_registry.approval_gate();
    assert!(
        queue
            .pending_for_owner("alice")
            .await
            .expect("pending_for_owner")
            .is_empty(),
        "row must no longer be pending after resolution"
    );
}

/// Regression (milestone-close security review, 2026-07-13): an
/// `untrusted` turn (email `From:` header, Discord/Slack public channel —
/// none cryptographically authenticated) must NEVER resolve a pending
/// approval-queue row, even with a matching "sim"/"yes" phrase. Proves
/// `run_turn_for_with_trust(..., true)` skips `approval_resolution`
/// entirely: the capability never dispatches and the row stays pending.
#[tokio::test]
async fn approval_resolution_skipped_when_turn_is_untrusted() {
    let f = NamedTempFile::new().unwrap();
    let path = f.path().to_str().unwrap().to_owned();
    let mut agent = make_loop(&path).await;
    let calls = queue_one_pending(&mut agent, "dangerous_action", "alice").await;

    let resp = agent
        .run_turn_for_with_trust("sim, pode confirmar", "alice", true)
        .await
        .expect("run_turn_for_with_trust must succeed");

    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "an untrusted 'sim' must never dispatch the pending capability"
    );
    assert!(
        !resp.contains("dangerous_action"),
        "an untrusted turn must never produce an approval confirmation; got: {resp:?}"
    );

    let queue = agent.capability_registry.approval_gate();
    assert_eq!(
        queue
            .pending_for_owner("alice")
            .await
            .expect("pending_for_owner")
            .len(),
        1,
        "row must remain pending — untrusted input must never resolve it"
    );
}

/// Test 3: one pending row, owner sends "não" -> reject; the capability
/// never dispatches.
#[tokio::test]
async fn approval_resolution_rejects_and_never_dispatches_on_nao() {
    let f = NamedTempFile::new().unwrap();
    let path = f.path().to_str().unwrap().to_owned();
    let mut agent = make_loop(&path).await;
    let calls = queue_one_pending(&mut agent, "dangerous_action", "alice").await;

    let resp = agent
        .run_turn_for("não, cancela isso", "alice")
        .await
        .expect("run_turn_for must succeed");

    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "a rejected action must never dispatch"
    );
    assert!(
        resp.contains("cancel") || resp.to_lowercase().contains("cancel"),
        "got: {resp:?}"
    );

    let queue = agent.capability_registry.approval_gate();
    let pending = queue
        .pending_for_owner("alice")
        .await
        .expect("pending_for_owner");
    assert!(
        pending.is_empty(),
        "rejected row must no longer be 'pending' status"
    );
}

/// Test 4: one pending row, owner sends an UNRELATED message -> None,
/// falling through to a completely normal turn; the pending row is
/// untouched (still pending, capability never dispatches).
#[tokio::test]
async fn approval_resolution_falls_through_on_unrelated_message() {
    let f = NamedTempFile::new().unwrap();
    let path = f.path().to_str().unwrap().to_owned();
    let mut agent = make_loop(&path).await;
    let calls = queue_one_pending(&mut agent, "dangerous_action", "alice").await;

    let resp = agent
        .run_turn_for("what's the weather like today?", "alice")
        .await
        .expect("run_turn_for must succeed");

    assert!(
        resp.contains("response from"),
        "an unrelated message must fall through to a normal LLM turn; got: {resp:?}"
    );
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "an unrelated message must never dispatch the pending capability"
    );

    let queue = agent.capability_registry.approval_gate();
    let pending = queue
        .pending_for_owner("alice")
        .await
        .expect("pending_for_owner");
    assert_eq!(pending.len(), 1, "the pending row must remain untouched");
}

/// Test 5: multiple pending rows for the same owner — a plain "sim" with no
/// id resolves the OLDEST pending row only (deterministic tie-break).
#[tokio::test]
async fn approval_resolution_resolves_oldest_pending_row_only() {
    let f = NamedTempFile::new().unwrap();
    let path = f.path().to_str().unwrap().to_owned();
    let mut agent = make_loop(&path).await;

    let calls_a = queue_one_pending(&mut agent, "action_a", "alice").await;
    // Nanosecond-resolution created_at should already differ, but a tiny
    // sleep makes the ordering assertion deterministic regardless of clock
    // granularity on any given CI runner.
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let calls_b = queue_one_pending(&mut agent, "action_b", "alice").await;

    let resp = agent
        .run_turn_for("sim", "alice")
        .await
        .expect("run_turn_for must succeed");

    assert!(
        resp.contains("action_a"),
        "must resolve the OLDEST (first-queued) row; got: {resp:?}"
    );
    assert_eq!(
        calls_a.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "action_a (oldest) must dispatch"
    );
    assert_eq!(
        calls_b.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "action_b (newer) must remain untouched"
    );

    let queue = agent.capability_registry.approval_gate();
    let pending = queue
        .pending_for_owner("alice")
        .await
        .expect("pending_for_owner");
    assert_eq!(pending.len(), 1, "only action_b should remain pending");
    assert_eq!(pending[0].capability_name, "action_b");
}

// --- Ciclo 2.1 (docs/revamp/C2-approval-port-design.md §2/§3): typed
// ApprovalDenied + DenyScope::Turn ---------------------------------------

/// Acceptance criterion #2: a `DenyScope::Turn` denial (the product default)
/// must end the tool-loop for the CURRENT round — a second, unrelated,
/// available tool call in the SAME LLM response must NEVER dispatch. Proves
/// this at three levels: `cap_b` (the second tool) never runs, the provider
/// is never asked for a second round (would panic if it were), and the
/// returned text is `Ok` (a structured result, not a propagated `Err` —
/// §2's "NÃO como crash do turn").
#[tokio::test]
async fn turn_scoped_denial_skips_remaining_tool_calls_and_ends_turn() {
    use bastion_types::ToolCall;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CapA {
        calls: Arc<AtomicUsize>,
    }
    #[async_trait]
    impl bastion_runtime::capability::Capability for CapA {
        fn name(&self) -> &str {
            "cap_a"
        }
        fn description(&self) -> &str {
            "dangerous action a"
        }
        fn input_schema(&self) -> &serde_json::Value {
            static SCHEMA: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
            SCHEMA.get_or_init(|| serde_json::json!({}))
        }
        fn needs_approval(&self) -> bool {
            true
        }
        async fn invoke(
            &self,
            _args: serde_json::Value,
            _ctx: &bastion_runtime::capability::InvokeCtx,
        ) -> anyhow::Result<serde_json::Value> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(serde_json::json!({"ran": "a"}))
        }
    }

    struct CapB {
        calls: Arc<AtomicUsize>,
    }
    #[async_trait]
    impl bastion_runtime::capability::Capability for CapB {
        fn name(&self) -> &str {
            "cap_b"
        }
        fn description(&self) -> &str {
            "unrelated, always-allowed action b"
        }
        fn input_schema(&self) -> &serde_json::Value {
            static SCHEMA: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
            SCHEMA.get_or_init(|| serde_json::json!({}))
        }
        async fn invoke(
            &self,
            _args: serde_json::Value,
            _ctx: &bastion_runtime::capability::InvokeCtx,
        ) -> anyhow::Result<serde_json::Value> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(serde_json::json!({"ran": "b"}))
        }
    }

    /// `PersonaResponder::respond` calls `persona::router::route` BEFORE
    /// dispatching to a persona — it classifies the turn via up to 3
    /// `provider.complete()` attempts against the SAME provider instance,
    /// falling back to safe single-persona routing when none parses as valid
    /// `RouterDecision` JSON (`router.rs`'s CF-2 fallback). This double serves
    /// both roles: calls 0..=2 return plain, deliberately non-JSON text (so
    /// routing always falls through to the safe single-persona fallback,
    /// deterministically), call 3 is the REAL turn's first response
    /// (tool_calls=[cap_a, cap_b]). Any call beyond that panics — proving the
    /// tool-loop genuinely ended after round 0 rather than merely skipping
    /// cap_b for some unrelated reason.
    struct TwoToolsThenNeverAgain {
        calls: AtomicUsize,
    }
    #[async_trait]
    impl Provider for TwoToolsThenNeverAgain {
        async fn complete(&self, _: &[Message], _: &CallConfig) -> anyhow::Result<LlmResponse> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            match n {
                0..=2 => Ok(LlmResponse {
                    text: "not valid router json".to_owned(),
                    tool_calls: None,
                    usage: TokenUsage::default(),
                }),
                3 => Ok(LlmResponse {
                    text: "calling two tools".to_owned(),
                    tool_calls: Some(vec![
                        ToolCall {
                            id: "1".to_owned(),
                            name: "cap_a".to_owned(),
                            arguments: serde_json::json!({}),
                            extra: None,
                        },
                        ToolCall {
                            id: "2".to_owned(),
                            name: "cap_b".to_owned(),
                            arguments: serde_json::json!({}),
                            extra: None,
                        },
                    ]),
                    usage: TokenUsage::default(),
                }),
                _ => panic!(
                    "a Turn-scoped denial must end the tool-loop — the provider must never \
                     be asked for a second round of the actual turn"
                ),
            }
        }
        async fn complete_simple(&self, _: &str) -> anyhow::Result<String> {
            Ok("s".to_owned())
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

    let f = NamedTempFile::new().unwrap();
    let path = f.path().to_str().unwrap().to_owned();
    let mut agent = make_loop(&path).await;

    let calls_a = Arc::new(AtomicUsize::new(0));
    let calls_b = Arc::new(AtomicUsize::new(0));
    agent
        .capability_registry
        .register(Arc::new(CapA {
            calls: calls_a.clone(),
        }))
        .expect("register cap_a");
    agent
        .capability_registry
        .register(Arc::new(CapB {
            calls: calls_b.clone(),
        }))
        .expect("register cap_b");

    // Pre-reject cap_a for owner "alice" with the EXACT args the turn below
    // will invoke it with (`{}`) — same idempotency hash, so the turn's
    // invoke() call hits the already-Rejected row instead of freshly queuing.
    let gate = agent.capability_registry.approval_gate().clone();
    let outcome = gate
        .enqueue_or_reuse("alice", "cap_a", &serde_json::json!({}))
        .await
        .expect("enqueue cap_a");
    let id = match outcome {
        bastion_runtime::capability::ApprovalOutcome::NewlyQueued(id) => id,
        other => panic!("expected NewlyQueued, got {other:?}"),
    };
    gate.reject("alice", id).await.expect("reject cap_a");

    agent.provider = Arc::new(RwLock::new(Box::new(TwoToolsThenNeverAgain {
        calls: AtomicUsize::new(0),
    }) as Box<dyn Provider>));

    let resp = agent
        .run_turn_for("do stuff", "alice")
        .await
        .expect("a Turn-scoped denial must be a structured Ok result, never a propagated Err");

    assert_eq!(
        calls_a.load(Ordering::SeqCst),
        0,
        "the denied capability must never dispatch"
    );
    assert_eq!(
        calls_b.load(Ordering::SeqCst),
        0,
        "the SECOND tool must never execute once a Turn-scoped denial fires this round"
    );
    assert!(
        resp.contains("calling two tools"),
        "the turn's answer must be the text already produced this round, plus a warning; got: {resp:?}"
    );
}

#[tokio::test]
async fn run_turn_contestation_phrase_revokes_belief() {
    let f = NamedTempFile::new().unwrap();
    let path = f.path().to_str().unwrap().to_owned();
    let mut agent = make_loop(&path).await;

    // Pre-store a belief
    {
        let mem = agent.memory.read().await;
        mem.store_belief(
            DEFAULT_OWNER,
            None,
            "Mario exercises every morning",
            "sess1",
            "user",
            false,
            None,
        )
        .await
        .expect("store_belief");
    }

    // Verify belief is stored
    let before = {
        let mem = agent.memory.read().await;
        mem.retrieve_tagged(DEFAULT_OWNER, None)
            .await
            .expect("retrieve")
    };
    assert_eq!(before.len(), 1, "belief must exist before contestation");

    // Run a turn with a contestation phrase that overlaps with the belief
    let _ = agent
        .run_turn("isso não é mais verdade sobre exercises morning")
        .await;

    // After the turn, the output-validator should have revoked the belief
    let after = {
        let mem = agent.memory.read().await;
        mem.retrieve_tagged(DEFAULT_OWNER, None)
            .await
            .expect("retrieve")
    };
    assert!(
        after.is_empty(),
        "belief must be revoked after contestation turn"
    );
}

// Guards CR-01/CR-02 (privacy egress through the tool loop):
// 1. resolved_tier must come from the persona actually handling the turn
//    (the returned pid), not from self.forced_persona — which is already
//    consumed by .take() in run_turn_for, so re-reading it yielded None and
//    a LocalOnly persona was stamped CloudOk.
// 2. the new per-round check_egress in dispatch_tool_loop must NOT over-block
//    a legitimate CloudOk persona's multi-round tool loop.
#[tokio::test]
async fn cloud_ok_persona_tool_loop_passes_egress_gate() {
    use bastion_types::{TokenUsage, ToolCall};
    use std::sync::atomic::{AtomicUsize, Ordering};

    // Round 0 returns a tool_call (forces a second provider round through
    // dispatch_tool_loop, where the new egress gate lives); round 1 returns
    // final text to terminate the loop.
    struct ToolThenText {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl Provider for ToolThenText {
        async fn complete(&self, _: &[Message], _: &CallConfig) -> anyhow::Result<LlmResponse> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                Ok(LlmResponse {
                    text: String::new(),
                    tool_calls: Some(vec![ToolCall {
                        id: "t1".to_owned(),
                        name: "noop".to_owned(),
                        arguments: serde_json::json!({}),
                        extra: None,
                    }]),
                    usage: TokenUsage {
                        input_tokens: 1,
                        output_tokens: 1,
                        cache_read: 0,
                        cache_write: 0,
                        ..Default::default()
                    },
                })
            } else {
                Ok(LlmResponse {
                    text: "done".to_owned(),
                    tool_calls: None,
                    usage: TokenUsage {
                        input_tokens: 1,
                        output_tokens: 1,
                        cache_read: 0,
                        cache_write: 0,
                        ..Default::default()
                    },
                })
            }
        }
        async fn complete_simple(&self, _prompt: &str) -> anyhow::Result<String> {
            Ok("s".to_owned())
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

    let f = NamedTempFile::new().unwrap();
    let path = f.path().to_str().unwrap().to_owned();

    let session = bastion_runtime::session::SessionManager::new(&path);
    session.init_schema().await.expect("init_schema");
    let session_id = session.create_session().await.expect("create_session");
    let memory: SharedMemory = Arc::new(RwLock::new(
        Box::new(SqliteMemory::new(&path)) as Box<dyn bastion_memory::Memory>
    ));
    let mcp = Arc::new(
        bastion_mcp::McpClient::connect_from_config(&std::collections::HashMap::new())
            .await
            .expect("empty MCP config"),
    );

    let mut personas = HashMap::new();
    personas.insert(
        "Cloudy".to_string(),
        Persona {
            name: "Cloudy".to_string(),
            description: Some("Cloud-ok persona".to_string()),
            system_prompt: "You are Cloudy.".to_string(),
            tier: PrivacyTier::CloudOk,
            weight: 0.9,
            skills: vec![],
            ..Default::default()
        },
    );
    let registry = PersonaRegistry::new_from_map(personas);

    let provider: SharedProvider = Arc::new(RwLock::new(Box::new(ToolThenText {
        calls: AtomicUsize::new(0),
    }) as Box<dyn Provider>));

    let mut agent = AgentLoop::new(
        provider,
        session,
        Arc::new(bastion_mcp::McpToolSource::new(mcp)),
        session_id,
        10.0,
        Arc::new(PersonaResponder::new(registry)),
        memory.clone(),
        Some(Arc::new(GoalEngine::new(&path, ScoringConfig::default()))),
        vec![],
        Arc::new(SqliteApprovalGate::new(path.clone())),
        Arc::new(bastion_cognition::eval::failure_sink::EvalFailureSink),
        bastion::agent::default_context_providers(&memory),
        Arc::new(bastion_providers::registry::RegistryProviderResolver),
        Some(Arc::new(bastion_cognition::agent::dream::DreamFlush::new(
            memory.clone(),
        ))),
        Some(Arc::new(bastion::agent::skills::SkillReloadObserver)),
    );

    // CloudOk persona + cloud provider: the multi-round tool loop must complete,
    // proving the per-round egress gate resolves Some(CloudOk) and lets it through.
    let resp = agent
        .run_turn("do a thing")
        .await
        .expect("CloudOk persona tool loop must not be egress-blocked");
    assert_eq!(
        resp, "done",
        "tool loop must run a second round and return final text"
    );
}

// --- Plan 11-07 (SEC-04): dispatch_tool_loop spotlighting-aware framing ----

/// Configurable-locality stub capability — proves `dispatch_tool_loop`'s
/// LLM-facing tool-result content differs structurally between a trusted
/// (`is_local()==true`, default `is_trusted()==true`) and untrusted
/// (`is_local()==false`, default `is_trusted()==false`) capability.
struct SpotlightStubCap {
    name: String,
    schema: serde_json::Value,
    local: bool,
}

#[async_trait]
impl bastion_runtime::capability::Capability for SpotlightStubCap {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &str {
        "spotlight stub"
    }
    fn input_schema(&self) -> &serde_json::Value {
        &self.schema
    }
    fn is_local(&self) -> bool {
        self.local
    }
    async fn invoke(
        &self,
        _args: serde_json::Value,
        _ctx: &bastion_runtime::capability::InvokeCtx,
    ) -> anyhow::Result<serde_json::Value> {
        Ok(serde_json::json!({"payload": "hello"}))
    }
}

/// Round 0 forces a single named-tool call; round 1 returns final text to
/// terminate the loop. Mirrors `cloud_ok_persona_tool_loop_passes_egress_gate`'s
/// `ToolThenText`, parameterized by tool name so both tests below can target
/// their own registered `SpotlightStubCap`.
///
/// `run_turn_for` ALSO uses this same provider for `persona::router::route`'s
/// classification call (a `response_format`-bearing call, distinct from the
/// actual per-persona completion) — that call is answered deterministically
/// with a single-persona `RouterDecision` and does NOT consume/advance
/// `calls`, so the round-0/round-1 tool-loop logic below is never desynced
/// by the router's own provider call.
struct ToolThenTextNamed {
    calls: std::sync::atomic::AtomicUsize,
    tool_name: String,
    persona_name: String,
}

#[async_trait]
impl Provider for ToolThenTextNamed {
    async fn complete(&self, _: &[Message], config: &CallConfig) -> anyhow::Result<LlmResponse> {
        if config.response_format.is_some() {
            // The router's own classification call — answer deterministically,
            // never touching `calls`.
            return Ok(LlmResponse {
                text: serde_json::json!({
                    "personas": [self.persona_name],
                    "mode": "single",
                    "convene_reason": null,
                })
                .to_string(),
                tool_calls: None,
                usage: TokenUsage::default(),
            });
        }
        let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if n == 0 {
            Ok(LlmResponse {
                text: String::new(),
                tool_calls: Some(vec![bastion_types::ToolCall {
                    id: "t1".to_owned(),
                    name: self.tool_name.clone(),
                    arguments: serde_json::json!({}),
                    extra: None,
                }]),
                usage: TokenUsage {
                    input_tokens: 1,
                    output_tokens: 1,
                    cache_read: 0,
                    cache_write: 0,
                    ..Default::default()
                },
            })
        } else {
            Ok(LlmResponse {
                text: "done".to_owned(),
                tool_calls: None,
                usage: TokenUsage {
                    input_tokens: 1,
                    output_tokens: 1,
                    cache_read: 0,
                    cache_write: 0,
                    ..Default::default()
                },
            })
        }
    }
    async fn complete_simple(&self, _prompt: &str) -> anyhow::Result<String> {
        Ok("s".to_owned())
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

/// Extracts the FIRST `ContentPart::ToolResult.content` string found in the
/// session's persisted history (most recent turn's tool round).
fn find_tool_result_content(history: &[Message]) -> String {
    history
        .iter()
        .find_map(|m| match &m.content {
            MessageContent::Parts(parts) => parts.iter().find_map(|p| match p {
                ContentPart::ToolResult { content, .. } => Some(content.clone()),
                _ => None,
            }),
            _ => None,
        })
        .expect("history must contain a ToolResult message")
}

/// Behavior Test 1: a TRUSTED tool result (`is_local()==true`, default
/// `is_trusted()==true`) produces `ContentPart::ToolResult.content`
/// byte-identical to today's behavior (`tagged.data.to_string()`) — zero
/// observable change for the overwhelming majority of existing tool calls.
#[tokio::test]
async fn dispatch_tool_loop_trusted_result_content_is_unchanged() {
    let f = NamedTempFile::new().unwrap();
    let path = f.path().to_str().unwrap().to_owned();
    let mut agent = make_loop(&path).await;
    agent.provider = Arc::new(RwLock::new(Box::new(ToolThenTextNamed {
        calls: std::sync::atomic::AtomicUsize::new(0),
        tool_name: "trusted_cap".to_owned(),
        persona_name: "TestPersona".to_owned(),
    }) as Box<dyn Provider>));
    agent
        .capability_registry
        .register(Arc::new(SpotlightStubCap {
            name: "trusted_cap".to_owned(),
            schema: serde_json::json!({}),
            local: true,
        }))
        .expect("register trusted_cap");

    let session_id = agent.session_id.clone();
    let resp = agent
        .run_turn("do something")
        .await
        .expect("run_turn must complete");
    assert_eq!(resp, "done");

    let history = agent
        .session
        .load_recent(&session_id)
        .await
        .expect("load_recent");
    let content = find_tool_result_content(&history);
    assert_eq!(
        content,
        serde_json::json!({"payload": "hello"}).to_string(),
        "trusted result must render byte-identical to today's behavior (no envelope)"
    );
}

/// Behavior Test 2: an UNTRUSTED tool result (`is_local()==false`, default
/// `is_trusted()==false`) produces a `content` string that is a STRUCTURED
/// JSON envelope — not a bare ad-hoc text prefix glued onto the raw data.
#[tokio::test]
async fn dispatch_tool_loop_untrusted_result_content_is_structured_envelope() {
    let f = NamedTempFile::new().unwrap();
    let path = f.path().to_str().unwrap().to_owned();
    let mut agent = make_loop(&path).await;
    agent.provider = Arc::new(RwLock::new(Box::new(ToolThenTextNamed {
        calls: std::sync::atomic::AtomicUsize::new(0),
        tool_name: "untrusted_cap".to_owned(),
        persona_name: "TestPersona".to_owned(),
    }) as Box<dyn Provider>));
    agent
        .capability_registry
        .register(Arc::new(SpotlightStubCap {
            name: "untrusted_cap".to_owned(),
            schema: serde_json::json!({}),
            local: false,
        }))
        .expect("register untrusted_cap");

    let session_id = agent.session_id.clone();
    let resp = agent
        .run_turn("do something external")
        .await
        .expect("run_turn must complete");
    assert_eq!(resp, "done");

    let history = agent
        .session
        .load_recent(&session_id)
        .await
        .expect("load_recent");
    let content = find_tool_result_content(&history);
    let envelope: serde_json::Value =
        serde_json::from_str(&content).expect("untrusted content must be structured JSON");
    assert_eq!(envelope["trusted"], serde_json::json!(false));
    assert_eq!(envelope["source"], serde_json::json!("untrusted_cap"));
    assert_eq!(envelope["data"], serde_json::json!({"payload": "hello"}));
    assert!(
        envelope["note"]
            .as_str()
            .unwrap_or_default()
            .contains("data, not instructions"),
        "envelope must carry an explicit non-instruction marker, got: {envelope}"
    );
}

// --- Plan 11-08 (SEC-05): run_turn_for_with_trust + quarantine wiring -----

/// Router-aware mock (same `response_format.is_some()` trick as
/// `ToolThenTextNamed`) that additionally RECORDS every `config.tools`
/// seen by the real (non-router) `complete()` call, so tests can assert
/// on exactly what the LLM-facing dispatch saw.
struct ToolsRecordingProvider {
    persona_name: String,
    seen_tools: Arc<std::sync::Mutex<Vec<Vec<serde_json::Value>>>>,
}

#[async_trait]
impl Provider for ToolsRecordingProvider {
    async fn complete(&self, _: &[Message], config: &CallConfig) -> anyhow::Result<LlmResponse> {
        if config.response_format.is_some() {
            return Ok(LlmResponse {
                text: serde_json::json!({
                    "personas": [self.persona_name],
                    "mode": "single",
                    "convene_reason": null,
                })
                .to_string(),
                tool_calls: None,
                usage: TokenUsage::default(),
            });
        }
        self.seen_tools.lock().unwrap().push(config.tools.clone());
        Ok(LlmResponse {
            text: "done".to_owned(),
            tool_calls: None,
            usage: TokenUsage::default(),
        })
    }
    async fn complete_simple(&self, _prompt: &str) -> anyhow::Result<String> {
        Ok("s".to_owned())
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

/// Test 4: `run_turn_for_with_trust(input, owner, true)` — the LLM-facing
/// call for this turn sees ZERO tools, genuinely hidden (not merely "no
/// new tools added"), even though a real capability is registered.
#[tokio::test]
async fn run_turn_for_with_trust_true_hides_all_tools_from_llm_facing_dispatch() {
    let f = NamedTempFile::new().unwrap();
    let path = f.path().to_str().unwrap().to_owned();
    let mut agent = make_loop(&path).await;
    let seen_tools = Arc::new(std::sync::Mutex::new(Vec::new()));
    agent.provider = Arc::new(RwLock::new(Box::new(ToolsRecordingProvider {
        persona_name: "TestPersona".to_owned(),
        seen_tools: seen_tools.clone(),
    }) as Box<dyn Provider>));
    agent
        .capability_registry
        .register(Arc::new(SpotlightStubCap {
            name: "cap1".to_owned(),
            schema: serde_json::json!({}),
            local: true,
        }))
        .expect("register cap1");

    let resp = agent
        .run_turn_for_with_trust("email body content", "alice", true)
        .await
        .expect("run_turn_for_with_trust must complete");
    assert_eq!(resp, "done");

    {
        let recorded = seen_tools.lock().unwrap();
        assert_eq!(
            recorded.last().unwrap().len(),
            0,
            "untrusted==true must hide every capability from the LLM-facing tools list"
        );
    }

    assert_eq!(
        agent.capability_registry.list_tool_defs().len(),
        1,
        "capabilities must be fully restored after the turn completes"
    );
}

/// Counterpart: `untrusted == false` shows the registered capability
/// unchanged — zero regression for the overwhelming majority of turns.
#[tokio::test]
async fn run_turn_for_with_trust_false_shows_tools_unchanged() {
    let f = NamedTempFile::new().unwrap();
    let path = f.path().to_str().unwrap().to_owned();
    let mut agent = make_loop(&path).await;
    let seen_tools = Arc::new(std::sync::Mutex::new(Vec::new()));
    agent.provider = Arc::new(RwLock::new(Box::new(ToolsRecordingProvider {
        persona_name: "TestPersona".to_owned(),
        seen_tools: seen_tools.clone(),
    }) as Box<dyn Provider>));
    agent
        .capability_registry
        .register(Arc::new(SpotlightStubCap {
            name: "cap1".to_owned(),
            schema: serde_json::json!({}),
            local: true,
        }))
        .expect("register cap1");

    let resp = agent
        .run_turn_for_with_trust("normal message", "alice", false)
        .await
        .expect("run_turn_for_with_trust must complete");
    assert_eq!(resp, "done");

    let recorded = seen_tools.lock().unwrap();
    assert_eq!(
        recorded.last().unwrap().len(),
        1,
        "untrusted==false must show the registered capability unchanged"
    );
}

/// Test 3: `run_turn_for` (existing method) is byte-identical to today —
/// internally a thin wrapper over `run_turn_for_with_trust(..., false)`.
#[tokio::test]
async fn run_turn_for_is_a_thin_wrapper_over_with_trust_false() {
    let f = NamedTempFile::new().unwrap();
    let path = f.path().to_str().unwrap().to_owned();
    let mut agent = make_loop(&path).await;
    let seen_tools = Arc::new(std::sync::Mutex::new(Vec::new()));
    agent.provider = Arc::new(RwLock::new(Box::new(ToolsRecordingProvider {
        persona_name: "TestPersona".to_owned(),
        seen_tools: seen_tools.clone(),
    }) as Box<dyn Provider>));
    agent
        .capability_registry
        .register(Arc::new(SpotlightStubCap {
            name: "cap1".to_owned(),
            schema: serde_json::json!({}),
            local: true,
        }))
        .expect("register cap1");

    let resp = agent
        .run_turn_for("normal message", "alice")
        .await
        .expect("run_turn_for must complete");
    assert_eq!(resp, "done");

    let recorded = seen_tools.lock().unwrap();
    assert_eq!(
        recorded.last().unwrap().len(),
        1,
        "run_turn_for must behave exactly like run_turn_for_with_trust(..., false)"
    );
}

/// Round-aware mock provider (mirrors `ToolThenTextNamed`'s
/// response_format branch) that emits a tool call for the SAME name on
/// rounds 0 and 1, then a final "done" on round 2 — lets a test drive
/// TWO consecutive untrusted tool rounds.
struct RoundAwareProvider {
    calls: std::sync::atomic::AtomicUsize,
    tool_name: String,
    persona_name: String,
}

#[async_trait]
impl Provider for RoundAwareProvider {
    async fn complete(&self, _: &[Message], config: &CallConfig) -> anyhow::Result<LlmResponse> {
        if config.response_format.is_some() {
            return Ok(LlmResponse {
                text: serde_json::json!({
                    "personas": [self.persona_name],
                    "mode": "single",
                    "convene_reason": null,
                })
                .to_string(),
                tool_calls: None,
                usage: TokenUsage::default(),
            });
        }
        let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if n < 2 {
            Ok(LlmResponse {
                text: String::new(),
                tool_calls: Some(vec![bastion_types::ToolCall {
                    id: format!("t{n}"),
                    name: self.tool_name.clone(),
                    arguments: serde_json::json!({}),
                    extra: None,
                }]),
                usage: TokenUsage {
                    input_tokens: 1,
                    output_tokens: 1,
                    cache_read: 0,
                    cache_write: 0,
                    ..Default::default()
                },
            })
        } else {
            Ok(LlmResponse {
                text: "done".to_owned(),
                tool_calls: None,
                usage: TokenUsage {
                    input_tokens: 1,
                    output_tokens: 1,
                    cache_read: 0,
                    cache_write: 0,
                    ..Default::default()
                },
            })
        }
    }
    async fn complete_simple(&self, _prompt: &str) -> anyhow::Result<String> {
        Ok("s".to_owned())
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

/// Stub capability with `is_local()==false` (untrusted by default) that
/// counts invocations — proves the SAME capability remains dispatchable
/// in the round FOLLOWING an untrusted result (the round-level
/// quarantine wraps ONLY the immediately-following LLM completion call,
/// tightly scoped and already dropped/restored by the time the next
/// round's tool dispatch runs).
struct CountingUntrustedCap {
    name: String,
    schema: serde_json::Value,
    calls: Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait]
impl bastion_runtime::capability::Capability for CountingUntrustedCap {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &str {
        "counting untrusted stub"
    }
    fn input_schema(&self) -> &serde_json::Value {
        &self.schema
    }
    fn is_local(&self) -> bool {
        false
    }
    async fn invoke(
        &self,
        _args: serde_json::Value,
        _ctx: &bastion_runtime::capability::InvokeCtx,
    ) -> anyhow::Result<serde_json::Value> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(serde_json::json!({"x": 1}))
    }
}

/// Test 5: when the CURRENT round's tool results include at least one
/// `TaggedValue.trusted == false`, the LLM call for the NEXT round runs
/// with quarantine active (tightly scoped to that one call). Proven here
/// by driving TWO untrusted rounds back-to-back: the loop must complete
/// normally, the capability must be invoked exactly twice (once per
/// round — the round-level quarantine has already dropped/restored by
/// the time each round's OWN tool dispatch runs, so it never permanently
/// blocks legitimate re-dispatch in a later round), and the capability
/// must remain registered/usable after the whole turn ends.
#[tokio::test]
async fn dispatch_tool_loop_untrusted_round_result_does_not_break_subsequent_rounds() {
    let f = NamedTempFile::new().unwrap();
    let path = f.path().to_str().unwrap().to_owned();
    let mut agent = make_loop(&path).await;
    agent.provider = Arc::new(RwLock::new(Box::new(RoundAwareProvider {
        calls: std::sync::atomic::AtomicUsize::new(0),
        tool_name: "untrusted_round_cap".to_owned(),
        persona_name: "TestPersona".to_owned(),
    }) as Box<dyn Provider>));
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    agent
        .capability_registry
        .register(Arc::new(CountingUntrustedCap {
            name: "untrusted_round_cap".to_owned(),
            schema: serde_json::json!({}),
            calls: calls.clone(),
        }))
        .expect("register untrusted_round_cap");

    let resp = agent
        .run_turn("trigger two untrusted rounds")
        .await
        .expect("run_turn must complete despite mid-loop quarantine wrapping");
    assert_eq!(resp, "done");
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "the capability must still be dispatchable in the round FOLLOWING an untrusted \
         result — round-level quarantine is scoped tightly to the intervening LLM call only"
    );
    assert_eq!(
        agent.capability_registry.list_tool_defs().len(),
        1,
        "the capability must remain registered/usable after the whole turn completes"
    );
}

/// Provider that records `config.tools.len()` on every non-router
/// `complete()` call, in call order — used to prove the round-level SEC-05
/// quarantine actually hides tools from the LLM-facing request, not just
/// from `invoke()` (milestone-close code review, 2026-07-13 regression).
struct ToolVisibilityRecordingProvider {
    calls: std::sync::atomic::AtomicUsize,
    tool_name: String,
    seen_tool_counts: Arc<std::sync::Mutex<Vec<usize>>>,
}

#[async_trait]
impl Provider for ToolVisibilityRecordingProvider {
    async fn complete(&self, _: &[Message], config: &CallConfig) -> anyhow::Result<LlmResponse> {
        if config.response_format.is_some() {
            return Ok(LlmResponse {
                text: serde_json::json!({
                    "personas": ["TestPersona"],
                    "mode": "single",
                    "convene_reason": null,
                })
                .to_string(),
                tool_calls: None,
                usage: TokenUsage::default(),
            });
        }
        self.seen_tool_counts
            .lock()
            .expect("mutex not poisoned")
            .push(config.tools.len());
        let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if n == 0 {
            Ok(LlmResponse {
                text: String::new(),
                tool_calls: Some(vec![bastion_types::ToolCall {
                    id: "t0".to_owned(),
                    name: self.tool_name.clone(),
                    arguments: serde_json::json!({}),
                    extra: None,
                }]),
                usage: TokenUsage {
                    input_tokens: 1,
                    output_tokens: 1,
                    ..Default::default()
                },
            })
        } else {
            Ok(LlmResponse {
                text: "done".to_owned(),
                tool_calls: None,
                usage: TokenUsage {
                    input_tokens: 1,
                    output_tokens: 1,
                    ..Default::default()
                },
            })
        }
    }
    async fn complete_simple(&self, _prompt: &str) -> anyhow::Result<String> {
        Ok("s".to_owned())
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

/// Regression (milestone-close code review, 2026-07-13): the round-level
/// SEC-05 quarantine must rebuild `CallConfig.tools` from the drained
/// registry for the quarantined round's LLM request — not just block
/// `invoke()` — so the model genuinely sees zero tools, matching the
/// turn-level `untrusted` path's already-correct behavior.
#[tokio::test]
async fn dispatch_tool_loop_untrusted_round_hides_tools_from_the_llm_request() {
    let f = NamedTempFile::new().unwrap();
    let path = f.path().to_str().unwrap().to_owned();
    let mut agent = make_loop(&path).await;
    let seen_tool_counts = Arc::new(std::sync::Mutex::new(Vec::new()));
    agent.provider = Arc::new(RwLock::new(Box::new(ToolVisibilityRecordingProvider {
        calls: std::sync::atomic::AtomicUsize::new(0),
        tool_name: "untrusted_visibility_cap".to_owned(),
        seen_tool_counts: seen_tool_counts.clone(),
    }) as Box<dyn Provider>));
    agent
        .capability_registry
        .register(Arc::new(CountingUntrustedCap {
            name: "untrusted_visibility_cap".to_owned(),
            schema: serde_json::json!({}),
            calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }))
        .expect("register untrusted_visibility_cap");

    let resp = agent
        .run_turn("trigger one untrusted round")
        .await
        .expect("run_turn must complete");
    assert_eq!(resp, "done");

    let counts = seen_tool_counts.lock().expect("mutex not poisoned");
    assert_eq!(
        *counts,
        vec![1, 0],
        "round 0 (initial) must see the 1 registered tool; round 1 (quarantined \
         by round 0's untrusted result) must see ZERO tools in the LLM-facing \
         request, not just have invoke() blocked"
    );
}

// --- M4-07: /backends and /backend use cockpit commands -------------------

/// Minimal in-process fake [`bastion_agent_runtime::AgentRuntime`] — never
/// actually opens a session in these tests (they only exercise the
/// listing/selection UX, not a real turn through it). Mirrors the shape of
/// `bastion_runtime::agent::backend`'s own private test fixture (that one
/// isn't reusable across this separate test binary).
struct FakeBackendAdapter {
    id: &'static str,
    ready: bool,
}

#[async_trait::async_trait]
impl bastion_agent_runtime::AgentRuntime for FakeBackendAdapter {
    fn descriptor(&self) -> bastion_agent_runtime::RuntimeDescriptor {
        bastion_agent_runtime::RuntimeDescriptor {
            id: self.id,
            adapter_version: "0.0.0".to_string(),
            target_version: "test".to_string(),
            transport: bastion_agent_runtime::Transport::Embedded,
            supports: bastion_agent_runtime::RuntimeSupports::default(),
            policy_coverage: bastion_agent_runtime::PolicyCoverage {
                tool_visibility: bastion_agent_runtime::ToolVisibility::DeclaredOnly,
                approvals: bastion_agent_runtime::ApprovalCoverage::HarnessOwned,
                egress: bastion_agent_runtime::EgressCoverage::HarnessOwned,
                budget: bastion_agent_runtime::BudgetCoverage::Reported,
                sandbox: bastion_agent_runtime::SandboxCoverage::None,
            },
        }
    }

    async fn health(
        &self,
    ) -> Result<bastion_agent_runtime::RuntimeHealth, bastion_agent_runtime::RuntimeError> {
        Ok(bastion_agent_runtime::RuntimeHealth {
            detected_version: "0.0.0".to_string(),
            ready: self.ready,
            detail: if self.ready {
                None
            } else {
                Some("fake unhealthy for this test".to_string())
            },
        })
    }

    async fn start(
        &self,
        _spec: bastion_agent_runtime::SessionSpec,
    ) -> Result<Box<dyn bastion_agent_runtime::RuntimeSession>, bastion_agent_runtime::RuntimeError>
    {
        Err(bastion_agent_runtime::RuntimeError::Unavailable(
            "fake: start unimplemented (this test only exercises selection UX)".to_string(),
        ))
    }

    async fn resume(
        &self,
        _handle: &bastion_agent_runtime::SessionHandle,
        _spec: bastion_agent_runtime::ResumeSpec,
    ) -> Result<Box<dyn bastion_agent_runtime::RuntimeSession>, bastion_agent_runtime::RuntimeError>
    {
        Err(bastion_agent_runtime::RuntimeError::NotResumable(
            "fake: resume unimplemented".to_string(),
        ))
    }
}

/// `/backends` must list `model` (always available) plus every registered
/// runtime with its live health and policy-coverage summary.
#[tokio::test]
async fn cockpit_backends_lists_model_and_registered_runtimes() {
    let f = NamedTempFile::new().unwrap();
    let path = f.path().to_str().unwrap().to_owned();
    let mut agent = make_loop(&path).await;

    let mut registry = bastion_runtime::agent::backend::RuntimeRegistry::new();
    registry.register(Arc::new(FakeBackendAdapter {
        id: "fake_healthy",
        ready: true,
    }));
    registry.register(Arc::new(FakeBackendAdapter {
        id: "fake_unhealthy",
        ready: false,
    }));
    agent = agent.with_runtime_registry(registry);

    let resp = agent
        .run_turn_for("/backends", DEFAULT_OWNER)
        .await
        .expect("/backends must not error");

    assert!(resp.contains("model"), "must list model: {resp:?}");
    assert!(
        resp.contains("fake_healthy") && resp.contains("saudável agora"),
        "must list the healthy fake runtime as available: {resp:?}"
    );
    assert!(
        resp.contains("fake_unhealthy") && resp.contains("INDISPONÍVEL"),
        "must list the unhealthy fake runtime as unavailable, with a reason: {resp:?}"
    );
}

/// `/backend use <id>` switches the conversation backend live (no restart)
/// when the id resolves — and populates `coverage_note` from the adapter's
/// own descriptor, never inventing one.
#[tokio::test]
async fn cockpit_backend_use_switches_conversation_to_registered_runtime() {
    let f = NamedTempFile::new().unwrap();
    let path = f.path().to_str().unwrap().to_owned();
    let mut agent = make_loop(&path).await;

    let mut registry = bastion_runtime::agent::backend::RuntimeRegistry::new();
    registry.register(Arc::new(FakeBackendAdapter {
        id: "fake_healthy",
        ready: true,
    }));
    agent = agent.with_runtime_registry(registry);

    assert_eq!(
        agent.backend_profile.conversation,
        bastion_runtime::agent::backend::ConversationBackend::Model,
        "sanity: starts on Model"
    );

    let resp = agent
        .run_turn_for("/backend use fake_healthy", DEFAULT_OWNER)
        .await
        .expect("switching to a healthy, registered runtime must succeed");
    assert!(resp.contains("fake_healthy"), "confirmation: {resp:?}");
    assert_eq!(
        agent.backend_profile.conversation,
        bastion_runtime::agent::backend::ConversationBackend::Runtime("fake_healthy".to_string())
    );
    assert!(
        agent.backend_profile.coverage_note.is_some(),
        "coverage_note must be populated from the adapter's own descriptor"
    );

    // Switch back to model — the other direction of the same UX.
    let resp = agent
        .run_turn_for("/backend use model", DEFAULT_OWNER)
        .await
        .expect("switching back to model must succeed");
    assert!(resp.contains("model"), "confirmation: {resp:?}");
    assert_eq!(
        agent.backend_profile.conversation,
        bastion_runtime::agent::backend::ConversationBackend::Model
    );
    assert!(
        agent.backend_profile.coverage_note.is_none(),
        "coverage_note must be cleared when switching back to Model"
    );
}

/// `/backend use <id>` for an unregistered or unhealthy id fails closed —
/// typed error, and `backend_profile` is left COMPLETELY untouched (never a
/// half-applied switch).
#[tokio::test]
async fn cockpit_backend_use_unknown_id_fails_closed_and_leaves_profile_untouched() {
    let f = NamedTempFile::new().unwrap();
    let path = f.path().to_str().unwrap().to_owned();
    let mut agent = make_loop(&path).await;

    let mut registry = bastion_runtime::agent::backend::RuntimeRegistry::new();
    registry.register(Arc::new(FakeBackendAdapter {
        id: "fake_unhealthy",
        ready: false,
    }));
    agent = agent.with_runtime_registry(registry);

    // Case 1: id never registered at all.
    let err = agent
        .run_turn_for("/backend use does_not_exist", DEFAULT_OWNER)
        .await
        .expect_err("an unregistered id must fail the switch");
    assert!(err.to_string().contains("does_not_exist"));
    assert_eq!(
        agent.backend_profile.conversation,
        bastion_runtime::agent::backend::ConversationBackend::Model,
        "profile must stay untouched after a failed switch"
    );

    // Case 2: id registered but unhealthy.
    let err = agent
        .run_turn_for("/backend use fake_unhealthy", DEFAULT_OWNER)
        .await
        .expect_err("an unhealthy id must fail the switch");
    assert!(err.to_string().contains("fake_unhealthy"));
    assert_eq!(
        agent.backend_profile.conversation,
        bastion_runtime::agent::backend::ConversationBackend::Model,
        "profile must stay untouched after a second failed switch"
    );
}

/// `/backend use task:<id>` / `task:none` — independent of the conversation
/// backend, same fail-closed discipline.
#[tokio::test]
async fn cockpit_backend_use_task_prefix_sets_and_clears_task_runtime() {
    let f = NamedTempFile::new().unwrap();
    let path = f.path().to_str().unwrap().to_owned();
    let mut agent = make_loop(&path).await;

    let mut registry = bastion_runtime::agent::backend::RuntimeRegistry::new();
    registry.register(Arc::new(FakeBackendAdapter {
        id: "fake_healthy",
        ready: true,
    }));
    agent = agent.with_runtime_registry(registry);

    assert!(agent.backend_profile.task_runtime.is_none());

    agent
        .run_turn_for("/backend use task:fake_healthy", DEFAULT_OWNER)
        .await
        .expect("setting a healthy task_runtime must succeed");
    assert_eq!(
        agent.backend_profile.task_runtime.as_deref(),
        Some("fake_healthy")
    );

    // Unknown id: fails closed, task_runtime stays whatever it was.
    let err = agent
        .run_turn_for("/backend use task:nope", DEFAULT_OWNER)
        .await
        .expect_err("unknown task_runtime id must fail");
    assert!(err.to_string().contains("nope"));
    assert_eq!(
        agent.backend_profile.task_runtime.as_deref(),
        Some("fake_healthy"),
        "a failed task_runtime switch must not clear the previous valid value"
    );

    agent
        .run_turn_for("/backend use task:none", DEFAULT_OWNER)
        .await
        .expect("clearing task_runtime must succeed");
    assert!(agent.backend_profile.task_runtime.is_none());
}
