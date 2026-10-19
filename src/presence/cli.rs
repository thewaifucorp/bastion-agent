//! `bastion presence …` — the presence contract from a terminal: what is
//! running, answering approvals, choosing the persona, pairing devices.
//! Same daemon discovery and token as `bastion hook` (`hook::Daemon`).

use reqwest::Method;
use serde_json::{json, Value};

use super::hook::Daemon;

fn ago(now: u64, since: u64) -> String {
    let secs = now.saturating_sub(since);
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m", secs / 60),
        _ => format!("{}h", secs / 3600),
    }
}

/// The snapshot as a few readable lines.
pub fn render_status(snapshot: &Value) -> String {
    let now = snapshot["now"].as_u64().unwrap_or(0);
    let mut out = format!(
        "Bastion on {} · persona {}\n",
        snapshot["host"].as_str().unwrap_or("?"),
        snapshot["persona"].as_str().unwrap_or("(default)")
    );
    let alerts = snapshot["alerts"].as_array().cloned().unwrap_or_default();
    for alert in &alerts {
        // An approval shows the whole call, never only the display cut.
        let what = format!(
            "{} {}",
            alert["tool"].as_str().unwrap_or(""),
            alert["input"]
                .as_str()
                .or_else(|| alert["det"].as_str())
                .unwrap_or("")
        );
        match alert["kind"].as_str() {
            Some("approval") if alert["input_complete"] != Value::Bool(true) => out.push_str(&format!(
                "  ! approval {} — {} … (too long to show whole: deny {} or answer it in the agent)\n",
                alert["id"].as_str().unwrap_or("?"),
                what.trim(),
                alert["id"].as_str().unwrap_or("?"),
            )),
            Some("approval") => out.push_str(&format!(
                "  ! approval {} — {}  (bastion presence approve {} | deny {})\n",
                alert["id"].as_str().unwrap_or("?"),
                what.trim(),
                alert["id"].as_str().unwrap_or("?"),
                alert["id"].as_str().unwrap_or("?"),
            )),
            Some("question") => {
                out.push_str(&format!("  ? waiting at the computer — {}\n", what.trim()))
            }
            Some("error") => out.push_str("  x a turn stopped with an error\n"),
            _ => {}
        }
    }
    let sessions = snapshot["sessions"].as_array().cloned().unwrap_or_default();
    if sessions.is_empty() {
        out.push_str("  no agent sessions\n");
    }
    for s in &sessions {
        let mut line = format!(
            "  {:<8} {:<11} {:<24} {:>4}",
            s["st"].as_str().unwrap_or("?"),
            s["src"].as_str().unwrap_or("?"),
            s["name"].as_str().unwrap_or("?"),
            ago(now, s["since"].as_u64().unwrap_or(now)),
        );
        if let Some(tool) = s["tool"].as_str() {
            line.push_str(&format!("  {tool} {}", s["det"].as_str().unwrap_or("")));
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    if let Some(more) = snapshot["more"].as_u64().filter(|m| *m > 0) {
        out.push_str(&format!("  +{more} more\n"));
    }
    let today = &snapshot["today"];
    out.push_str(&format!(
        "Today: {} turns · {} tool calls · {} errors",
        today["turns"].as_u64().unwrap_or(0),
        today["tools"].as_u64().unwrap_or(0),
        today["errors"].as_u64().unwrap_or(0)
    ));
    out
}

pub async fn status() -> anyhow::Result<String> {
    let snapshot = Daemon::discover()
        .call(Method::GET, "/presence/snapshot", None)
        .await?;
    Ok(render_status(&snapshot))
}

pub async fn persona(name: Option<&str>) -> anyhow::Result<String> {
    let body = json!({ "action": "persona", "persona": name });
    let snapshot = Daemon::discover()
        .call(Method::POST, "/presence/action", Some(&body))
        .await?;
    Ok(format!(
        "persona: {}",
        snapshot["persona"].as_str().unwrap_or("(default)")
    ))
}

pub async fn answer(id: &str, approve: bool) -> anyhow::Result<String> {
    let action = if approve { "approve" } else { "deny" };
    let body = json!({ "action": action, "id": id });
    Daemon::discover()
        .call(Method::POST, "/presence/action", Some(&body))
        .await?;
    Ok(format!(
        "{id}: {}",
        if approve { "approved" } else { "denied" }
    ))
}

pub async fn pair(addr: &str, code: &str, name: Option<&str>) -> anyhow::Result<String> {
    let body = json!({ "addr": addr, "code": code, "name": name });
    let answer = Daemon::discover()
        .call(Method::POST, "/presence/devices/pair", Some(&body))
        .await?;
    Ok(format!(
        "paired {} — it reaches Bastion at {}",
        answer["device"]["name"].as_str().unwrap_or("device"),
        answer["bastion_url"].as_str().unwrap_or("?")
    ))
}

pub async fn devices() -> anyhow::Result<String> {
    let answer = Daemon::discover()
        .call(Method::GET, "/presence/devices", None)
        .await?;
    let list = answer["devices"].as_array().cloned().unwrap_or_default();
    if list.is_empty() {
        return Ok("no paired devices — `bastion presence pair <ip> <code>`".to_string());
    }
    Ok(list
        .iter()
        .map(|d| {
            format!(
                "{}  {}  {}{}",
                d["id"].as_str().unwrap_or("?"),
                d["name"].as_str().unwrap_or("?"),
                d["addr"].as_str().unwrap_or("?"),
                if d["caps"]["touch"].as_bool() == Some(true) {
                    "  (touch)"
                } else {
                    ""
                }
            )
        })
        .collect::<Vec<_>>()
        .join("\n"))
}

pub async fn unpair(id: &str) -> anyhow::Result<String> {
    Daemon::discover()
        .call(Method::DELETE, &format!("/presence/devices/{id}"), None)
        .await?;
    Ok(format!("unpaired {id}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_lists_approvals_first_with_the_commands_to_answer_them() {
        let text = render_status(&json!({
            "now": 1000, "host": "box", "persona": "ada",
            "sessions": [{ "id": "a1", "src": "claude-code", "name": "demo", "st": "waiting", "since": 940, "tool": "Bash", "det": "git push" }],
            "more": 2,
            "alerts": [{ "id": "ap-3", "kind": "approval", "sid": "a1", "tool": "Bash", "det": "git push", "input_complete": true }],
            "today": { "turns": 4, "tools": 9, "errors": 0 }
        }));
        assert!(text.starts_with("Bastion on box · persona ada"));
        assert!(text.contains("bastion presence approve ap-3"));
        assert!(text.contains("1m"));
        assert!(text.contains("+2 more"));
        assert!(text.ends_with("Today: 4 turns · 9 tool calls · 0 errors"));
    }
}
