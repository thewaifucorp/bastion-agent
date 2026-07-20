//! Concrete adaptive-execution port implementations wired to an external
//! [`bastion_agent_runtime::AgentRuntime`] (US-203, US-206).
//!
//! `bastion_runtime::task` defines three neutral seams the durable `Pursue`
//! cycle drives — `TaskExecutor`, `Chooser`, `Verifier` — and owns none of
//! the policy behind them. This module supplies the product-level policy for
//! a coding `Pursue` task: delegate every action to a registered external
//! harness (Codex/ACP, via `RuntimeRegistry`), retry a bounded number of
//! times, and verify deterministically off the harness's own terminal exit
//! status. No LLM planning/judging call lives here — everything is a pure
//! function of `CycleHistory`/`Evidence` (US-104: deterministic verification
//! before any judge).
//!
//! The daemon driver that wires these into the actual `AdaptiveCycle` loop is
//! composed elsewhere (`main.rs`/composition root); this module is
//! self-contained and only depends on the neutral `bastion_runtime::task`
//! contract plus `bastion_agent_runtime`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use bastion_agent_runtime::{
    AuthProfileRef, DenyScope, EnvPolicy, OtelContext, PermissionDecision, PermissionProfile,
    RuntimeEvent, SandboxProfile, SessionSpec, TaskExpectation, TaskInput, TaskOutcome,
    TimeoutPolicy, WorkspacePolicy,
};
use bastion_runtime::agent::backend::RuntimeRegistry;
use bastion_runtime::task::{
    ActionId, ActionKind, ActionOutcome, AdaptiveCycle, ArtifactRef, AttemptId, CandidateAction,
    Chooser, ChosenStep, CycleHistory, Evidence, EvidenceId, EvidenceKind, TaskCase, TaskExecutor,
    TaskStore, UsageAccum, Verdict, VerdictProvenance, VerificationStatus, Verifier,
};

use super::observer::TracingObserver;

/// Fallback runtime id used when `execute` is handed an action whose kind
/// isn't `ActionKind::Runtime` (a host wiring that routed a `Capability`/
/// `Respond`/`Delegate` action through this executor by mistake) — logged,
/// never silently dropped.
const DEFAULT_RUNTIME_ID: &str = "codex_app_server";

fn now_nanos() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as i64
}

/// Monotonic counter for synthesizing [`EvidenceId`]s within one `execute`
/// call — evidence ids only need to be unique per attempt, and the cycle
/// stamps the real `attempt`/`action` correlation onto each `Evidence`
/// afterwards (see `bastion_runtime::task::ports` rustdoc).
static EVIDENCE_SEQ: AtomicU64 = AtomicU64::new(0);

fn next_evidence_id(kind: &str) -> EvidenceId {
    let n = EVIDENCE_SEQ.fetch_add(1, Ordering::Relaxed);
    EvidenceId(format!("{kind}-{n}"))
}

/// Owner workspace root for a delegated session, sanitized the same way
/// `bastion_runtime::agent::loop_::runtime_workspace_root` sanitizes owner
/// ids for its own runtime-backed conversation turns (non-alphanumeric ->
/// `_`), so both paths are equally safe to use as a directory name.
fn sanitize_owner(owner: &str) -> String {
    let sanitized: String = owner
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if sanitized.is_empty() {
        "_owner".to_string()
    } else {
        sanitized
    }
}

/// [`TaskExecutor`] that delegates a `Pursue` task's chosen action to an
/// external [`AgentRuntime`] resolved from `registry` (US-203, US-206).
///
/// Drives one full session per `execute` call: starts a fresh harness
/// session scoped to `owner`'s workspace, submits the task's objective as a
/// `CodeChange` task, drains every event to completion, and folds the
/// session's output into `Evidence`/`UsageAccum`. Mirrors the event-loop
/// shape of `bastion_runtime::agent::loop_::AgentLoop::run_runtime_backed_turn`
/// (mode 2) — same `SessionSpec` construction, same fail-closed
/// `PermissionRequest` handling (`Deny { scope: Turn }`, no synchronous
/// cross-attempt pause channel exists here) — but scoped to ONE delegated
/// action rather than an interactive conversation turn. Unlike that turn
/// path, a session here is never persisted/resumed: a `Pursue` retry is a
/// new attempt, so it gets a fresh, disposable session rather than a
/// reattached one.
pub struct RuntimeTaskExecutor {
    /// Registry of healthy `AgentRuntime` adapters this executor resolves
    /// `runtime_id`s against.
    pub registry: RuntimeRegistry,
    /// Owner the delegated session is scoped to (workspace root, audit).
    pub owner: String,
}

