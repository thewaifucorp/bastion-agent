//! Windows UI Automation helper for a Bastion node.
//!
//! Protocol: one JSON command per line on **stdin**, one JSON reply per line on
//! **stdout**. The node's `ui.snapshot` / `ui.act` capabilities (in
//! `bastion-agent`, which forbids `unsafe`) drive this process; all the COM /
//! Win32 `unsafe` lives here.
//!
//! Commands (`cmd`):
//! - `resolve { match }` — find a top-level window (foreground / by title
//!   substring / by executable name) WITHOUT capturing or sending input, and
//!   report its owning process, so the caller can check the grant's app scope
//!   before anything is captured (BMD-13).
//! - `capture { hwnd, pid, exe }` — re-verify the window still belongs to that
//!   process, then return its UI Automation tree and a PNG screenshot.
//! - `act { hwnd, pid, exe, action }` — re-verify the window still belongs to
//!   that process (`window_gone` otherwise, never acting on another window),
//!   then perform the action (invoke a control, set a value).
//!
//! Replies are `{"ok":true, ...}` on success, or
//! `{"ok":false,"error":"window_gone"|"cancelled"|"failed","detail":"..."}`.
//!
//! NOTE: this package builds only on Windows (it needs the Windows UI
//! Automation and GDI APIs). On other platforms it compiles to a stub so the
//! workspace is inspectable, but every command returns an "unsupported" error.

use std::io::{BufRead, Write};

use serde::Deserialize;
use serde_json::{json, Value};

#[cfg(windows)]
mod win;

/// A command from the node.
#[derive(Debug, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    Resolve {
        #[serde(rename = "match")]
        matcher: Match,
    },
    Capture {
        hwnd: i64,
        pid: u32,
        exe: String,
    },
    Act {
        hwnd: i64,
        pid: u32,
        exe: String,
        action: Action,
    },
}

/// How to find the window to resolve.
#[derive(Debug, Default, Deserialize)]
pub struct Match {
    #[serde(default)]
    pub foreground: bool,
    #[serde(default)]
    pub title_contains: Option<String>,
    #[serde(default)]
    pub exe_name: Option<String>,
}

/// What to do to a window.
#[derive(Debug, Deserialize)]
pub struct Action {
    /// `invoke` | `set_value`.
    pub kind: String,
    pub target: Target,
    #[serde(default)]
    pub value: Option<String>,
}

/// How to find the element inside the window to act on.
#[derive(Debug, Default, Deserialize)]
pub struct Target {
    #[serde(default)]
    pub automation_id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    /// A dotted index path from the window root, e.g. `"0.2.1"`, as returned in
    /// the capture tree's `ref` field.
    #[serde(default, rename = "ref")]
    pub reference: Option<String>,
}

/// Build a typed failure reply.
pub fn err_reply(error: &str, detail: impl Into<String>) -> Value {
    json!({"ok": false, "error": error, "detail": detail.into()})
}

fn handle(request: Request) -> Value {
    #[cfg(windows)]
    {
        win::handle(request)
    }
    #[cfg(not(windows))]
    {
        let _ = request;
        err_reply("failed", "the UI automation helper runs only on Windows")
    }
}

fn main() {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(e) => {
                let _ = writeln!(out, "{}", err_reply("failed", e.to_string()));
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Request>(&line) {
            Ok(request) => handle(request),
            Err(e) => err_reply("failed", format!("bad request: {e}")),
        };
        if writeln!(out, "{reply}").is_err() {
            break;
        }
        let _ = out.flush();
    }
}
