//! US-206 — delegation: split a `Pursue` objective into independent child
//! tasks, run them concurrently (bounded), and complete/escalate the parent
//! on the aggregate.
//!
//! Builds on the Core `Orchestrator` (US-106: durable parent/child provenance,
//! cancel-cascade, aggregation) and the coding cycle (US-203). Deliberately
//! depth-1: children are never themselves decomposed. A small objective is
//! left whole — `decompose` returns `None` and the caller runs the single
//! coding cycle instead, so a one-liner never spawns a subagent.
//!
//! Only *independent* children run in parallel (the decomposition heuristic
//! splits on coordination words / list structure — it never claims a
//! dependency order it can't prove, so everything it emits is safe to run
//! concurrently). Concurrency is bounded by a semaphore; the parent's global
//! budget therefore covers the children (they share the same runtime pool).

use std::sync::Arc;

use bastion_runtime::agent::backend::RuntimeRegistry;
use bastion_runtime::task::{
    ChildSummary, CorrelationIds, ExecutionMode, Frame, Intent, IntentOrigin, OpaqueState,
    Orchestrator, StopReason, TaskCase, TaskCaseId, TaskStatus, TaskStore, UsageAccum,
};
use tokio::sync::Semaphore;

use super::exec::coding_cycle;
use super::schedule::now_nanos;

/// Default max children run at once when the parent sets no parallelism bound.
const DEFAULT_MAX_PARALLEL: usize = 3;

/// Split `objective` into independent child objectives, or `None` if it is a
/// single unit of work (US-206: a small scenario creates no subagent).
///
/// Deterministic and conservative: splits on explicit coordination (` and `,
/// `;`, newlines) or a leading `1. 2.`-style enumeration, trims, drops empties,
/// and only decomposes when it finds at least two substantial parts.
pub fn decompose(objective: &str) -> Option<Vec<String>> {
    let normalized = objective.replace(';', "\n").replace(" and then ", "\n");
    let mut parts: Vec<String> = normalized
        .split(['\n'])
        .flat_map(|line| line.split(" and "))
        .map(|p| strip_enumeration(p.trim()))
        .filter(|p| p.chars().count() >= 6)
        .collect();
    parts.dedup();
    if parts.len() >= 2 {
        Some(parts)
    } else {
        None
    }
}

/// Strip a leading `1.` / `2)` / `- ` list marker from a fragment.
fn strip_enumeration(s: &str) -> String {
    let trimmed = s.trim_start_matches(['-', '*', ' ']).trim_start();
    // drop a leading "<digits><.|)>" marker
    let after_num: String = trimmed
        .char_indices()
        .skip_while(|(_, c)| c.is_ascii_digit())
        .map(|(_, c)| c)
        .collect();
    let cleaned = after_num.trim_start_matches(['.', ')', ' ']).trim();
    if cleaned.is_empty() {
        trimmed.to_string()
    } else if cleaned.len() < trimmed.len() {
        cleaned.to_string()
    } else {
        trimmed.to_string()
    }
}

/// Build a Pending child `Pursue` case under `parent` for `objective`.
fn child_case(parent: &TaskCase, objective: &str, index: usize) -> TaskCase {
    let id = TaskCaseId(format!("{}-child-{index}", parent.id));
    TaskCase {
        id,
        owner: parent.owner.clone(),
        mode: ExecutionMode::Pursue,
        intent: Intent {
            owner: parent.owner.clone(),
            mode: ExecutionMode::Pursue,
            summary: objective.to_string(),
            origin: IntentOrigin::Message,
        },
        frame: Frame {
            objective: objective.to_string(),
            acceptance: vec![],
            context_refs: vec![],
        },
        bounds: parent.bounds,
        status: TaskStatus::Pending,
        stop_reason: None,
        attempts: vec![],
        pending_approvals: vec![],
        next_decision: None,
        usage: UsageAccum::default(),
        parent: Some(parent.id.clone()),
        correlation: CorrelationIds::default(),
        business_state: OpaqueState::default(),
        created_at: now_nanos(),
        updated_at: now_nanos(),
        revision: 1,
    }
}

