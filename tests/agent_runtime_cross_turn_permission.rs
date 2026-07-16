//! Loop 3-A — 6a live proof (`docs/revamp/C3-runtime-followups-design.md`
//! §6a): cross-turn permission resolution against an in-process, pause-
//! capable `FakeRuntime` — no real subprocess, no `--ignored` gate. Proves:
//!
//! 1. A delegated task's `PermissionRequest` is genuinely PAUSED (not
//!    auto-denied) — persisted (owner-scoped) in the real `SqlitePermissionGate`
//!    — while the daemon keeps serving a completely different turn on the
//!    SAME `AgentLoop` (never blocking `&mut agent`).
//! 2. A LATER call to `AgentLoop::respond_permission` (simulating a later
//!    turn/command) resumes the task with `Allow`.
//! 3. An explicit `Deny` resolves the task down the deny path.
//! 4. No resolution before `permission_timeout` elapses → fail-closed
//!    `Deny { scope: Turn }` automatically.
//!
//! Fixture note: duplicates the `make_loop` shape from
//! `tests/agent_runtime_delegated_task_live.rs` (each file under `tests/`
//! compiles as its own crate, so fixtures cannot be shared across files).

use async_trait::async_trait;
use bastion_agent_runtime::{
    AgentRuntime, CancelMode, PermissionAction, PermissionDecision, PermissionRequestId,
    ResumeSpec, RuntimeError, RuntimeEvent, RuntimeHealth, RuntimeSession, SessionHandle,
    SessionSpec, SessionStatus, TaskId, TaskInput, TaskOutcome,
};
use bastion_cognition::goal::{GoalEngine, ScoringConfig};
use bastion_memory::sqlite::SqliteMemory;
use bastion_memory::{PrivacyTier, SharedMemory};
use bastion_personas::persona::{Persona, PersonaRegistry, PersonaResponder};
use bastion_providers::Provider;
use bastion_runtime::agent::backend::{BackendProfile, RuntimeRegistry};
use bastion_runtime::agent::loop_::AgentLoop;
use bastion_runtime::agent::ports::PermissionGate;
use bastion_runtime::capability::approval::SqliteApprovalGate;
use bastion_runtime::capability::permission_queue::SqlitePermissionGate;
use bastion_runtime::session::SessionManager;
use bastion_types::{CallConfig, LlmResponse, Message, TokenUsage};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;
use tempfile::NamedTempFile;
use tokio::sync::{Mutex, RwLock};

// ---------------------------------------------------------------------
// FakePausableRuntime / FakeSession — pause-capable in-process reference.
//
// Unlike `tests/agent_runtime_conformance.rs`'s FakeRuntime (which
// auto-answers its own scripted "emit:permission" prompt inline), THIS fake
// genuinely stalls: `next_event()` emits one `MessageDelta` then a
// `PermissionRequest`, and does NOT produce anything further until
// `respond_permission` is actually called — mirroring a real harness that
// is truly waiting on Bastion's answer.
// ---------------------------------------------------------------------

struct FakePausableRuntime;

struct SessionInner {
    task_id: Option<TaskId>,
    next_perm_id: u64,
    // Once respond_permission is called, the resulting follow-up events are
    // queued here; next_event() drains this queue after the initial
    // MessageDelta+PermissionRequest pair.
    queued: VecDeque<RuntimeEvent>,
    decided: bool,
}

struct FakeSession {
    handle: SessionHandle,
    inner: Arc<Mutex<SessionInner>>,
}

#[async_trait]
impl AgentRuntime for FakePausableRuntime {
    fn descriptor(&self) -> bastion_agent_runtime::RuntimeDescriptor {
        bastion_agent_runtime::RuntimeDescriptor {
            id: "fake_pausable",
            adapter_version: "0.0.0".to_string(),
            target_version: "test".to_string(),
            transport: bastion_agent_runtime::Transport::Embedded,
            supports: bastion_agent_runtime::RuntimeSupports {
                resume: false,
                steer: false,
                usage_reporting: false,
                diff_events: false,
                permission_bridge: true,
                concurrent_sessions: true,
            },
            policy_coverage: bastion_agent_runtime::PolicyCoverage {
                tool_visibility: bastion_agent_runtime::ToolVisibility::Full,
                approvals: bastion_agent_runtime::ApprovalCoverage::Bridged,
                egress: bastion_agent_runtime::EgressCoverage::InputFiltered,
                budget: bastion_agent_runtime::BudgetCoverage::Estimated,
                sandbox: bastion_agent_runtime::SandboxCoverage::Honored,
            },
        }
    }

