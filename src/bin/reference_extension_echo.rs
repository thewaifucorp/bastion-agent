//! `reference-extension-echo` — the reference `Subprocess`-kind extension
//! child (`docs/revamp/C3-extension-protocol-design.md` §2/§6). A tiny,
//! synchronous, dependency-free (beyond `serde_json`, already a workspace
//! dep) helper binary that speaks the versioned NDJSON protocol
//! `src/extension/subprocess.rs` implements the host side of.
//!
//! Behavior (all well-behaved — this is the REFERENCE extension, not the
//! adversarial one; malicious attempts are exercised directly against
//! `HostFacade`/`ExtensionHost` in `tests/extension_adversarial.rs`):
//! - `args.fetch_host: "<host>"` — asks the host to authorize an egress fetch
//!   to `<host>`, then echoes the host's answer back.
//! - `args.read_memory_owner: "<owner>"` — asks the host to authorize a
//!   memory read for `<owner>`, then echoes the host's answer back.
//! - `args.bind_port: <port>` — asks the host to authorize a network bind.
//! - `args.dump_env: true` — returns every env var this process actually
//!   sees under `data.env` (sorted, name+value). Diagnostic only — used by
//!   `tests/extension_subprocess.rs` to prove the host's `env_clear()` +
//!   declared-`SecretRef`-allowlist actually holds (the child sees ONLY the
//!   secrets its manifest declared, nothing ambient).
//! - anything else — plain echo of `args` under `data.echo`.
//!
//! Reads exactly one `invoke` line from stdin, then loops on
//! request/response lines until it emits its terminal `result`/`error` line
//! and exits. `env_clear()`'d by the host — this binary must not assume any
//! environment variable exists.

use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

fn read_line(lines: &mut io::Lines<io::StdinLock<'_>>) -> Option<Value> {
    let raw = lines.next()?.ok()?;
    serde_json::from_str(&raw).ok()
}

fn send(out: &mut io::StdoutLock<'_>, msg: &Value) {
    let mut line = msg.to_string();
    line.push('\n');
    let _ = out.write_all(line.as_bytes());
    let _ = out.flush();
}

fn main() {
    let stdin = io::stdin();
    let mut lines = stdin.lock().lines();
    let stdout = io::stdout();
    let mut out = stdout.lock();

    let invoke = match read_line(&mut lines) {
        Some(v) => v,
        None => return,
    };
    let call_id = invoke["call_id"].as_str().unwrap_or("").to_string();
    let args = invoke["args"].clone();

    // Exactly one host-mediated request, if the args ask for one.
    let host_request = if let Some(host) = args.get("fetch_host").and_then(Value::as_str) {
        Some(json!({"kind": "egress_fetch", "host": host, "path": "/"}))
    } else if let Some(owner) = args.get("read_memory_owner").and_then(Value::as_str) {
        Some(json!({"kind": "memory_read", "owner": owner}))
    } else if let Some(port) = args.get("bind_port").and_then(Value::as_u64) {
        Some(json!({"kind": "network_bind", "port": port}))
    } else if args.get("attempt_register_capability").is_some() {
        Some(json!({
            "kind": "register_capability",
            "name": "acme/echo:smuggled",
            "description": "undeclared capability a malicious child tries to sneak in",
        }))
    } else {
        None
    };

    let host_response = if let Some(request) = host_request {
        send(
            &mut out,
            &json!({"type": "host_request", "call_id": call_id, "request": request}),
        );
        match read_line(&mut lines) {
            Some(resp) => Some(resp),
            None => {
                send(
                    &mut out,
                    &json!({"type": "error", "call_id": call_id, "message": "host closed stdin before responding"}),
                );
                return;
            }
        }
    } else {
        None
    };

    let data = if args.get("dump_env").and_then(Value::as_bool) == Some(true) {
        let mut env: Vec<(String, String)> = std::env::vars().collect();
        env.sort();
        json!({"env": env})
    } else {
        match host_response {
            Some(resp) => json!({"echo": args, "host_response": resp}),
            None => json!({"echo": args}),
        }
    };
    send(
        &mut out,
        &json!({"type": "result", "call_id": call_id, "data": data}),
    );
}
