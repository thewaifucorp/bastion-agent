//! US-202 — the task cockpit: inspect and control durable `Pursue` tasks.
//!
//! A single owner-scoped query/control layer over the `TaskStore`, reused by
//! every surface (console, webhook/channel) so authorization is never
//! duplicated. Minimum cut: `list`, `inspect`, `pause`, `resume`, `steer`,
//! `cancel`. It needs the store, not `&mut AgentLoop`, so — like
//! `backend_command` — it is special-cased in the daemon dispatch rather than
//! routed through the generic `CommandHandler` port (which has no store).

use std::sync::Arc;

use bastion_runtime::task::{StopReason, TaskCase, TaskCaseId, TaskStatus, TaskStore};

/// Handle `/task <sub> [args]`. `arg` is everything after `/task`.
pub async fn handle(
    store: &Arc<dyn TaskStore>,
    arg: Option<&str>,
    owner: &str,
) -> anyhow::Result<String> {
    let arg = arg.unwrap_or("").trim();
    let (sub, rest) = match arg.split_once(char::is_whitespace) {
        Some((s, r)) => (s, r.trim()),
        None => (arg, ""),
    };

    match sub {
        "" | "list" => list(store, owner).await,
        "inspect" => inspect(store, owner, rest).await,
        "pause" => transition(store, owner, rest, TaskStatus::Paused, None, "paused").await,
        "resume" => transition(store, owner, rest, TaskStatus::Running, None, "resumed").await,
        "cancel" => {
            transition(
                store,
                owner,
                rest,
                TaskStatus::Cancelled,
                Some(StopReason::Cancelled),
                "cancelled",
            )
            .await
        }
        "steer" => steer(store, owner, rest).await,
        other => Ok(format!(
            "unknown /task subcommand '{other}'. Use: list | inspect <id> | pause <id> | \
             resume <id> | steer <id> <text> | cancel <id>"
        )),
    }
}

async fn list(store: &Arc<dyn TaskStore>, owner: &str) -> anyhow::Result<String> {
    let cases = store.list_cases_for_owner(owner).await?;
    if cases.is_empty() {
        return Ok("no tasks.".to_string());
    }
    let mut out = String::from("tasks (newest first):\n");
    for c in &cases {
        out.push_str(&format!(
            "  {}  [{:?}/{:?}]  attempts={}  {}\n",
            c.id,
            c.mode,
            c.status,
            c.attempts.len(),
            one_line(&c.frame.objective, 60),
        ));
    }
    Ok(out.trim_end().to_string())
}

async fn inspect(store: &Arc<dyn TaskStore>, owner: &str, id: &str) -> anyhow::Result<String> {
    if id.is_empty() {
        return Ok("usage: /task inspect <id>".to_string());
    }
    let case = match store.load_case(owner, &TaskCaseId(id.to_string())).await? {
        Some(c) => c,
        None => return Ok(format!("task {id} not found.")),
    };
    Ok(render_case(&case))
}

fn render_case(c: &TaskCase) -> String {
    let mut s = String::new();
    s.push_str(&format!("task {}\n", c.id));
    s.push_str(&format!("  mode:      {:?}\n", c.mode));
    s.push_str(&format!("  status:    {:?}\n", c.status));
    if let Some(r) = &c.stop_reason {
        s.push_str(&format!("  stop:      {r:?}\n"));
    }
    s.push_str(&format!("  objective: {}\n", c.frame.objective));
    s.push_str(&format!(
        "  usage:     {} llm calls, {} steps, {} in + {} out tokens\n",
        c.usage.llm_calls, c.usage.steps, c.usage.input_tokens, c.usage.output_tokens
    ));
    s.push_str(&format!("  attempts:  {}\n", c.attempts.len()));
    if !c.pending_approvals.is_empty() {
        s.push_str(&format!(
            "  approvals: {} pending\n",
            c.pending_approvals.len()
        ));
    }
    s.push_str(&format!("  revision:  {}", c.revision));
    s
}

async fn transition(
    store: &Arc<dyn TaskStore>,
    owner: &str,
    id: &str,
    next: TaskStatus,
    stop_reason: Option<StopReason>,
    verb: &str,
) -> anyhow::Result<String> {
    if id.is_empty() {
        return Ok(format!("usage: /task {verb} <id>"));
    }
    let case = match store.load_case(owner, &TaskCaseId(id.to_string())).await? {
        Some(c) => c,
        None => return Ok(format!("task {id} not found.")),
    };
    match store
        .transition_status(owner, &case.id, next, stop_reason, case.revision)
        .await
    {
        Ok(_) => Ok(format!("task {id} {verb}.")),
        // Invalid transition / conflict: surface the store's typed reason.
        Err(e) => Ok(format!("cannot {verb} task {id}: {e}")),
    }
}

