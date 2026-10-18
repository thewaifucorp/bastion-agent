//! `bastion hook <host>` — what a host agent's hooks run to be this Bastion.
//!
//! One short-lived process per hook: read the host's payload on stdin, talk
//! to the local daemon's `/presence/*` routes, print the host's hook answer
//! on stdout. It must never delay or break the host, so:
//! - it always exits 0 and prints nothing but a valid hook answer;
//! - an unreachable daemon, a bad payload or a failed call is silent (set
//!   `BASTION_HOOK_DEBUG=1` to see why on stderr) and the host carries on
//!   exactly as without the pack;
//! - only the whitelisted tool-input fields leave the host
//!   (`policy::sanitize_tool_input`), and prompt/reply text goes only to the
//!   local journal, cut short.
//!
//! The daemon is `BASTION_URL` (default `http://127.0.0.1:$BASTION_HTTP_PORT`,
//! port 8080), authenticated with `BASTION_TOKEN`, else the local bootstrap
//! token, else the session `bastion chat` saved.

use std::io::{Read, Seek, SeekFrom};
use std::time::Duration;

use serde_json::{json, Value};

use super::policy::{cut, sanitize_tool_input, select_tool_input, RULE_INPUT_MAX};

const SOURCE_CLAUDE_CODE: &str = "claude-code";
const TRANSCRIPT_TAIL_BYTES: u64 = 512 * 1024;
const JOURNAL_TEXT_CHARS: usize = 600;

fn debug(message: impl std::fmt::Display) {
    if std::env::var_os("BASTION_HOOK_DEBUG").is_some_and(|v| v == "1") {
        eprintln!("bastion hook: {message}");
    }
}

pub(super) struct Daemon {
    client: reqwest::Client,
    pub(super) base: String,
    token: Option<String>,
}

impl Daemon {
    pub(super) fn discover() -> Self {
        let base = std::env::var("BASTION_URL")
            .ok()
            .filter(|u| !u.trim().is_empty())
            .unwrap_or_else(|| {
                let port =
                    std::env::var("BASTION_HTTP_PORT").unwrap_or_else(|_| "8080".to_string());
                format!("http://127.0.0.1:{port}")
            });
        let base = base.trim_end_matches('/').to_string();
        let env = |name: &str| std::env::var(name).ok().filter(|t| !t.trim().is_empty());
        let token = env("BASTION_TOKEN")
            .or_else(|| {
                crate::tui::is_local_daemon_url(&base)
                    .then(|| env("BASTION_BOOTSTRAP_TOKEN"))
                    .flatten()
            })
            .or_else(crate::tui::saved_session_token);
        Self {
            client: reqwest::Client::new(),
            base,
            token,
        }
    }

    /// One call for the CLI: the answer body, or the daemon's error text.
    pub(super) async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
    ) -> anyhow::Result<Value> {
        let mut request = self
            .client
            .request(method, format!("{}{path}", self.base))
            .timeout(Duration::from_secs(15));
        if let Some(body) = body {
            request = request.json(body);
        }
        if let Some(token) = &self.token {
            request = request.header("x-bastion-token", token);
        }
        let response = request.send().await.map_err(|e| {
            anyhow::anyhow!("cannot reach the Bastion daemon at {}: {e}", self.base)
        })?;
        let status = response.status();
        let value: Value = response.json().await.unwrap_or(Value::Null);
        if status.is_success() {
            return Ok(value);
        }
        let message = value
            .get("error")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| status.to_string());
        match status.as_u16() {
            401 => anyhow::bail!(
                "the daemon refused the token: set BASTION_TOKEN, or sign in with `bastion chat`"
            ),
            404 if message == status.to_string() => anyhow::bail!(
                "this daemon has no /presence routes (older version, or [presence] enabled = false)"
            ),
            _ => anyhow::bail!("{message}"),
        }
    }

    async fn post(&self, path: &str, body: &Value, timeout: Duration) -> Option<Value> {
        let mut request = self
            .client
            .post(format!("{}{path}", self.base))
            .timeout(timeout)
            .json(body);
        if let Some(token) = &self.token {
            request = request.header("x-bastion-token", token);
        }
        match request.send().await {
            Ok(response) if response.status().is_success() => response.json().await.ok(),
            Ok(response) => {
                debug(format!("{path} answered {}", response.status()));
                None
            }
            Err(e) => {
                debug(format!("{path}: {e}"));
                None
            }
        }
    }
}