    async fn health(&self) -> Result<RuntimeHealth, RuntimeError> {
        Ok(RuntimeHealth {
            detected_version: "0.0.0".to_string(),
            ready: true,
            detail: None,
        })
    }

    async fn start(&self, spec: SessionSpec) -> Result<Box<dyn RuntimeSession>, RuntimeError> {
        Ok(Box::new(FakeSession {
            handle: SessionHandle {
                runtime_id: "fake_pausable".to_string(),
                owner: spec.owner,
                external_ref: format!("fake-pausable-{:x}", rand_suffix()),
            },
            inner: Arc::new(Mutex::new(SessionInner {
                task_id: None,
                next_perm_id: 0,
                queued: VecDeque::new(),
                decided: false,
            })),
        }))
    }

    async fn resume(
        &self,
        _handle: &SessionHandle,
        _spec: ResumeSpec,
    ) -> Result<Box<dyn RuntimeSession>, RuntimeError> {
        Err(RuntimeError::NotResumable(
            "fake_pausable: resume unimplemented".to_string(),
        ))
    }
}

fn rand_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

#[async_trait]
impl RuntimeSession for FakeSession {
    fn handle(&self) -> SessionHandle {
        self.handle.clone()
    }

    async fn submit(&mut self, _input: TaskInput) -> Result<TaskId, RuntimeError> {
        let mut inner = self.inner.lock().await;
        let task = TaskId(1);
        inner.task_id = Some(task);
        inner.queued.push_back(RuntimeEvent::MessageDelta {
            task,
            text: "before-permission ".to_string(),
        });
        let perm_id = PermissionRequestId(inner.next_perm_id);
        inner.next_perm_id += 1;
        inner.queued.push_back(RuntimeEvent::PermissionRequest {
            task,
            id: perm_id,
            action: PermissionAction::RunCommand,
            detail: "run: echo fake-tool".to_string(),
        });
        Ok(task)
    }

    async fn next_event(&mut self) -> Option<RuntimeEvent> {
        let mut inner = self.inner.lock().await;
        inner.queued.pop_front()
    }

    async fn steer(&mut self, _text: &str) -> Result<(), RuntimeError> {
        Err(RuntimeError::Protocol("steer not supported".to_string()))
    }

    async fn cancel(&mut self, _mode: CancelMode) -> Result<(), RuntimeError> {
        let mut inner = self.inner.lock().await;
        if let Some(task) = inner.task_id {
            inner.queued.push_back(RuntimeEvent::Ended {
                task,
                outcome: TaskOutcome::Cancelled,
            });
        }
        Ok(())
    }

    async fn respond_permission(
        &mut self,
        _id: PermissionRequestId,
        decision: PermissionDecision,
    ) -> Result<(), RuntimeError> {
        let mut inner = self.inner.lock().await;
        if inner.decided {
            return Ok(()); // idempotent — a fake shouldn't double-queue
        }
        inner.decided = true;
        let Some(task) = inner.task_id else {
            return Ok(());
        };
        match decision {
            PermissionDecision::Allow => {
                inner.queued.push_back(RuntimeEvent::ToolResult {
                    task,
                    name: "echo".to_string(),
                    output_digest: "sha256:fake".to_string(),
                    is_error: false,
                });
                inner.queued.push_back(RuntimeEvent::MessageDelta {
                    task,
                    text: "TOOL-ALLOWED".to_string(),
                });
                inner.queued.push_back(RuntimeEvent::Ended {
                    task,
                    outcome: TaskOutcome::Success,
                });
            }
            PermissionDecision::Deny { .. } => {
                inner.queued.push_back(RuntimeEvent::MessageDelta {
                    task,
                    text: "TOOL-DENIED".to_string(),
                });
                inner.queued.push_back(RuntimeEvent::Ended {
                    task,
                    outcome: TaskOutcome::Success,
                });
            }
        }
        Ok(())
    }

