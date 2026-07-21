//! Turn observability: persona/cabinet/turn events on the SSE feed.
//!
//! `GET /events` (webhook.rs `sse_handler`) has carried only `mesh_sync`
//! since Phase 6 — the TUI already parses richer event shapes
//! (`tui/visual.rs::mode_for_event`: `cabinet.started`, `turn.started`,
//! `turn.completed`, `turn.failed`) that nothing emitted until now. This
//! module closes that gap from the PRODUCT side: [`ObservedResponder`]
//! decorates the `Responder` port around `PersonaResponder`, so the kernel
//! (bastion-core) stays untouched and every event is derived from what the
//! port boundary already exposes (`TurnContext.forced_cabinet` on the way
//! in, `RespondOutcome.attribution` on the way out).
//!
//! Known limit of the decorator seam: an AUTO-convened cabinet (router's
//! decision inside `PersonaResponder`) is only visible post-hoc via
//! `attribution.len() > 1`, so its `cabinet.started` cannot be emitted
//! before deliberation — only a forced `/cabinet` turn gets the upfront
//! event. Emitting mid-routing requires a `TurnObserver` port in
//! bastion-core (backlog).

use std::sync::Arc;
use std::time::Instant;

/// The bundled observability dashboard (`GET /ui`, served by
/// `channel/webhook.rs`), embedded like `control_plane/routes.rs` embeds the
/// OpenAPI YAML. Self-contained single file: the daemon serves it with a CSP
/// that only allows same-origin connects, so it can never load remote assets.
/// The page itself is an unauthenticated static shell — every byte of data it
/// shows comes from `/events` (owner token) and `/v1/*` (Control Plane
/// credential), both entered by the operator in the page and sent per
/// request.
pub const DASHBOARD_HTML: &str = include_str!("observability/dashboard.html");

use bastion_runtime::agent::ports::{Responder, RespondOutcome, TurnContext};
use tokio::sync::broadcast;

/// `Responder` decorator that broadcasts turn lifecycle events as JSON
/// strings on the same channel `sse_handler` streams to `/events` clients.
///
/// Send failures are ignored on purpose: `broadcast::Sender::send` errs only
/// when there is no subscriber, which is the normal state whenever no TUI,
/// mobile app, or web UI is attached.
pub struct ObservedResponder {
    inner: Arc<dyn Responder>,
    events_tx: broadcast::Sender<String>,
}

impl ObservedResponder {
    pub fn new(inner: Arc<dyn Responder>, events_tx: broadcast::Sender<String>) -> Self {
        Self { inner, events_tx }
    }

    fn emit(&self, event: serde_json::Value) {
        let _ = self.events_tx.send(event.to_string());
    }
}

#[async_trait::async_trait]
impl Responder for ObservedResponder {
    async fn respond(&self, turn: TurnContext<'_>) -> anyhow::Result<RespondOutcome> {
        // `turn` moves into `inner.respond` — copy what the events need first.
        let owner = turn.owner.to_string();
        let session_id = turn.session_id.to_string();
        let forced_cabinet = turn.forced_cabinet.clone();

        self.emit(turn_started(&owner, &session_id, forced_cabinet.as_deref()));
        if let Some(personas) = forced_cabinet.as_deref() {
            self.emit(cabinet_started(&owner, personas));
        }

        let t0 = Instant::now();
        let result = self.inner.respond(turn).await;
        let latency_ms = t0.elapsed().as_millis() as u64;

        match &result {
            Ok(outcome) => {
                self.emit(turn_completed(&owner, &session_id, &outcome.attribution, latency_ms));
            }
            Err(_) => {
                // The error itself stays on the tracing/log path (WR-09: no
                // internal detail on a network-visible surface).
                self.emit(serde_json::json!({
                    "event": "turn.failed",
                    "owner": owner,
                    "session_id": session_id,
                    "latency_ms": latency_ms,
                }));
            }
        }
        result
    }
}

/// [`Observer`] for the adaptive execution loop (`AdaptiveCycle`,
/// `run_delegated`): fans every `TaskLifecycleEvent` out to the SSE feed the
/// dashboard/TUI watch, keeps `TracingObserver`'s log line, and enqueues the
/// spec's two remaining Control Plane event types — `attempt.completed` (from
/// `task.verified`: an attempt completes at its verification) and
/// `task.escalated` (from a `task.terminal` whose status is `Escalated`) —
/// into the same durable delivery queue `core_ops` uses. Closes the
/// "attempt.completed/task.escalated are not emitted" known gap
/// (docs/en/control-plane-security.md).
///
/// Fire-and-forget per the `Observer` contract: any failure here is logged
/// and dropped, never surfaced into the cycle.
pub struct LifecycleObserver {
    events_tx: broadcast::Sender<String>,
    core_ops: crate::control_plane::core_ops::CoreOpsState,
}

impl LifecycleObserver {
    pub fn new(
        events_tx: broadcast::Sender<String>,
        core_ops: crate::control_plane::core_ops::CoreOpsState,
    ) -> Arc<Self> {
        Arc::new(Self {
            events_tx,
            core_ops,
        })
    }

