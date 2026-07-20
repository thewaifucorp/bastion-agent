//! A concrete [`Observer`] that bridges the kernel's neutral task-lifecycle
//! events onto `tracing`.
//!
//! `AdaptiveCycle` requires an `Arc<dyn Observer>`; the kernel ships only a
//! no-op `NoObserver`. This emits one structured `tracing` event per
//! lifecycle event under the `bastion::task` target. The kernel already
//! guarantees the metadata is id/status-only (no prompt text or evidence
//! content), so forwarding it verbatim is egress-safe.

use bastion_runtime::hooks::Observer;

/// Forwards `TaskLifecycleEvent` records to `tracing` (target `bastion::task`).
#[derive(Debug, Clone, Default)]
pub struct TracingObserver;

#[async_trait::async_trait]
impl Observer for TracingObserver {
    async fn record(&self, event: &str, metadata: serde_json::Value) {
        tracing::info!(target: "bastion::task", event, metadata = %metadata);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn records_without_panicking() {
        let obs = TracingObserver;
        // id/status-only metadata, as the kernel produces it.
        obs.record(
            "task.terminal",
            serde_json::json!({"owner": "alice", "task": "t1", "status": "Completed"}),
        )
        .await;
    }
}
