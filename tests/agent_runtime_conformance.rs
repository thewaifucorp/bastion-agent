//! Integration test for the A-02 `AgentRuntime` conformance suite
//! (`bastion_agent_runtime::conformance`).
//!
//! Ships an embedded, deterministic reference implementation of the A-01
//! contract (`FakeRuntime`/`FakeSession`, no subprocess/network) and asserts
//! it passes every check with zero failures and zero skips (it declares full
//! `RuntimeSupports` and implements `FaultInjection` completely).
//!
//! ## Fake mini-protocol
//!
//! `FakeSession` decides how to react to a submitted task purely from
//! `TaskInput.prompt` (substring match, checked in this order):
//!
//! | prompt contains   | behavior                                                          |
//! |--------------------|-------------------------------------------------------------------|
//! | `"hang"`            | emits one `MessageDelta`, then never reaches `Ended` on its own (relies on `cancel`/timeout/crash). |
//! | `"emit:permission"` | emits a `MessageDelta` then a `PermissionRequest`; on `Allow` emits a non-error `ToolResult` then `Ended{Success}`; on `Deny` emits `Ended{Success}` with no `ToolResult`. |
//! | `"emit:artifact"`   | writes `artifact.txt` under the session workspace, emits `MessageDelta` then `Artifact{digest}` then `Ended{Success}`. |
//! | anything else       | happy path: two `MessageDelta`s then `Ended{Success}`.            |
//!
//! `Started { handle }` is always the first event of a session, queued at
//! `AgentRuntime::start`. Timeout is evaluated lazily: `next_event` compares
//! the active task's age against `SessionSpec.timeout.per_task` and
//! synthesizes `Ended{TimedOut}` once it elapses.
//!
//! Session state (`SessionInner`) lives behind an `Arc<Mutex<_>>` also held
//! by the runtime's `Shared.active_session`, so it survives dropping the
//! `Box<dyn RuntimeSession>` — this is what makes `resume` actually reattach
//! instead of just returning `NotResumable`.

use async_trait::async_trait;
use bastion_agent_runtime::conformance::{self, ConformanceScenarios, FaultInjection};
use bastion_agent_runtime::*;
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

// ---------------------------------------------------------------------
// FakeRuntime / FakeSession — embedded reference implementation
// ---------------------------------------------------------------------

#[derive(Clone)]
struct TaskState {
    id: TaskId,
    started_at: Instant,
    ended: bool,
    pending_permission: Option<PermissionRequestId>,
}

struct SessionInner {
    status: SessionStatus,
    pending_events: VecDeque<RuntimeEvent>,
    next_task_id: u64,
    next_perm_id: u64,
    current: Option<TaskState>,
    per_task_timeout: Duration,
    workspace_root: PathBuf,
    garbage_pending: bool,
}

struct Shared {
    auth_fail_armed: bool,
    next_session_seq: u64,
    active_session: Option<(SessionHandle, Arc<Mutex<SessionInner>>)>,
}

/// Embedded, in-process `AgentRuntime` reference implementation used to
/// exercise the conformance suite deterministically (no subprocess, no
/// network). Declares full `RuntimeSupports` and implements
/// [`FaultInjection`] completely, so a conformant run of the suite against
/// it produces zero `Skip`s.
struct FakeRuntime {
    shared: Arc<Mutex<Shared>>,
}

impl FakeRuntime {
    fn new() -> Self {
        Self {
            shared: Arc::new(Mutex::new(Shared {
                auth_fail_armed: false,
                next_session_seq: 0,
                active_session: None,
            })),
        }
    }
}

struct FakeSession {
    inner: Arc<Mutex<SessionInner>>,
    handle: SessionHandle,
    supports_concurrent: bool,
}

fn classify(prompt: &str) -> &'static str {
    if prompt.contains("hang") {
        "hang"
    } else if prompt.contains("emit:permission") {
        "permission"
    } else if prompt.contains("emit:artifact") {
        "artifact"
    } else {
        "happy"
    }
}