    async fn status(&self) -> Result<SessionStatus, RuntimeError> {
        Ok(SessionStatus::Running)
    }
}

// ---------------------------------------------------------------------
// Fixture (duplicated from tests/agent_runtime_delegated_task_live.rs)
// ---------------------------------------------------------------------

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
        bastion_mcp::McpClient::connect_from_config(&std::collections::HashMap::new())
            .await
            .expect("empty MCP config"),
    );

    let agent = AgentLoop::new(
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
    );

    // 6a: wire the REAL SqlitePermissionGate (same db as everything else) —
    // without this, AgentLoop keeps NullPermissionGate and every permission
    // request would deny immediately (this test would never see a pending
    // row to resolve).
    agent.with_permission_gate(Arc::new(SqlitePermissionGate::new(db_path)))
}

fn fake_registry() -> RuntimeRegistry {
    let mut registry = RuntimeRegistry::new();
    registry.register(Arc::new(FakePausableRuntime));
    registry
}

async fn recv_until_contains(
    rx: &mut tokio::sync::mpsc::Receiver<bastion_runtime::agent::loop_::PendingItem>,
    needle: &str,
    deadline: Duration,
) -> bastion_runtime::agent::loop_::PendingItem {
    let start = tokio::time::Instant::now();
    loop {
        let remaining = deadline.saturating_sub(start.elapsed());
        if remaining.is_zero() {
            panic!("timed out waiting for a pending_tx message containing {needle:?}");
        }
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Some(item)) if item.text.contains(needle) => return item,
            Ok(Some(_other)) => continue,
            Ok(None) => panic!("pending_tx channel closed before {needle:?} arrived"),
            Err(_) => panic!("timed out waiting for a pending_tx message containing {needle:?}"),
        }
    }
}

