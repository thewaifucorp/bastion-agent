//! Pure `bastion_runtime::task::*` → `dto::*` translation (US — External
//! Control Plane and SDK, Phase 2: "Read-only task API").
//!
//! No I/O, no auth, no store access — these functions only reshape data
//! already fetched. Kept separate from `routes.rs` so the translation logic
//! (the part most likely to silently drift from Core's real field shapes) is
//! testable without spinning up an axum app or a store.

use bastion_runtime::task::{
    Attempt, Bounds, BudgetKind, ExecutionMode, StopReason, TaskCase, TaskStatus, UsageAccum,
    VerificationStatus,
};

use super::dto::{
    AttemptSummaryDto, AttemptVerificationDto, BudgetSummaryDto, StopReasonDto, TaskMode,
    TaskResource, TaskStatusDto,
};

pub fn task_mode(mode: ExecutionMode) -> TaskMode {
    match mode {
        ExecutionMode::Respond => TaskMode::Respond,
        ExecutionMode::Act => TaskMode::Act,
        ExecutionMode::Pursue => TaskMode::Pursue,
    }
}

pub fn task_status(status: TaskStatus) -> TaskStatusDto {
    match status {
        TaskStatus::Pending => TaskStatusDto::Pending,
        TaskStatus::Running => TaskStatusDto::Running,
        TaskStatus::AwaitingApproval => TaskStatusDto::AwaitingApproval,
        TaskStatus::Paused => TaskStatusDto::Paused,
        TaskStatus::Completed => TaskStatusDto::Completed,
        TaskStatus::Escalated => TaskStatusDto::Escalated,
        TaskStatus::Cancelled => TaskStatusDto::Cancelled,
        TaskStatus::Failed => TaskStatusDto::Failed,
    }
}

/// Matches the `dimension` values documented on `dto::StopReasonDto::BudgetExceeded`
/// and declared in the OpenAPI fixture's `StopReason.dimension` (implicitly, as a
/// free-form string) — explicit snake_case mapping, never a naive `.to_lowercase()`
/// (`WallClock` must become `"wall_clock"`, not `"wallclock"`).
fn budget_kind_dimension(kind: BudgetKind) -> &'static str {
    match kind {
        BudgetKind::Steps => "steps",
        BudgetKind::WallClock => "wall_clock",
        BudgetKind::Tokens => "tokens",
        BudgetKind::Money => "money",
        BudgetKind::Parallelism => "parallelism",
    }
}

pub fn stop_reason(reason: &StopReason) -> StopReasonDto {
    match reason {
        StopReason::Completed => StopReasonDto::Completed,
        StopReason::BudgetExceeded(kind) => StopReasonDto::BudgetExceeded {
            dimension: budget_kind_dimension(*kind).to_string(),
        },
        StopReason::Cancelled => StopReasonDto::Cancelled,
        StopReason::AwaitingApproval => StopReasonDto::AwaitingApproval,
        StopReason::Impossible(reason) => StopReasonDto::Impossible {
            reason: reason.clone(),
        },
        StopReason::Escalated(reason) => StopReasonDto::Escalated {
            reason: reason.clone(),
        },
    }
}

pub fn verification_status(status: VerificationStatus) -> AttemptVerificationDto {
    match status {
        VerificationStatus::Unverified => AttemptVerificationDto::Unverified,
        VerificationStatus::Failed => AttemptVerificationDto::Failed,
        VerificationStatus::Partial => AttemptVerificationDto::Partial,
        VerificationStatus::Succeeded => AttemptVerificationDto::Succeeded,
    }
}

/// `UsageAccum` + `Bounds` → `BudgetSummaryDto` (dto.rs's own doc comment
/// names this exact merge).
pub fn budget_summary(usage: &UsageAccum, bounds: &Bounds) -> BudgetSummaryDto {
    BudgetSummaryDto {
        llm_calls: usage.llm_calls,
        steps: usage.steps,
        total_tokens: usage.input_tokens.saturating_add(usage.output_tokens),
        cost_usd: usage.cost_usd,
        cost_coverage: match usage.cost_coverage {
            bastion_agent_runtime::BudgetCoverage::Reported => "reported",
            bastion_agent_runtime::BudgetCoverage::Estimated => "estimated",
            bastion_agent_runtime::BudgetCoverage::Unknown => "unknown",
        }
        .to_string(),
        wall_clock_ms: usage.wall_clock_ms,
        max_cost_usd: bounds.max_cost_usd,
        max_steps: bounds.max_steps,
    }
}