impl RuntimeTaskExecutor {
    /// Construct an executor over an already-populated [`RuntimeRegistry`].
    pub fn new(registry: RuntimeRegistry, owner: impl Into<String>) -> Self {
        Self {
            registry,
            owner: owner.into(),
        }
    }
}

#[async_trait]
impl TaskExecutor for RuntimeTaskExecutor {
    /// Execute `action` by delegating to the `AgentRuntime` it names (or the
    /// [`DEFAULT_RUNTIME_ID`] fallback when `action.kind` isn't
    /// `ActionKind::Runtime`), driving the harness session to its terminal
    /// event. Never decides success itself — every observation becomes
    /// `Evidence`, including a final `ExitStatus` record encoding the
    /// harness's own `TaskOutcome`, for `RuntimeOutcomeVerifier` to judge.
    async fn execute(
        &self,
        action: &CandidateAction,
        case: &TaskCase,
    ) -> anyhow::Result<ActionOutcome> {
        let runtime_id = match &action.kind {
            ActionKind::Runtime { runtime_id, .. } => runtime_id.clone(),
            other => {
                tracing::warn!(
                    event = "adaptive_exec_non_runtime_action_kind",
                    task = %case.id,
                    kind = ?other,
                    fallback_runtime_id = DEFAULT_RUNTIME_ID,
                );
                DEFAULT_RUNTIME_ID.to_string()
            }
        };

        let runtime = self.registry.resolve(&runtime_id).await.map_err(|e| {
            anyhow::anyhow!("resolving runtime '{runtime_id}' for task {}: {e}", case.id)
        })?;

        let workspace_root = std::env::temp_dir()
            .join("bastion-adaptive-exec")
            .join(sanitize_owner(&self.owner));
        // Best-effort: a pre-existing/creatable workspace root is required
        // for the session to do anything useful, but its absence is
        // surfaced later (start()/submit() failing) rather than here — this
        // mirrors `run_runtime_backed_turn`'s own `let _ = create_dir_all`.
        let _ = tokio::fs::create_dir_all(&workspace_root).await;

        let spec = SessionSpec {
            owner: self.owner.clone(),
            workspace: WorkspacePolicy {
                root: workspace_root,
                read_only: false,
                deny: Vec::new(),
            },
            sandbox: SandboxProfile::WorkspaceNet,
            permissions: PermissionProfile::default(),
            auth: AuthProfileRef("host-cli-login".to_string()),
            runtime_id: runtime_id.clone(),
            timeout: TimeoutPolicy {
                per_task: Duration::from_secs(600),
                idle: Duration::from_secs(300),
            },
            env: EnvPolicy::default(),
            mcp_bridge: None,
            otel: OtelContext::default(),
        };

        let mut session = runtime.start(spec).await.map_err(|e| {
            anyhow::anyhow!(
                "starting runtime '{runtime_id}' session for task {}: {e}",
                case.id
            )
        })?;

        let task_id = session
            .submit(TaskInput {
                prompt: case.frame.objective.clone(),
                attachments: Vec::new(),
                expected: TaskExpectation::CodeChange,
            })
            .await
            .map_err(|e| {
                anyhow::anyhow!("submitting task {} to runtime '{runtime_id}': {e}", case.id)
            })?;

        let mut usage = UsageAccum::default();
        let mut evidence = Vec::new();
        let mut transcript = String::new();

        let outcome = loop {
            let Some(event) = session.next_event().await else {
                anyhow::bail!(
                    "runtime '{runtime_id}' session event stream closed before task {} ended",
                    case.id
                );
            };
            match event {
                RuntimeEvent::MessageDelta { task, text } if task == task_id => {
                    transcript.push_str(&text);
                }
                RuntimeEvent::Usage { task, delta } if task == task_id => {
                    usage.add_tokens(delta.input_tokens, delta.output_tokens);
                }
                RuntimeEvent::Diff {
                    task,
                    path,
                    added,
                    removed,
                } if task == task_id => {
                    evidence.push(Evidence {
                        id: next_evidence_id("diff"),
                        attempt: AttemptId(String::new()),
                        action: None,
                        kind: EvidenceKind::Diff,
                        source_ref: ArtifactRef(format!("{}:+{added}-{removed}", path.display())),
                        trusted: false,
                        max_tier: None,
                        captured_at: now_nanos(),
                    });
                }
                RuntimeEvent::Artifact { task, artifact } if task == task_id => {
                    evidence.push(Evidence {
                        id: next_evidence_id("artifact"),
                        attempt: AttemptId(String::new()),
                        action: None,
                        kind: EvidenceKind::Artifact,
                        source_ref: ArtifactRef(format!(
                            "{}#{}",
                            artifact.path.display(),
                            artifact.digest
                        )),
                        trusted: false,
                        max_tier: None,
                        captured_at: now_nanos(),
                    });
                }
                RuntimeEvent::PermissionRequest { id, .. } => {
                    // Fail-closed, matching `run_runtime_backed_turn`: this
                    // call drives one attempt synchronously to completion —
                    // there is no cross-attempt pause channel to genuinely
                    // park on here, so every request is denied and the task
                    // ends rather than silently auto-approved.
                    let deny = PermissionDecision::Deny {
                        scope: DenyScope::Turn,
                    };
                    if let Err(e) = session.respond_permission(id, deny).await {
                        tracing::warn!(
                            event = "adaptive_exec_permission_respond_failed",
                            runtime_id = %runtime_id,
                            task = %case.id,
                            error = %e,
                        );
                    }
                }
                RuntimeEvent::Warning { code, detail, .. } => {
                    tracing::warn!(
                        event = "adaptive_exec_runtime_warning",
                        runtime_id = %runtime_id,
                        task = %case.id,
                        ?code,
                        detail = %detail,
                    );
                }
                RuntimeEvent::Ended { task, outcome } if task == task_id => break outcome,
                // Started/ToolCall/ToolResult/Thinking, and any event for a
                // different task on this session (shouldn't occur — one task
                // per session on this path): observability-only this cycle,
                // matching `run_runtime_backed_turn`'s own scope.
                _ => {}
            }
        };
        tracing::debug!(
            event = "adaptive_exec_transcript",
            runtime_id = %runtime_id,
            task = %case.id,
            chars = transcript.len(),
        );

        let exit_label = match &outcome {
            TaskOutcome::Success => "success".to_string(),
            TaskOutcome::Cancelled => "cancelled".to_string(),
            TaskOutcome::TimedOut => "timed_out".to_string(),
            TaskOutcome::Failed { reason } => format!("failed:{reason}"),
        };
        evidence.push(Evidence {
            id: next_evidence_id("exit"),
            attempt: AttemptId(String::new()),
            action: None,
            kind: EvidenceKind::ExitStatus,
            source_ref: ArtifactRef(exit_label),
            trusted: true,
            max_tier: None,
            captured_at: now_nanos(),
        });

        Ok(ActionOutcome {
            evidence,
            usage,
            pending_approval: None,
        })
    }
}