/// Poll `permission_gate.pending_for_owner` until at least one row shows up
/// (or panic after `deadline`) — proves the request was genuinely
/// persisted/paused, not auto-denied.
async fn wait_for_one_pending(
    gate: &Arc<dyn PermissionGate>,
    owner: &str,
    deadline: Duration,
) -> bastion_runtime::agent::ports::PendingPermission {
    let start = tokio::time::Instant::now();
    loop {
        let pending = gate.pending_for_owner(owner).await.expect("pending query");
        if let Some(row) = pending.into_iter().next() {
            return row;
        }
        if start.elapsed() > deadline {
            panic!("timed out waiting for a pending permission row for owner {owner:?}");
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

// ---------------------------------------------------------------------
// 1. Non-blocking pause + later-turn Allow resumes the task.
// ---------------------------------------------------------------------

#[tokio::test]
async fn permission_request_pauses_then_later_turn_approves_and_resumes() {
    let f = NamedTempFile::new().unwrap();
    let db_path = f.path().to_str().unwrap().to_owned();
    let mut agent = make_loop(&db_path).await;

    agent = agent
        .with_backend_profile(BackendProfile {
            task_runtime: Some("fake_pausable".to_string()),
            ..Default::default()
        })
        .with_runtime_registry(fake_registry())
        // Long timeout — this test resolves explicitly, never via timeout.
        .with_permission_timeout(Duration::from_secs(30));

    let owner = "alice-6a";
    let permission_gate = agent.permission_gate.clone();
    let mut pending_rx = agent.pending_rx.take().expect("pending_rx present");

    let task_key = agent
        .delegate_task(owner, "please run the tool".to_string())
        .await
        .expect("delegate_task must succeed");

    // ---- Daemon keeps serving another turn while the task is paused. ----
    let convo = agent
        .run_turn_for("hello while a permission is pending", owner)
        .await
        .expect("conversation turn must succeed while a permission is pending");
    assert_eq!(convo, "mock conversation response");

    // ---- The request is genuinely persisted/paused (not auto-denied). ----
    let row = wait_for_one_pending(&permission_gate, owner, Duration::from_secs(5)).await;
    assert_eq!(row.owner, owner);
    assert!(matches!(row.action, PermissionAction::RunCommand));

    // ---- A LATER turn approves by id — task resumes. ----
    agent
        .respond_permission(owner, row.row_id, PermissionDecision::Allow)
        .await
        .expect("respond_permission must succeed");

    let result = recv_until_contains(&mut pending_rx, &task_key, Duration::from_secs(5)).await;
    assert!(result.text.contains("concluída"), "got: {}", result.text);
    assert!(
        result.text.contains("TOOL-ALLOWED"),
        "task must have resumed down the Allow path; got: {}",
        result.text
    );
    assert_eq!(result.owner.as_deref(), Some(owner));

    // The row must no longer be pending after resolution.
    assert!(permission_gate
        .pending_for_owner(owner)
        .await
        .unwrap()
        .is_empty());
}

// ---------------------------------------------------------------------
// 2. Explicit later-turn Deny.
// ---------------------------------------------------------------------

#[tokio::test]
async fn permission_request_explicit_deny_resolves_deny_path() {
    let f = NamedTempFile::new().unwrap();
    let db_path = f.path().to_str().unwrap().to_owned();
    let mut agent = make_loop(&db_path).await;

    agent = agent
        .with_backend_profile(BackendProfile {
            task_runtime: Some("fake_pausable".to_string()),
            ..Default::default()
        })
        .with_runtime_registry(fake_registry())
        .with_permission_timeout(Duration::from_secs(30));

    let owner = "bob-6a";
    let permission_gate = agent.permission_gate.clone();
    let mut pending_rx = agent.pending_rx.take().expect("pending_rx present");

    let task_key = agent
        .delegate_task(owner, "please run the tool".to_string())
        .await
        .expect("delegate_task must succeed");

    let row = wait_for_one_pending(&permission_gate, owner, Duration::from_secs(5)).await;

    agent
        .respond_permission(
            owner,
            row.row_id,
            PermissionDecision::Deny {
                scope: bastion_agent_runtime::DenyScope::Turn,
            },
        )
        .await
        .expect("respond_permission (deny) must succeed");

    let result = recv_until_contains(&mut pending_rx, &task_key, Duration::from_secs(5)).await;
    assert!(
        result.text.contains("TOOL-DENIED"),
        "task must reflect the explicit deny; got: {}",
        result.text
    );
}

// ---------------------------------------------------------------------
// 3. No resolution before permission_timeout → fail-closed Deny{Turn}.
// ---------------------------------------------------------------------

#[tokio::test]
async fn permission_request_timeout_denies_fail_closed() {
    let f = NamedTempFile::new().unwrap();
    let db_path = f.path().to_str().unwrap().to_owned();
    let mut agent = make_loop(&db_path).await;

    agent = agent
        .with_backend_profile(BackendProfile {
            task_runtime: Some("fake_pausable".to_string()),
            ..Default::default()
        })
        .with_runtime_registry(fake_registry())
        // Short timeout — this test deliberately never resolves; proves the
        // fail-closed automatic Deny{Turn} path fires on its own.
        .with_permission_timeout(Duration::from_millis(100));

    let owner = "carol-6a";
    let permission_gate = agent.permission_gate.clone();
    let mut pending_rx = agent.pending_rx.take().expect("pending_rx present");

    let task_key = agent
        .delegate_task(owner, "please run the tool".to_string())
        .await
        .expect("delegate_task must succeed");

    // Prove it's genuinely pending before the timeout fires.
    let _row = wait_for_one_pending(&permission_gate, owner, Duration::from_secs(2)).await;

    // Never call respond_permission — let the timeout fire.
    let result = recv_until_contains(&mut pending_rx, &task_key, Duration::from_secs(5)).await;
    assert!(
        result.text.contains("TOOL-DENIED"),
        "an unresolved request must fail-closed to Deny{{Turn}} on timeout; got: {}",
        result.text
    );

    // The persisted row must be resolved (not left dangling "pending").
    assert!(
        permission_gate
            .pending_for_owner(owner)
            .await
            .unwrap()
            .is_empty(),
        "timed-out request must be resolved in the persisted queue, not left pending"
    );
}