/// `Attempt` → `AttemptSummaryDto`. Deliberately drops `actions`,
/// `belief_refs`, and the `Verdict`'s `provenance`/`detail`/`evidence` ids —
/// see the DTO's own doc comment ("Evidence defaults to metadata/safe
/// summaries").
pub fn attempt_summary(attempt: &Attempt) -> AttemptSummaryDto {
    AttemptSummaryDto {
        id: attempt.id.to_string(),
        started_at: attempt.started_at,
        ended_at: attempt.ended_at,
        verified: attempt
            .verdict
            .as_ref()
            .map(|v| verification_status(v.status)),
        llm_calls: attempt.usage.llm_calls,
        total_tokens: attempt
            .usage
            .input_tokens
            .saturating_add(attempt.usage.output_tokens),
        cost_usd: attempt.usage.cost_usd,
    }
}

/// `TaskCase` → `TaskResource`.
///
/// `attempts` is caller-supplied rather than derived from `case.attempts`
/// (which in Core is only `Vec<AttemptId>` — resolving each id to a full
/// `Attempt` is a separate store call the caller already had to make, or
/// deliberately didn't, to avoid an N+1 query fan-out on list endpoints; see
/// `routes.rs`'s doc comments on `list_tasks` vs `get_task`).
///
/// `external_ref` is recovered from `case.business_state` via
/// `super::business_state::external_ref` (Phase 3) — `None` for any task
/// that wasn't created through `POST /v1/tasks` with one set (Core itself
/// has no `external_ref` field at all).
pub fn task_resource(case: &TaskCase, attempts: Vec<AttemptSummaryDto>) -> TaskResource {
    TaskResource {
        id: case.id.to_string(),
        owner_id: case.owner.clone(),
        external_ref: super::business_state::external_ref(&case.business_state.0),
        mode: task_mode(case.mode),
        objective: case.frame.objective.clone(),
        status: task_status(case.status),
        stop_reason: case.stop_reason.as_ref().map(stop_reason),
        created_at: case.created_at,
        updated_at: case.updated_at,
        revision: case.revision,
        budget_summary: budget_summary(&case.usage, &case.bounds),
        attempts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bastion_agent_runtime::BudgetCoverage;
    use bastion_runtime::task::{
        AttemptId, Frame, Intent, IntentOrigin, OpaqueState, TaskCaseId, Verdict, VerdictProvenance,
    };

    fn sample_usage() -> UsageAccum {
        UsageAccum {
            llm_calls: 2,
            steps: 3,
            input_tokens: 100,
            output_tokens: 50,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cost_usd: Some(0.42),
            cost_coverage: BudgetCoverage::Reported,
            wall_clock_ms: 1234,
        }
    }

    fn sample_case() -> TaskCase {
        TaskCase {
            id: TaskCaseId("task_1".into()),
            owner: "alice".into(),
            mode: ExecutionMode::Pursue,
            intent: Intent {
                owner: "alice".into(),
                mode: ExecutionMode::Pursue,
                summary: "do the thing".into(),
                origin: IntentOrigin::Message,
            },
            frame: Frame {
                objective: "Ship the thing".into(),
                acceptance: vec![],
                context_refs: vec![],
            },
            bounds: Bounds {
                max_steps: Some(20),
                max_wall_clock_ms: None,
                max_tokens: None,
                max_cost_usd: Some(5.0),
                max_parallelism: None,
            },
            status: TaskStatus::Running,
            stop_reason: None,
            attempts: vec![],
            pending_approvals: vec![],
            next_decision: None,
            usage: sample_usage(),
            parent: None,
            correlation: Default::default(),
            business_state: Default::default(),
            created_at: 1000,
            updated_at: 2000,
            revision: 3,
        }
    }

    #[test]
    fn task_mode_maps_all_variants() {
        assert_eq!(task_mode(ExecutionMode::Respond), TaskMode::Respond);
        assert_eq!(task_mode(ExecutionMode::Act), TaskMode::Act);
        assert_eq!(task_mode(ExecutionMode::Pursue), TaskMode::Pursue);
    }

    #[test]
    fn task_status_maps_all_eight_variants() {
        assert_eq!(task_status(TaskStatus::Pending), TaskStatusDto::Pending);
        assert_eq!(task_status(TaskStatus::Running), TaskStatusDto::Running);
        assert_eq!(
            task_status(TaskStatus::AwaitingApproval),
            TaskStatusDto::AwaitingApproval
        );
        assert_eq!(task_status(TaskStatus::Paused), TaskStatusDto::Paused);
        assert_eq!(task_status(TaskStatus::Completed), TaskStatusDto::Completed);
        assert_eq!(task_status(TaskStatus::Escalated), TaskStatusDto::Escalated);
        assert_eq!(task_status(TaskStatus::Cancelled), TaskStatusDto::Cancelled);
        assert_eq!(task_status(TaskStatus::Failed), TaskStatusDto::Failed);
    }

    #[test]
    fn budget_exceeded_dimension_is_snake_case_not_naive_lowercase() {
        let dto = stop_reason(&StopReason::BudgetExceeded(BudgetKind::WallClock));
        match dto {
            StopReasonDto::BudgetExceeded { dimension } => assert_eq!(dimension, "wall_clock"),
            other => panic!("expected BudgetExceeded, got {other:?}"),
        }
    }

    #[test]
    fn stop_reason_maps_all_variants() {
        assert_eq!(stop_reason(&StopReason::Completed), StopReasonDto::Completed);
        assert_eq!(stop_reason(&StopReason::Cancelled), StopReasonDto::Cancelled);
        assert_eq!(
            stop_reason(&StopReason::AwaitingApproval),
            StopReasonDto::AwaitingApproval
        );
        assert_eq!(
            stop_reason(&StopReason::Impossible("no can do".into())),
            StopReasonDto::Impossible {
                reason: "no can do".into()
            }
        );
        assert_eq!(
            stop_reason(&StopReason::Escalated("ask a human".into())),
            StopReasonDto::Escalated {
                reason: "ask a human".into()
            }
        );
    }

    #[test]
    fn budget_summary_sums_tokens_and_carries_coverage_and_bounds() {
        let case = sample_case();
        let dto = budget_summary(&case.usage, &case.bounds);
        assert_eq!(dto.llm_calls, 2);
        assert_eq!(dto.steps, 3);
        assert_eq!(dto.total_tokens, 150);
        assert_eq!(dto.cost_usd, Some(0.42));
        assert_eq!(dto.cost_coverage, "reported");
        assert_eq!(dto.wall_clock_ms, 1234);
        assert_eq!(dto.max_cost_usd, Some(5.0));
        assert_eq!(dto.max_steps, Some(20));
    }

    #[test]
    fn attempt_summary_excludes_verdict_detail_and_evidence() {
        let attempt = Attempt {
            id: AttemptId("attempt_1".into()),
            task: TaskCaseId("task_1".into()),
            started_at: 10,
            ended_at: Some(20),
            actions: vec![],
            belief_refs: vec![],
            usage: sample_usage(),
            verdict: Some(Verdict {
                attempt: AttemptId("attempt_1".into()),
                status: VerificationStatus::Succeeded,
                provenance: VerdictProvenance::Deterministic,
                evidence: vec![],
                detail: Some("secret internal detail".into()),
            }),
        };
        let dto = attempt_summary(&attempt);
        assert_eq!(dto.id, "attempt_1");
        assert_eq!(dto.verified, Some(AttemptVerificationDto::Succeeded));
        assert_eq!(dto.total_tokens, 150);
        // The DTO type has no field to carry `detail`/`provenance`/`evidence` —
        // this test exists so a future field addition to AttemptSummaryDto
        // must consciously decide whether to expose them, not do so by accident.
    }

    #[test]
    fn task_resource_external_ref_is_none_for_a_case_that_never_set_one() {
        let dto = task_resource(&sample_case(), vec![]);
        assert_eq!(dto.external_ref, None);
        assert_eq!(dto.id, "task_1");
        assert_eq!(dto.owner_id, "alice");
        assert_eq!(dto.revision, 3);
    }

    #[test]
    fn task_resource_recovers_external_ref_from_business_state() {
        let mut case = sample_case();
        case.business_state = OpaqueState(super::super::business_state::new_business_state(Some(
            "paperclip-issue-42",
        )));
        let dto = task_resource(&case, vec![]);
        assert_eq!(dto.external_ref.as_deref(), Some("paperclip-issue-42"));
    }
}