/// [`Chooser`] for a `Pursue` coding task: a deterministic, bounded-retry
/// policy over a single external runtime (US-203, US-206). No LLM planning
/// call — the decision is a pure function of `history.attempt_count` and the
/// previous [`Verdict`], so the retry bound is always explicit and finite
/// (US-104: no adaptive choice hides an unbounded loop).
pub struct CodingChooser {
    /// Runtime id used for every `Act` this chooser produces, e.g.
    /// `"codex_app_server"`.
    pub runtime_id: String,
    /// Attempts allowed before an unconverged task (`Failed`/`Unverified`/
    /// `Partial`) escalates instead of retrying.
    pub max_attempts: u32,
}

impl CodingChooser {
    /// A chooser bound to `runtime_id` with the default two-attempt budget.
    pub fn new(runtime_id: impl Into<String>) -> Self {
        Self {
            runtime_id: runtime_id.into(),
            max_attempts: 2,
        }
    }

    fn act(&self, case: &TaskCase, attempt_count: u32) -> CandidateAction {
        CandidateAction {
            id: ActionId(format!("{}-attempt-{attempt_count}", case.id)),
            kind: ActionKind::Runtime {
                runtime_id: self.runtime_id.clone(),
                input_ref: ArtifactRef(case.id.to_string()),
            },
            rationale: format!(
                "delegate coding objective for task {} to runtime '{}' (attempt {attempt_count})",
                case.id, self.runtime_id
            ),
            belief_refs: Vec::new(),
        }
    }
}

impl Default for CodingChooser {
    /// Defaults to `"codex_app_server"` — the same adapter id
    /// `agent_runtime_registry` registers first.
    fn default() -> Self {
        Self::new(DEFAULT_RUNTIME_ID)
    }
}