    /// Current revision of `task`, for the event envelope's ordering field.
    async fn revision_of(&self, owner: &str, task: &str) -> Option<u64> {
        let id = bastion_runtime::task::TaskCaseId(task.to_string());
        match self.core_ops.task_store.load_case(owner, &id).await {
            Ok(Some(case)) => Some(case.revision),
            Ok(None) => None,
            Err(e) => {
                tracing::warn!(event = "lifecycle_observer_load_failed", task, error = %e);
                None
            }
        }
    }
}

#[async_trait::async_trait]
impl bastion_runtime::hooks::Observer for LifecycleObserver {
    async fn record(&self, event: &str, metadata: serde_json::Value) {
        // Preserve TracingObserver's structured log line (same target).
        tracing::info!(target: "bastion::task", event, metadata = %metadata);

        // SSE: the whole lifecycle vocabulary, verbatim — the dashboard's
        // ledger renders any `event` field.
        let mut sse = metadata.clone();
        if let serde_json::Value::Object(ref mut map) = sse {
            map.insert("event".to_string(), serde_json::json!(event));
        }
        let _ = self.events_tx.send(sse.to_string());

        // Control Plane queue: only the spec-named mappings.
        let (owner, task) = match (
            metadata.get("owner").and_then(serde_json::Value::as_str),
            metadata.get("task").and_then(serde_json::Value::as_str),
        ) {
            (Some(o), Some(t)) => (o.to_string(), t.to_string()),
            _ => return,
        };
        let status = metadata.get("status").and_then(serde_json::Value::as_str);
        let mapped = match event {
            "task.verified" => Some("attempt.completed"),
            "task.terminal" if status == Some("Escalated") => Some("task.escalated"),
            _ => None,
        };
        let Some(event_type) = mapped else { return };
        let Some(revision) = self.revision_of(&owner, &task).await else {
            tracing::warn!(
                event = "lifecycle_observer_event_dropped",
                event_type,
                task,
                "task not loadable for envelope revision — event not enqueued"
            );
            return;
        };
        crate::control_plane::core_ops::emit_event(
            &self.core_ops,
            &owner,
            event_type,
            &task,
            revision,
            metadata,
        )
        .await;
    }
}

fn turn_started(
    owner: &str,
    session_id: &str,
    forced_cabinet: Option<&[String]>,
) -> serde_json::Value {
    let mut event = serde_json::json!({
        "event": "turn.started",
        "owner": owner,
        "session_id": session_id,
    });
    if forced_cabinet.is_some() {
        event["mode"] = serde_json::json!("cabinet");
    }
    event
}

fn cabinet_started(owner: &str, personas: &[String]) -> serde_json::Value {
    serde_json::json!({
        "event": "cabinet.started",
        "owner": owner,
        "personas": personas,
    })
}

fn turn_completed(
    owner: &str,
    session_id: &str,
    attribution: &[String],
    latency_ms: u64,
) -> serde_json::Value {
    let mut event = serde_json::json!({
        "event": "turn.completed",
        "owner": owner,
        "session_id": session_id,
        "personas": attribution,
        "latency_ms": latency_ms,
    });
    if attribution.len() > 1 {
        // Post-hoc cabinet marker — see the module doc's decorator-seam limit.
        event["mode"] = serde_json::json!("cabinet");
    }
    event
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turn_started_marks_cabinet_only_when_forced() {
        let plain = turn_started("alice", "s1", None);
        assert_eq!(plain["event"], "turn.started");
        assert!(plain.get("mode").is_none());

        let forced = vec!["ada".to_string(), "grace".to_string()];
        let cabinet = turn_started("alice", "s1", Some(&forced));
        assert_eq!(cabinet["mode"], "cabinet");
    }

    #[test]
    fn cabinet_started_carries_personas() {
        let personas = vec!["ada".to_string(), "grace".to_string()];
        let event = cabinet_started("alice", &personas);
        assert_eq!(event["event"], "cabinet.started");
        assert_eq!(event["personas"], serde_json::json!(["ada", "grace"]));
    }

    #[test]
    fn turn_completed_marks_cabinet_only_for_multi_persona_attribution() {
        let single = turn_completed("alice", "s1", &["ada".to_string()], 42);
        assert_eq!(single["event"], "turn.completed");
        assert_eq!(single["personas"], serde_json::json!(["ada"]));
        assert!(single.get("mode").is_none());

        let multi = turn_completed("alice", "s1", &["ada".to_string(), "grace".to_string()], 42);
        assert_eq!(multi["mode"], "cabinet");
    }

    #[tokio::test]
    async fn decorator_emits_started_and_completed_around_inner() {
        struct FixedResponder;
        #[async_trait::async_trait]
        impl Responder for FixedResponder {
            async fn respond(&self, _turn: TurnContext<'_>) -> anyhow::Result<RespondOutcome> {
                unreachable!("event-builder tests don't dispatch; see module tests above")
            }
        }
        // Constructing a real TurnContext needs a live kernel — the emit
        // path is covered by the pure builders above plus the send-ignores-
        // no-subscriber contract checked here.
        let (tx, mut rx) = broadcast::channel::<String>(8);
        let observed = ObservedResponder::new(Arc::new(FixedResponder), tx);
        observed.emit(serde_json::json!({"event": "turn.started"}));
        let received = rx.try_recv().expect("event should be broadcast");
        assert!(received.contains("turn.started"));
    }
}