#[async_trait]
impl AgentRuntime for FakeRuntime {
    fn descriptor(&self) -> RuntimeDescriptor {
        RuntimeDescriptor {
            id: "fake_embedded",
            adapter_version: "0.1.0".to_string(),
            target_version: "n/a".to_string(),
            transport: Transport::Embedded,
            supports: RuntimeSupports {
                resume: true,
                steer: true,
                usage_reporting: false,
                diff_events: true,
                permission_bridge: true,
                concurrent_sessions: true,
            },
            policy_coverage: PolicyCoverage {
                tool_visibility: ToolVisibility::Full,
                approvals: ApprovalCoverage::Bridged,
                egress: EgressCoverage::InputFiltered,
                budget: BudgetCoverage::Estimated,
                sandbox: SandboxCoverage::Honored,
            },
        }
    }

    async fn health(&self) -> Result<RuntimeHealth, RuntimeError> {
        Ok(RuntimeHealth {
            detected_version: "0.1.0".to_string(),
            ready: true,
            detail: None,
        })
    }

    async fn start(&self, spec: SessionSpec) -> Result<Box<dyn RuntimeSession>, RuntimeError> {
        let mut shared = self.shared.lock().await;
        if shared.auth_fail_armed {
            shared.auth_fail_armed = false;
            return Err(RuntimeError::Auth(
                "credential resolution failed".to_string(),
            ));
        }
        shared.next_session_seq += 1;
        let handle = SessionHandle {
            runtime_id: "fake_embedded".to_string(),
            owner: spec.owner.clone(),
            external_ref: format!("fake-session-{}", shared.next_session_seq),
        };
        let inner = Arc::new(Mutex::new(SessionInner {
            status: SessionStatus::Running,
            pending_events: VecDeque::from([RuntimeEvent::Started {
                handle: handle.clone(),
            }]),
            next_task_id: 0,
            next_perm_id: 0,
            current: None,
            per_task_timeout: spec.timeout.per_task,
            workspace_root: spec.workspace.root.clone(),
            garbage_pending: false,
        }));
        shared.active_session = Some((handle.clone(), Arc::clone(&inner)));
        Ok(Box::new(FakeSession {
            inner,
            handle,
            supports_concurrent: true,
        }))
    }

    async fn resume(
        &self,
        handle: &SessionHandle,
        _spec: ResumeSpec,
    ) -> Result<Box<dyn RuntimeSession>, RuntimeError> {
        let shared = self.shared.lock().await;
        match &shared.active_session {
            Some((active_handle, inner)) if active_handle == handle => Ok(Box::new(FakeSession {
                inner: Arc::clone(inner),
                handle: handle.clone(),
                supports_concurrent: true,
            })),
            _ => Err(RuntimeError::NotResumable(
                "no active session matches this handle".to_string(),
            )),
        }
    }
}

#[async_trait]
impl FaultInjection for FakeRuntime {
    async fn induce_crash(&self) -> bool {
        let shared = self.shared.lock().await;
        let Some((_, inner)) = &shared.active_session else {
            return false;
        };
        let mut session = inner.lock().await;
        if let Some(task) = session.current.as_mut() {
            if !task.ended {
                task.ended = true;
                let id = task.id;
                session.pending_events.push_back(RuntimeEvent::Ended {
                    task: id,
                    outcome: TaskOutcome::Failed {
                        reason: "harness crashed".to_string(),
                    },
                });
            }
        }
        session.status = SessionStatus::Crashed;
        true
    }

    async fn induce_auth_failure(&self) -> bool {
        let mut shared = self.shared.lock().await;
        shared.auth_fail_armed = true;
        true
    }

    async fn feed_garbage_frame(&self) -> bool {
        let shared = self.shared.lock().await;
        let Some((_, inner)) = &shared.active_session else {
            return false;
        };
        let mut session = inner.lock().await;
        session.garbage_pending = true;
        true
    }
}

#[async_trait]
impl RuntimeSession for FakeSession {
    fn handle(&self) -> SessionHandle {
        self.handle.clone()
    }