#[async_trait]
impl Chooser for CodingChooser {
    /// First attempt: always `Act`. After that: `Complete` on a succeeded
    /// verdict, bounded retry (`Act`) below `max_attempts`, else `Escalate`.
    async fn choose(
        &self,
        case: &TaskCase,
        history: &CycleHistory<'_>,
    ) -> anyhow::Result<ChosenStep> {
        if history.attempt_count == 0 {
            return Ok(ChosenStep::Act(self.act(case, history.attempt_count)));
        }
        match history.last_verdict.map(|v| v.status) {
            Some(VerificationStatus::Succeeded) => Ok(ChosenStep::Complete),
            // Failed, Unverified, Partial, or (defensively) no verdict yet:
            // retry while under budget, otherwise escalate rather than loop
            // forever.
            _ => {
                if history.attempt_count >= self.max_attempts {
                    Ok(ChosenStep::Escalate(
                        "coding task did not converge".to_string(),
                    ))
                } else {
                    Ok(ChosenStep::Act(self.act(case, history.attempt_count)))
                }
            }
        }
    }
}

/// [`Verifier`] that judges an attempt purely from the terminal `ExitStatus`
/// [`Evidence`] [`RuntimeTaskExecutor`] appends to every attempt (US-203,
/// US-104: deterministic verification, no LLM judge). Success/failure is
/// read from that evidence's `source_ref` (`"success"` vs. anything else —
/// see `RuntimeTaskExecutor::execute`); an attempt with no `ExitStatus`
/// evidence at all is `Unverified`, never silently `Succeeded`.
pub struct RuntimeOutcomeVerifier;

#[async_trait]
impl Verifier for RuntimeOutcomeVerifier {
    async fn verify(
        &self,
        _case: &TaskCase,
        attempt: &AttemptId,
        evidence: &[Evidence],
    ) -> anyhow::Result<Verdict> {
        let exit = evidence.iter().find(|e| e.kind == EvidenceKind::ExitStatus);
        let (status, detail) = match exit {
            Some(e) if e.source_ref.as_str() == "success" => (VerificationStatus::Succeeded, None),
            Some(e) => (
                VerificationStatus::Failed,
                Some(e.source_ref.as_str().to_string()),
            ),
            None => (VerificationStatus::Unverified, None),
        };
        let used_evidence = exit.map(|e| vec![e.id.clone()]).unwrap_or_default();
        Ok(Verdict {
            attempt: attempt.clone(),
            status,
            provenance: VerdictProvenance::Deterministic,
            evidence: used_evidence,
            detail,
        })
    }
}

