//! Presence in the TUI: what host agents (Claude Code in Bastion mode, …)
//! are doing and the approvals waiting for the person, read from
//! `GET /presence/snapshot` (`docs/en/presence.md`). Pure functions over the
//! snapshot JSON; `tui.rs` does the fetching, the keys and the drawing.

use std::collections::HashSet;

use serde_json::Value;

/// Whether an `/events` line is a presence change (the TUI then refetches
/// the snapshot instead of printing the raw event).
pub(super) fn is_presence_event(event: &str) -> bool {
    serde_json::from_str::<Value>(event)
        .ok()
        .and_then(|v| v.get("event").and_then(Value::as_str).map(str::to_string))
        .is_some_and(|name| name.starts_with("presence.") || name.starts_with("approval."))
}

fn alerts<'a>(snapshot: &'a Value, kind: &'a str) -> impl Iterator<Item = &'a Value> + 'a {
    snapshot["alerts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(move |a| a["kind"] == kind)
}

/// The approval the keys act on: the first (soonest to expire) pending one.
pub(super) fn first_approval(snapshot: &Value) -> Option<&Value> {
    alerts(snapshot, "approval").next()
}

/// The call as the person must see it before approving: the whole input
/// when the daemon sent it, else the display cut.
fn call_text(alert: &Value) -> String {
    let tool = alert["tool"].as_str().unwrap_or("");
    let input = alert["input"]
        .as_str()
        .or_else(|| alert["det"].as_str())
        .unwrap_or("");
    format!("{tool} {input}").trim().to_string()
}

/// Whether the approval can be answered from the bar: only when the daemon
/// says its `input` is the whole call.
pub(super) fn approvable_here(alert: &Value) -> bool {
    alert["input_complete"] == Value::Bool(true)
}

/// The highlighted bar shown while an approval waits, or `None`.
pub(super) fn approval_bar(snapshot: &Value) -> Option<String> {
    let alert = first_approval(snapshot)?;
    let more = alerts(snapshot, "approval").count().saturating_sub(1);
    let more = if more > 0 {
        format!(" (+{more} more: /agents)")
    } else {
        String::new()
    };
    let keys = if approvable_here(alert) {
        "Ctrl+Y approve · Ctrl+N deny"
    } else {
        "call too long to show whole — review in the web app · Ctrl+N deny"
    };
    Some(format!("⏳ Approval: {} — {keys}{more}", call_text(alert)))
}

/// A short "what agents are doing" piece for the companion status line.
pub(super) fn summary(snapshot: &Value) -> Option<String> {
    let sessions = snapshot["sessions"].as_array()?;
    if sessions.is_empty() {
        return None;
    }
    let count = |st: &str| sessions.iter().filter(|s| s["st"] == st).count();
    let (working, waiting) = (count("working"), count("waiting"));
    let mut parts = vec![format!(
        "{} agent{}",
        sessions.len(),
        if sessions.len() == 1 { "" } else { "s" }
    )];
    if working > 0 {
        parts.push(format!("{working} working"));
    }
    if waiting > 0 {
        parts.push(format!("{waiting} waiting"));
    }
    Some(parts.join(" · "))
}

/// Transcript notices for approvals that appeared or left since the last
/// snapshot; updates `seen` (the pending approval ids).
pub(super) fn notices(seen: &mut HashSet<String>, snapshot: &Value) -> Vec<String> {
    let now: HashSet<String> = alerts(snapshot, "approval")
        .filter_map(|a| a["id"].as_str().map(str::to_string))
        .collect();
    let mut out = Vec::new();
    for alert in alerts(snapshot, "approval") {
        if let Some(id) = alert["id"].as_str() {
            if !seen.contains(id) {
                out.push(format!("⏳ {id} waits for you: {}", call_text(alert)));
            }
        }
    }
    let mut gone: Vec<&String> = seen.difference(&now).collect();
    gone.sort();
    for id in gone {
        out.push(format!("✔ {id} answered"));
    }
    *seen = now;
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn snapshot(alerts: Value) -> Value {
        json!({
            "sessions": [
                { "id": "a", "src": "claude-code", "name": "demo", "st": "waiting" },
                { "id": "b", "src": "claude-code", "name": "api", "st": "working" }
            ],
            "alerts": alerts,
        })
    }

    #[test]
    fn only_presence_and_approval_events_trigger_a_refetch() {
        assert!(is_presence_event(r#"{"event":"presence.updated","seq":3}"#));
        assert!(is_presence_event(
            r#"{"event":"approval.requested","id":"ap-1"}"#
        ));
        assert!(!is_presence_event(r#"{"event":"turn.started"}"#));
        assert!(!is_presence_event("not json"));
    }

    #[test]
    fn the_bar_shows_the_whole_call_and_offers_keys_only_when_complete() {
        let s = snapshot(json!([
            { "id": "ap-1", "kind": "approval", "tool": "Bash", "det": "git push…",
              "input": "git push origin main", "input_complete": true },
            { "id": "ap-2", "kind": "approval", "tool": "Bash", "det": "rm" }
        ]));
        let bar = approval_bar(&s).unwrap();
        assert!(bar.contains("Bash git push origin main"));
        assert!(bar.contains("Ctrl+Y approve"));
        assert!(bar.contains("+1 more"));
        let cut =
            snapshot(json!([{ "id": "ap-3", "kind": "approval", "tool": "Bash", "det": "x" }]));
        let bar = approval_bar(&cut).unwrap();
        assert!(!bar.contains("Ctrl+Y"), "no blind approve: {bar}");
        assert!(approval_bar(&snapshot(json!([]))).is_none());
    }

    #[test]
    fn summary_counts_agents_by_state() {
        assert_eq!(
            summary(&snapshot(json!([]))).unwrap(),
            "2 agents · 1 working · 1 waiting"
        );
        assert!(summary(&json!({ "sessions": [] })).is_none());
    }

    #[test]
    fn notices_report_new_and_answered_approvals_once() {
        let mut seen = HashSet::new();
        let s =
            snapshot(json!([{ "id": "ap-1", "kind": "approval", "tool": "Bash", "det": "ls" }]));
        assert_eq!(
            notices(&mut seen, &s),
            vec!["⏳ ap-1 waits for you: Bash ls"]
        );
        assert!(notices(&mut seen, &s).is_empty());
        assert_eq!(
            notices(&mut seen, &snapshot(json!([]))),
            vec!["✔ ap-1 answered"]
        );
    }
}