async fn steer(store: &Arc<dyn TaskStore>, owner: &str, rest: &str) -> anyhow::Result<String> {
    let (id, text) = match rest.split_once(char::is_whitespace) {
        Some((i, t)) if !t.trim().is_empty() => (i, t.trim()),
        _ => return Ok("usage: /task steer <id> <text>".to_string()),
    };
    let mut case = match store.load_case(owner, &TaskCaseId(id.to_string())).await? {
        Some(c) => c,
        None => return Ok(format!("task {id} not found.")),
    };
    if case.status.is_terminal() {
        return Ok(format!("task {id} is {:?}; cannot steer.", case.status));
    }
    // Steering is host-owned guidance: append to the opaque business_state as a
    // list of notes the chooser can read. The kernel never interprets it.
    let mut notes = match case.business_state.0.take() {
        serde_json::Value::Array(a) => a,
        serde_json::Value::Null => Vec::new(),
        other => vec![other],
    };
    notes.push(serde_json::json!({ "steer": text }));
    case.business_state.0 = serde_json::Value::Array(notes);
    let rev = case.revision;
    store.update_case(&case, rev).await?;
    Ok(format!("task {id} steered."))
}

/// Collapse an objective to a single line, truncated to `max` chars.
fn one_line(s: &str, max: usize) -> String {
    let flat: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > max {
        let truncated: String = flat.chars().take(max.saturating_sub(1)).collect();
        format!("{truncated}…")
    } else {
        flat
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bastion_runtime::task::{
        Bounds, CorrelationIds, ExecutionMode, Frame, Intent, IntentOrigin, OpaqueState,
        SqliteTaskStore, UsageAccum,
    };
    use tempfile::NamedTempFile;

    async fn store_with_case() -> (NamedTempFile, Arc<dyn TaskStore>, TaskCaseId) {
        let f = NamedTempFile::new().unwrap();
        let c = SqliteTaskStore::new(f.path().to_str().unwrap());
        c.init_schema().await.unwrap();
        let store: Arc<dyn TaskStore> = Arc::new(c);
        let id = TaskCaseId("t1".into());
        let case = TaskCase {
            id: id.clone(),
            owner: "alice".into(),
            mode: ExecutionMode::Pursue,
            intent: Intent {
                owner: "alice".into(),
                mode: ExecutionMode::Pursue,
                summary: "s".into(),
                origin: IntentOrigin::Message,
            },
            frame: Frame {
                objective: "ship the feature".into(),
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
        store.create_case(&case, "k").await.unwrap();
        (f, store, id)
    }

    #[tokio::test]
    async fn list_and_inspect() {
        let (_f, store, _id) = store_with_case().await;
        let listed = handle(&store, Some("list"), "alice").await.unwrap();
        assert!(listed.contains("t1"));
        let inspected = handle(&store, Some("inspect t1"), "alice").await.unwrap();
        assert!(inspected.contains("ship the feature"));
        assert!(inspected.contains("Running"));
    }

    #[tokio::test]
    async fn cancel_transitions_and_is_owner_scoped() {
        let (_f, store, _id) = store_with_case().await;
        // wrong owner cannot see or cancel it
        let miss = handle(&store, Some("cancel t1"), "bob").await.unwrap();
        assert!(miss.contains("not found"));
        let ok = handle(&store, Some("cancel t1"), "alice").await.unwrap();
        assert!(ok.contains("cancelled"));
        // second cancel is rejected (already terminal)
        let again = handle(&store, Some("cancel t1"), "alice").await.unwrap();
        assert!(again.contains("cannot cancel"));
    }

    #[tokio::test]
    async fn steer_appends_note() {
        let (_f, store, id) = store_with_case().await;
        let out = handle(&store, Some("steer t1 focus on the tests"), "alice")
            .await
            .unwrap();
        assert!(out.contains("steered"));
        let case = store.load_case("alice", &id).await.unwrap().unwrap();
        let notes = case.business_state.0.as_array().expect("array");
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0]["steer"], "focus on the tests");
    }
}