fn text(payload: &Value, field: &str) -> Option<String> {
    payload
        .get(field)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// The `/presence/event` body for a Claude Code hook payload.
fn event_body(payload: &Value, event: &str, session_id: &str) -> Value {
    let mut body = json!({
        "source": SOURCE_CLAUDE_CODE,
        "session_id": session_id,
        "event": event,
    });
    for field in ["cwd", "tool_name", "notification_type", "agent_id", "model"] {
        if let Some(value) = text(payload, field) {
            body[field] = json!(value);
        }
    }
    if let Some(input) = sanitize_tool_input(payload.get("tool_input")) {
        body["tool_input"] = input;
    }
    body
}

fn additional_context(event: &str, context: &str) -> Option<Value> {
    (!context.trim().is_empty()).then(|| {
        json!({
            "hookSpecificOutput": {
                "hookEventName": event,
                "additionalContext": context,
            }
        })
    })
}

fn permission_answer(decision: &str, reason: Option<&str>) -> Option<Value> {
    matches!(decision, "allow" | "deny" | "ask").then(|| {
        json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": decision,
                "permissionDecisionReason": reason.unwrap_or("Bastion rule"),
            }
        })
    })
}

/// The text of a transcript message: a plain string, or the `text` parts of
/// a content array (tool results and tool calls are skipped).
fn message_text(message: &Value) -> Option<String> {
    let content = message.get("content")?;
    let text = match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter(|p| p.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => return None,
    };
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// The last prompt the person typed and the reply that followed it, from
/// the tail of a Claude Code transcript (JSON lines).
fn last_exchange(path: &std::path::Path) -> (Option<String>, Option<String>) {
    let Ok(mut file) = std::fs::File::open(path) else {
        return (None, None);
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    if len > TRANSCRIPT_TAIL_BYTES {
        let _ = file.seek(SeekFrom::Start(len - TRANSCRIPT_TAIL_BYTES));
    }
    let mut raw = Vec::new();
    if file.read_to_end(&mut raw).is_err() {
        return (None, None);
    }
    let raw = String::from_utf8_lossy(&raw);
    let (mut prompt, mut reply) = (None, None);
    for line in raw.lines() {
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue; // the first line after a seek is usually partial
        };
        if entry.get("isMeta").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let Some(message) = entry.get("message") else {
            continue;
        };
        match entry.get("type").and_then(Value::as_str) {
            Some("user") => {
                if let Some(text) = message_text(message).filter(|t| !t.starts_with('<')) {
                    prompt = Some(text);
                    reply = None;
                }
            }
            Some("assistant") => {
                if let Some(text) = message_text(message) {
                    reply = Some(text);
                }
            }
            _ => {}
        }
    }
    (
        prompt.map(|p| cut(&p, JOURNAL_TEXT_CHARS)),
        reply.map(|r| cut(&r, JOURNAL_TEXT_CHARS)),
    )
}

/// Handles one Claude Code hook payload; `Some` is what to print.
async fn claude_code(daemon: &Daemon, payload: &Value) -> Option<Value> {
    let event = text(payload, "hook_event_name")?;
    let session_id = text(payload, "session_id")?;
    let quick = Duration::from_secs(2);
    let event_body = event_body(payload, &event, &session_id);
    let base = json!({ "source": SOURCE_CLAUDE_CODE, "session_id": session_id });

    match event.as_str() {
        "SessionStart" | "UserPromptSubmit" => {
            daemon.post("/presence/event", &event_body, quick).await;
            let mut request = base.clone();
            if event == "SessionStart" {
                request["kind"] = json!("session");
            } else {
                request["kind"] = json!("prompt");
                request["prompt"] = json!(text(payload, "prompt").unwrap_or_default());
            }
            let answer = daemon
                .post("/presence/context", &request, Duration::from_secs(4))
                .await?;
            additional_context(&event, answer.get("context")?.as_str()?)
        }
        "PreToolUse" => {
            daemon.post("/presence/event", &event_body, quick).await;
            let mut request = base.clone();
            request["tool_name"] = json!(text(payload, "tool_name").unwrap_or_default());
            if let Some(cwd) = text(payload, "cwd") {
                request["cwd"] = json!(cwd);
            }
            // Untruncated (whitelisted fields only): a rule must see the whole
            // command, not the 200-character copy surfaces display.
            if let Some(input) = select_tool_input(payload.get("tool_input"), RULE_INPUT_MAX) {
                request["tool_input"] = input;
            }
            // A `remote` rule waits for a person: the host's own hook timeout
            // (pack `hooks.json`) is the real bound, this one only a backstop.
            let answer = daemon
                .post("/presence/approval", &request, Duration::from_secs(600))
                .await?;
            permission_answer(
                answer.get("decision")?.as_str()?,
                answer.get("reason").and_then(Value::as_str),
            )
        }
        "Stop" => {
            daemon.post("/presence/event", &event_body, quick).await;
            if let Some(path) = text(payload, "transcript_path") {
                let (prompt, reply) = last_exchange(std::path::Path::new(&path));
                if prompt.is_some() || reply.is_some() {
                    let mut request = base.clone();
                    request["cwd"] = json!(text(payload, "cwd"));
                    request["prompt"] = json!(prompt);
                    request["reply"] = json!(reply);
                    daemon.post("/presence/journal", &request, quick).await;
                }
            }
            None
        }
        _ => {
            daemon.post("/presence/event", &event_body, quick).await;
            None
        }
    }
}

/// Entry point of `bastion hook <host>`. Never fails the host: every error
/// path ends in `Ok(())` with nothing printed.
pub async fn run(host: &str) -> anyhow::Result<()> {
    if host != SOURCE_CLAUDE_CODE {
        anyhow::bail!("unknown host '{host}' (supported: {SOURCE_CLAUDE_CODE})");
    }
    let mut raw = String::new();
    if std::io::stdin().read_to_string(&mut raw).is_err() {
        return Ok(());
    }
    let Ok(payload) = serde_json::from_str::<Value>(&raw) else {
        debug("payload is not JSON");
        return Ok(());
    };
    let daemon = Daemon::discover();
    if let Some(answer) = claude_code(&daemon, &payload).await {
        println!("{answer}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn events_carry_only_whitelisted_fields() {
        let payload = json!({
            "hook_event_name": "PreToolUse",
            "session_id": "abc",
            "cwd": "/w/demo",
            "tool_name": "Write",
            "tool_input": { "file_path": "/w/demo/a.rs", "content": "fn main() {}" },
            "transcript_path": "/tmp/t.jsonl",
            "prompt": "should not travel in an event",
        });
        let body = event_body(&payload, "PreToolUse", "abc");
        assert_eq!(body["source"], "claude-code");
        assert_eq!(body["tool_input"]["file_path"], "/w/demo/a.rs");
        assert!(body["tool_input"].get("content").is_none());
        assert!(body.get("prompt").is_none());
        assert!(body.get("transcript_path").is_none());
    }

    #[test]
    fn hook_answers_follow_the_claude_code_format() {
        let ctx = additional_context("SessionStart", "<bastion>…</bastion>").unwrap();
        assert_eq!(ctx["hookSpecificOutput"]["hookEventName"], "SessionStart");
        assert!(additional_context("UserPromptSubmit", "  ").is_none());
        let deny = permission_answer("deny", Some("Bastion rule: Bash")).unwrap();
        assert_eq!(deny["hookSpecificOutput"]["permissionDecision"], "deny");
        assert!(permission_answer("none", None).is_none());
    }

    #[test]
    fn the_last_exchange_is_read_from_the_transcript_tail() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        for line in [
            json!({ "type": "user", "message": { "role": "user", "content": "first question" } }),
            json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": "first answer" }] } }),
            json!({ "type": "user", "isMeta": true, "message": { "content": "meta" } }),
            json!({ "type": "user", "message": { "content": "<command-name>/clear</command-name>" } }),
            json!({ "type": "user", "message": { "content": [{ "type": "text", "text": "fix the build" }] } }),
            json!({ "type": "assistant", "message": { "content": [{ "type": "tool_use", "name": "Bash" }] } }),
            json!({ "type": "user", "message": { "content": [{ "type": "tool_result", "content": "ok" }] } }),
            json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": "Build fixed." }] } }),
        ] {
            writeln!(file, "{line}").unwrap();
        }
        let (prompt, reply) = last_exchange(file.path());
        assert_eq!(prompt.as_deref(), Some("fix the build"));
        assert_eq!(reply.as_deref(), Some("Build fixed."));
        assert_eq!(
            last_exchange(std::path::Path::new("/nonexistent")),
            (None, None)
        );
    }
}