    async fn submit(&mut self, input: TaskInput) -> Result<TaskId, RuntimeError> {
        let mut inner = self.inner.lock().await;
        if inner.status == SessionStatus::Crashed {
            return Err(RuntimeError::Crashed("session already crashed".to_string()));
        }
        if inner.garbage_pending {
            return Err(RuntimeError::Protocol(
                "malformed frame pending on transport".to_string(),
            ));
        }
        if !self.supports_concurrent {
            if let Some(task) = &inner.current {
                if !task.ended {
                    return Err(RuntimeError::Unavailable(
                        "a task is already active on this session".to_string(),
                    ));
                }
            }
        }

        let id = TaskId(inner.next_task_id);
        inner.next_task_id += 1;
        let mut task = TaskState {
            id,
            started_at: Instant::now(),
            ended: false,
            pending_permission: None,
        };

        match classify(&input.prompt) {
            "hang" => {
                inner.pending_events.push_back(RuntimeEvent::MessageDelta {
                    task: id,
                    text: "working...".to_string(),
                });
            }
            "permission" => {
                inner.pending_events.push_back(RuntimeEvent::MessageDelta {
                    task: id,
                    text: "checking permission...".to_string(),
                });
                let perm_id = PermissionRequestId(inner.next_perm_id);
                inner.next_perm_id += 1;
                inner
                    .pending_events
                    .push_back(RuntimeEvent::PermissionRequest {
                        task: id,
                        id: perm_id,
                        action: PermissionAction::WriteFile,
                        detail: "write guarded.txt".to_string(),
                    });
                task.pending_permission = Some(perm_id);
            }
            "artifact" => {
                let rel_path = PathBuf::from("artifact.txt");
                let content: &[u8] = b"fake artifact content";
                if let Err(e) = std::fs::write(inner.workspace_root.join(&rel_path), content) {
                    return Err(RuntimeError::Unavailable(format!(
                        "fake could not write artifact: {e}"
                    )));
                }
                let digest = format!("sha256:{:x}", Sha256::digest(content));
                inner.pending_events.push_back(RuntimeEvent::MessageDelta {
                    task: id,
                    text: "writing artifact".to_string(),
                });
                inner.pending_events.push_back(RuntimeEvent::Artifact {
                    task: id,
                    artifact: Artifact {
                        kind: ArtifactKind::File,
                        path: rel_path,
                        digest,
                        produced_by: None,
                    },
                });
                inner.pending_events.push_back(RuntimeEvent::Ended {
                    task: id,
                    outcome: TaskOutcome::Success,
                });
                task.ended = true;
            }
            _ => {
                inner.pending_events.push_back(RuntimeEvent::MessageDelta {
                    task: id,
                    text: "Hello".to_string(),
                });
                inner.pending_events.push_back(RuntimeEvent::MessageDelta {
                    task: id,
                    text: ", world".to_string(),
                });
                inner.pending_events.push_back(RuntimeEvent::Ended {
                    task: id,
                    outcome: TaskOutcome::Success,
                });
                task.ended = true;
            }
        }

        inner.current = Some(task);
        Ok(id)
    }

