//! Persist a `Pursue` request as a durable, Pending `TaskCase` (US-201).
//!
//! When the mode selector picks `Pursue`, the daemon enqueues a durable task
//! here. It starts `Pending` — a resumable queue entry the adaptive executor
//! drains later (US-203/206); persisting it is what lets a complex objective
//! survive restart and be inspected/steered from the cockpit (US-202). A
//! `Respond` or `Act` request never reaches this path, so simple messages
//! never pay for a durable lifecycle (Gate A).

use bastion_memory::SharedMemory;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use bastion_runtime::task::{
    Bounds, CorrelationIds, ExecutionMode, Frame, Intent, IntentOrigin, OpaqueState, TaskCase,
    TaskCaseId, TaskStatus, TaskStore, UsageAccum,
};

fn now_nanos() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as i64
}

/// Enqueue `request` as a Pending `Pursue` task for `owner`. Returns the new
/// task id. `reason` is the selector's explanation, recorded for inspection.
pub async fn enqueue_pursue(
    store: &Arc<dyn TaskStore>,
    owner: &str,
    memory: &SharedMemory,
    request: &str,
    reason: &str,
) -> anyhow::Result<TaskCaseId> {
    let id = TaskCaseId(format!("pursue-{}", now_nanos()));
    let case = TaskCase {
        id: id.clone(),
        owner: owner.to_string(),
        mode: ExecutionMode::Pursue,
        intent: Intent {
            owner: owner.to_string(),
            mode: ExecutionMode::Pursue,
            summary: request.to_string(),
            origin: IntentOrigin::Message,
        },
        frame: Frame {
            objective: request.to_string(),
            acceptance: vec![],
            context_refs: vec![],
        },
        bounds: Bounds::default(),
        status: TaskStatus::Pending,
        stop_reason: None,
        attempts: vec![],
        pending_approvals: vec![],
        next_decision: None,
        usage: UsageAccum::default(),
        parent: None,
        correlation: CorrelationIds::default(),
        business_state: OpaqueState(super::state_for_pursue(memory, owner, request).await),
        created_at: 0,
        updated_at: 0,
        revision: 1,
    };
    // Idempotency key is unique per enqueue (each interactive request is its
    // own task); scheduled/dedup-sensitive callers supply their own later.
    store.create_case(&case, id.as_str()).await?;
    tracing::info!(
        target: "bastion::task",
        event = "pursue_enqueued",
        task = %id,
        owner = owner,
        reason = reason,
    );
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bastion_memory::{sqlite::SqliteMemory, Memory};
    use bastion_runtime::task::SqliteTaskStore;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn enqueue_persists_a_pending_pursue_case() {
        let f = NamedTempFile::new().unwrap();
        let concrete = SqliteTaskStore::new(f.path().to_str().unwrap());
        concrete.init_schema().await.unwrap();
        let store: Arc<dyn TaskStore> = Arc::new(concrete);
        let memory: SharedMemory = Arc::new(tokio::sync::RwLock::new(Box::new(SqliteMemory::new(
            f.path().to_str().unwrap(),
        ))
            as Box<dyn Memory>));
        let id = enqueue_pursue(&store, "alice", &memory, "build me a weather app", "cue")
            .await
            .expect("enqueue");
        let loaded = store
            .load_case("alice", &id)
            .await
            .unwrap()
            .expect("exists");
        assert_eq!(loaded.status, TaskStatus::Pending);
        assert_eq!(loaded.mode, ExecutionMode::Pursue);
        assert_eq!(loaded.frame.objective, "build me a weather app");
        assert!(store.load_case("bob", &id).await.unwrap().is_none());
    }
}