/// Run `parent` by delegating `children_objectives` to independent child
/// tasks (US-206). Spawns each child under the Core `Orchestrator`, drives
/// their coding cycles concurrently up to `parent.bounds.max_parallelism`
/// (default [`DEFAULT_MAX_PARALLEL`]), then sets the parent terminal from the
/// aggregate: all children succeeded → `Completed`, otherwise `Escalated`
/// (each child's own outcome stays inspectable — divergence is preserved, not
/// hidden). Returns the child rollup.
pub async fn run_delegated(
    store: Arc<dyn TaskStore>,
    registry: RuntimeRegistry,
    parent: TaskCase,
    children_objectives: Vec<String>,
) -> anyhow::Result<ChildSummary> {
    let orch = Orchestrator::new(store.clone());
    let owner = parent.owner.clone();

    // Register every child up front (provenance), then drive them.
    let mut child_ids = Vec::with_capacity(children_objectives.len());
    for (i, obj) in children_objectives.iter().enumerate() {
        let child = child_case(&parent, obj, i);
        orch.spawn_child(&parent, &child, child.id.as_str()).await?;
        child_ids.push(child.id.clone());
    }

    let max_parallel = parent
        .bounds
        .max_parallelism
        .map(|n| n as usize)
        .unwrap_or(DEFAULT_MAX_PARALLEL)
        .max(1);
    let sem = Arc::new(Semaphore::new(max_parallel));

    let mut handles = Vec::with_capacity(child_ids.len());
    for child_id in child_ids {
        let permit_sem = sem.clone();
        let cycle = coding_cycle(&store, &registry, &owner);
        let owner_c = owner.clone();
        handles.push(tokio::spawn(async move {
            // Bound concurrency: only `max_parallel` children run at once.
            let _permit = permit_sem.acquire_owned().await;
            if let Err(e) = cycle.run(&owner_c, &child_id, None).await {
                tracing::error!(
                    event = "delegated_child_cycle_error",
                    child = %child_id,
                    error = %e,
                );
            }
        }));
    }
    for h in handles {
        let _ = h.await;
    }

    let summary = orch.summarize_children(&owner, &parent.id).await?;

    // Parent verdict on the aggregate. Re-load for the current revision.
    if let Some(current) = store.load_case(&owner, &parent.id).await? {
        if !current.status.is_terminal() {
            let (next, reason) = if summary.total > 0 && summary.failed == 0 {
                (TaskStatus::Completed, StopReason::Completed)
            } else {
                (
                    TaskStatus::Escalated,
                    StopReason::Escalated(format!(
                        "{}/{} children did not succeed",
                        summary.failed, summary.total
                    )),
                )
            };
            store
                .transition_status(&owner, &parent.id, next, Some(reason), current.revision)
                .await?;
        }
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bastion_runtime::task::Bounds;

    #[test]
    fn small_objective_is_not_decomposed() {
        assert!(decompose("fix the bug").is_none());
    }

    #[test]
    fn and_splits_into_children() {
        let parts = decompose("add a login page and write tests for it").expect("decompose");
        assert_eq!(parts.len(), 2);
        assert!(parts[0].contains("login page"));
        assert!(parts[1].contains("tests"));
    }

    #[test]
    fn enumeration_splits_and_strips_markers() {
        let parts =
            decompose("1. scaffold the API\n2. add the database\n3. wire the frontend").expect("d");
        assert_eq!(parts.len(), 3);
        assert!(parts[0].starts_with("scaffold"));
        assert!(parts[1].starts_with("add the database"));
    }

    #[test]
    fn child_case_links_to_parent() {
        let parent = TaskCase {
            id: TaskCaseId("p".into()),
            owner: "alice".into(),
            mode: ExecutionMode::Pursue,
            intent: Intent {
                owner: "alice".into(),
                mode: ExecutionMode::Pursue,
                summary: String::new(),
                origin: IntentOrigin::Message,
            },
            frame: Frame {
                objective: String::new(),
                acceptance: vec![],
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
        };
        let child = child_case(&parent, "do a thing", 0);
        assert_eq!(child.parent, Some(TaskCaseId("p".into())));
        assert_eq!(child.owner, "alice");
        assert_eq!(child.status, TaskStatus::Pending);
        assert_eq!(child.frame.objective, "do a thing");
    }
}