    async fn next_event(&mut self) -> Option<RuntimeEvent> {
        loop {
            {
                let mut inner = self.inner.lock().await;
                if let Some(evt) = inner.pending_events.pop_front() {
                    return Some(evt);
                }
                if matches!(inner.status, SessionStatus::Crashed | SessionStatus::Closed) {
                    return None;
                }
                let timed_out = match &inner.current {
                    Some(task) if !task.ended => {
                        task.started_at.elapsed() >= inner.per_task_timeout
                    }
                    _ => false,
                };
                if timed_out {
                    let id = {
                        let task = inner.current.as_mut().expect("checked Some above");
                        task.ended = true;
                        task.id
                    };
                    return Some(RuntimeEvent::Ended {
                        task: id,
                        outcome: TaskOutcome::TimedOut,
                    });
                }
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    async fn steer(&mut self, _text: &str) -> Result<(), RuntimeError> {
        let inner = self.inner.lock().await;
        if inner.status == SessionStatus::Crashed {
            return Err(RuntimeError::Crashed("session already crashed".to_string()));
        }
        Ok(())
    }

    async fn cancel(&mut self, _mode: CancelMode) -> Result<(), RuntimeError> {
        let mut inner = self.inner.lock().await;
        if let Some(task) = inner.current.as_mut() {
            if !task.ended {
                task.ended = true;
                let id = task.id;
                inner.pending_events.push_back(RuntimeEvent::Ended {
                    task: id,
                    outcome: TaskOutcome::Cancelled,
                });
            }
        }
        if inner.status != SessionStatus::Crashed {
            inner.status = SessionStatus::Cancelled;
        }
        Ok(())
    }

    async fn respond_permission(
        &mut self,
        id: PermissionRequestId,
        decision: PermissionDecision,
    ) -> Result<(), RuntimeError> {
        let mut inner = self.inner.lock().await;
        let task_id = match &inner.current {
            Some(t) if t.pending_permission == Some(id) => t.id,
            _ => {
                return Err(RuntimeError::Protocol(
                    "no matching pending permission request".to_string(),
                ))
            }
        };
        if let Some(t) = inner.current.as_mut() {
            t.pending_permission = None;
            t.ended = true;
        }
        match decision {
            PermissionDecision::Allow => {
                inner.pending_events.push_back(RuntimeEvent::ToolResult {
                    task: task_id,
                    name: "write_file".to_string(),
                    output_digest: "sha256:deadbeef".to_string(),
                    is_error: false,
                });
                inner.pending_events.push_back(RuntimeEvent::Ended {
                    task: task_id,
                    outcome: TaskOutcome::Success,
                });
            }
            PermissionDecision::Deny {
                scope: DenyScope::Instance,
            } => {
                // Deny this one request; the task still completes normally
                // (the model just didn't get to do the guarded action).
                inner.pending_events.push_back(RuntimeEvent::Ended {
                    task: task_id,
                    outcome: TaskOutcome::Success,
                });
            }
            PermissionDecision::Deny {
                scope: DenyScope::Turn,
            } => {
                // Ciclo 2.2: Turn-scoped deny gracefully cancels the task —
                // mirrors what `cancel(CancelMode::Graceful { .. })` does.
                inner.pending_events.push_back(RuntimeEvent::Ended {
                    task: task_id,
                    outcome: TaskOutcome::Cancelled,
                });
                if inner.status != SessionStatus::Crashed {
                    inner.status = SessionStatus::Cancelled;
                }
            }
        }
        Ok(())
    }

    async fn status(&self) -> Result<SessionStatus, RuntimeError> {
        let inner = self.inner.lock().await;
        if inner.garbage_pending {
            return Err(RuntimeError::Protocol(
                "malformed frame received on transport".to_string(),
            ));
        }
        Ok(inner.status)
    }
}

// ---------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------

fn make_spec(workspace_root: PathBuf) -> SessionSpec {
    SessionSpec {
        owner: "test-owner".to_string(),
        workspace: WorkspacePolicy {
            root: workspace_root,
            read_only: false,
            deny: Vec::new(),
        },
        sandbox: SandboxProfile::Isolated,
        permissions: PermissionProfile::default(),
        auth: AuthProfileRef("test-auth".to_string()),
        runtime_id: "fake".to_string(),
        timeout: TimeoutPolicy {
            per_task: Duration::from_secs(30),
            idle: Duration::from_secs(60),
        },
        env: EnvPolicy::default(),
        mcp_bridge: None,
        otel: OtelContext::default(),
    }
}

fn make_scenarios() -> ConformanceScenarios {
    ConformanceScenarios {
        happy_path: TaskInput {
            prompt: "say hello".to_string(),
            attachments: Vec::new(),
            expected: TaskExpectation::Conversation,
        },
        never_terminates: TaskInput {
            prompt: "hang forever".to_string(),
            attachments: Vec::new(),
            expected: TaskExpectation::Conversation,
        },
        requests_permission: TaskInput {
            prompt: "emit:permission".to_string(),
            attachments: Vec::new(),
            expected: TaskExpectation::CodeChange,
        },
        produces_artifact: TaskInput {
            prompt: "emit:artifact".to_string(),
            attachments: Vec::new(),
            expected: TaskExpectation::CodeChange,
        },
        watchdog: conformance::DEFAULT_WATCHDOG,
    }
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

/// `run_all` against `FakeRuntime` must produce zero `Fail` and zero `Skip`
/// — the fake declares full `RuntimeSupports`, `ApprovalCoverage::Bridged`,
/// and a complete `FaultInjection` implementation, so nothing is applicable
/// to skip.
#[tokio::test]
async fn agent_runtime_conformance_suite_all_pass() {
    let workspace = tempfile::tempdir().expect("tempdir");
    let spec = make_spec(workspace.path().to_path_buf());
    let scenarios = make_scenarios();
    let runtime = FakeRuntime::new();

    let results = conformance::run_all(&runtime, &spec, &scenarios).await;
    let report = conformance::format_report(&results);

    let failed: Vec<_> = results.iter().filter(|(_, r)| r.is_fail()).collect();
    let skipped: Vec<_> = results.iter().filter(|(_, r)| r.is_skip()).collect();

    assert!(failed.is_empty(), "conformance failures:\n{report}");
    assert!(
        skipped.is_empty(),
        "unexpected skips (FakeRuntime declares full support):\n{report}"
    );
    assert_eq!(
        results.len(),
        14,
        "expected all 14 checks to run:\n{report}"
    );
}

/// Ciclo 2.2 acceptance criterion: `DenyScope::Turn` makes the adapter
/// cancel the task gracefully after denying (contract change 4/6 of the
/// A-01 v2 review — `docs/revamp/C2-approval-port-design.md` §3).
#[tokio::test]
async fn deny_turn_scope_cancels_the_task() {
    let workspace = tempfile::tempdir().expect("tempdir");
    let spec = make_spec(workspace.path().to_path_buf());
    let runtime = FakeRuntime::new();
    let mut session = runtime.start(spec).await.expect("start");

    assert!(matches!(
        session.next_event().await,
        Some(RuntimeEvent::Started { .. })
    ));

    let task = session
        .submit(TaskInput {
            prompt: "emit:permission".to_string(),
            attachments: Vec::new(),
            expected: TaskExpectation::CodeChange,
        })
        .await
        .expect("submit");

    let req_id = loop {
        match session.next_event().await.expect("event stream open") {
            RuntimeEvent::PermissionRequest { task: t, id, .. } if t == task => break id,
            _ => continue,
        }
    };

    session
        .respond_permission(
            req_id,
            PermissionDecision::Deny {
                scope: DenyScope::Turn,
            },
        )
        .await
        .expect("respond_permission");

    let outcome = loop {
        match session.next_event().await.expect("event stream open") {
            RuntimeEvent::Ended { task: t, outcome } if t == task => break outcome,
            _ => continue,
        }
    };
    assert_eq!(
        outcome,
        TaskOutcome::Cancelled,
        "DenyScope::Turn must cancel the task, not let it complete normally"
    );
    assert_eq!(
        session.status().await.expect("status"),
        SessionStatus::Cancelled
    );
}

/// Symmetric coverage: `DenyScope::Instance` preserves the pre-Ciclo-2.2
/// behavior — only the guarded action is blocked, the task still completes.
#[tokio::test]
async fn deny_instance_scope_leaves_task_completing_normally() {
    let workspace = tempfile::tempdir().expect("tempdir");
    let spec = make_spec(workspace.path().to_path_buf());
    let runtime = FakeRuntime::new();
    let mut session = runtime.start(spec).await.expect("start");

    assert!(matches!(
        session.next_event().await,
        Some(RuntimeEvent::Started { .. })
    ));

    let task = session
        .submit(TaskInput {
            prompt: "emit:permission".to_string(),
            attachments: Vec::new(),
            expected: TaskExpectation::CodeChange,
        })
        .await
        .expect("submit");

    let req_id = loop {
        match session.next_event().await.expect("event stream open") {
            RuntimeEvent::PermissionRequest { task: t, id, .. } if t == task => break id,
            _ => continue,
        }
    };

    session
        .respond_permission(
            req_id,
            PermissionDecision::Deny {
                scope: DenyScope::Instance,
            },
        )
        .await
        .expect("respond_permission");

    let outcome = loop {
        match session.next_event().await.expect("event stream open") {
            RuntimeEvent::Ended { task: t, outcome } if t == task => break outcome,
            _ => continue,
        }
    };
    assert_eq!(
        outcome,
        TaskOutcome::Success,
        "DenyScope::Instance must not cancel the task"
    );
}

#[test]
fn session_handle_serde_round_trip() {
    let handle = SessionHandle {
        runtime_id: "fake_embedded".to_string(),
        owner: "alice".to_string(),
        external_ref: "sess-1".to_string(),
    };
    let json = serde_json::to_string(&handle).expect("serialize");
    let back: SessionHandle = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(handle, back);
}

#[test]
fn runtime_error_auth_display_never_contains_secret_material() {
    let secret = "sk-super-secret-token-ABC123";
    // A conformant Auth error describes the failure generically; it must
    // never be constructed by threading the credential itself into the
    // message (that's the whole point of the typed variant).
    let err = RuntimeError::Auth("credential resolution failed".to_string());
    let rendered = err.to_string();
    assert!(!rendered.contains(secret));
    assert!(rendered.contains("auth failed"));
}