/// Assemble the coding `AdaptiveCycle` for `owner`: the deterministic
/// `CodingChooser` + runtime-backed `RuntimeTaskExecutor` + deterministic
/// `RuntimeOutcomeVerifier`, over the shared `TaskStore`, emitting lifecycle
/// events to tracing. This is what the daemon spawns to drain an enqueued
/// `Pursue` task (US-203). `registry` is cloned into the executor.
pub fn coding_cycle(
    store: &Arc<dyn TaskStore>,
    registry: &RuntimeRegistry,
    owner: &str,
) -> AdaptiveCycle {
    AdaptiveCycle::new(
        store.clone(),
        Arc::new(CodingChooser::default()),
        Arc::new(RuntimeTaskExecutor::new(
            registry.clone(),
            owner.to_string(),
        )),
        Arc::new(RuntimeOutcomeVerifier),
        Arc::new(TracingObserver),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use bastion_runtime::task::{
        AcceptanceCriterion, Bounds, CorrelationIds, ExecutionMode, Frame, Intent, IntentOrigin,
        OpaqueState, TaskCaseId, TaskStatus,
    };

    fn sample_case() -> TaskCase {
        TaskCase {
            id: TaskCaseId("t1".into()),
            owner: "alice".into(),
            mode: ExecutionMode::Pursue,
            intent: Intent {
                owner: "alice".into(),
                mode: ExecutionMode::Pursue,
                summary: "fix the failing build".into(),
                origin: IntentOrigin::Message,
            },
            frame: Frame {
                objective: "fix the failing test".into(),
                acceptance: vec![AcceptanceCriterion {
                    description: "tests pass".into(),
                    check: Some("cargo-test".into()),
                }],
                context_refs: vec![],
            },
            bounds: Bounds::default(),
            status: TaskStatus::Running,
            stop_reason: None,
            attempts: vec![],
            pending_approvals: vec![],
            next_decision: None,
            usage: UsageAccum::default(),
            parent: None,
            correlation: CorrelationIds::default(),
            business_state: OpaqueState::default(),
            created_at: 0,
            updated_at: 0,
            revision: 1,
        }
    }

    fn sample_verdict(status: VerificationStatus) -> Verdict {
        Verdict {
            attempt: AttemptId("a1".into()),
            status,
            provenance: VerdictProvenance::Deterministic,
            evidence: vec![],
            detail: None,
        }
    }

    #[tokio::test]
    async fn chooser_acts_on_first_attempt() {
        let chooser = CodingChooser::default();
        let case = sample_case();
        let usage = UsageAccum::default();
        let history = CycleHistory {
            last_verdict: None,
            attempt_count: 0,
            usage: &usage,
        };
        let step = chooser.choose(&case, &history).await.expect("choose");
        let ChosenStep::Act(action) = step else {
            panic!("expected Act on the first attempt");
        };
        assert!(
            matches!(action.kind, ActionKind::Runtime { ref runtime_id, .. } if runtime_id == "codex_app_server")
        );
    }

    #[tokio::test]
    async fn chooser_completes_on_succeeded_verdict() {
        let chooser = CodingChooser::default();
        let case = sample_case();
        let usage = UsageAccum::default();
        let verdict = sample_verdict(VerificationStatus::Succeeded);
        let history = CycleHistory {
            last_verdict: Some(&verdict),
            attempt_count: 1,
            usage: &usage,
        };
        let step = chooser.choose(&case, &history).await.expect("choose");
        assert!(matches!(step, ChosenStep::Complete));
    }

    #[tokio::test]
    async fn chooser_retries_under_budget_then_escalates_at_the_bound() {
        let chooser = CodingChooser::new("codex_app_server"); // max_attempts defaults to 2
        let case = sample_case();
        let usage = UsageAccum::default();
        let failed = sample_verdict(VerificationStatus::Failed);

        let under_budget = CycleHistory {
            last_verdict: Some(&failed),
            attempt_count: 1,
            usage: &usage,
        };
        let step = chooser.choose(&case, &under_budget).await.expect("choose");
        assert!(matches!(step, ChosenStep::Act(_)));

        let at_budget = CycleHistory {
            last_verdict: Some(&failed),
            attempt_count: 2,
            usage: &usage,
        };
        let step = chooser.choose(&case, &at_budget).await.expect("choose");
        assert!(matches!(step, ChosenStep::Escalate(_)));
    }

    #[tokio::test]
    async fn chooser_treats_unverified_and_partial_as_retryable() {
        let chooser = CodingChooser::default();
        let case = sample_case();
        let usage = UsageAccum::default();
        for status in [VerificationStatus::Unverified, VerificationStatus::Partial] {
            let verdict = sample_verdict(status);
            let history = CycleHistory {
                last_verdict: Some(&verdict),
                attempt_count: 1,
                usage: &usage,
            };
            let step = chooser.choose(&case, &history).await.expect("choose");
            assert!(matches!(step, ChosenStep::Act(_)));
        }
    }

    #[tokio::test]
    async fn verifier_maps_success_exit_status_to_succeeded() {
        let verifier = RuntimeOutcomeVerifier;
        let case = sample_case();
        let attempt = AttemptId("a1".into());
        let evidence = vec![Evidence {
            id: EvidenceId("e1".into()),
            attempt: attempt.clone(),
            action: None,
            kind: EvidenceKind::ExitStatus,
            source_ref: ArtifactRef("success".into()),
            trusted: true,
            max_tier: None,
            captured_at: 0,
        }];
        let verdict = verifier
            .verify(&case, &attempt, &evidence)
            .await
            .expect("verify");
        assert_eq!(verdict.status, VerificationStatus::Succeeded);
        assert_eq!(verdict.provenance, VerdictProvenance::Deterministic);
        assert_eq!(verdict.evidence, vec![EvidenceId("e1".into())]);
    }

    #[tokio::test]
    async fn verifier_maps_non_success_exit_status_to_failed() {
        let verifier = RuntimeOutcomeVerifier;
        let case = sample_case();
        let attempt = AttemptId("a1".into());
        let evidence = vec![Evidence {
            id: EvidenceId("e2".into()),
            attempt: attempt.clone(),
            action: None,
            kind: EvidenceKind::ExitStatus,
            source_ref: ArtifactRef("failed:build error".into()),
            trusted: true,
            max_tier: None,
            captured_at: 0,
        }];
        let verdict = verifier
            .verify(&case, &attempt, &evidence)
            .await
            .expect("verify");
        assert_eq!(verdict.status, VerificationStatus::Failed);
        assert_eq!(verdict.detail, Some("failed:build error".to_string()));
    }

    #[tokio::test]
    async fn verifier_is_unverified_with_no_exit_status_evidence() {
        let verifier = RuntimeOutcomeVerifier;
        let case = sample_case();
        let attempt = AttemptId("a1".into());
        let verdict = verifier.verify(&case, &attempt, &[]).await.expect("verify");
        assert_eq!(verdict.status, VerificationStatus::Unverified);
        assert!(verdict.evidence.is_empty());
    }
}
