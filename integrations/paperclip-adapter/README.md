# bastion-paperclip-adapter

Proof-of-concept adapter showing how an external orchestrator (the planning
doc's example: Paperclip) drives Bastion's durable Pursue tasks entirely
through the public `/v1/*` Control Plane HTTP API
(`docs/en/contracts/control-plane-v1.openapi.yaml`) — never Bastion's
internal Rust types. Paperclip's own codebase isn't available to this repo,
so this crate is the reference: three calls (`heartbeat`, `poll`, `cancel`),
each a thin, typed wrapper over one or two HTTP requests.

Standalone on purpose (own `Cargo.toml`/`Cargo.lock`, not a workspace member
of the `bastion` package) — depending on `bastion` here would defeat the
point of proving the wire contract alone is enough.

## Usage sketch

```rust
use bastion_paperclip_adapter::{BastionAdapter, AdapterSession};

let adapter = BastionAdapter::new("http://127.0.0.1:8080", std::env::var("BASTION_TOKEN")?);

// First contact for an issue: no session yet.
let snapshot = adapter.heartbeat("ISSUE-123", "Fix the flaky test", None).await?;
let mut session = snapshot.session; // persist this (task_id + revision) keyed by ISSUE-123

// Later, on a timer:
let snapshot = adapter.poll(&session).await?;
session = snapshot.session;
if let Some(outcome) = snapshot.outcome {
    // task reached a terminal status — `outcome` is typed, not parsed prose
}

// If Paperclip decides to cancel:
let snapshot = adapter.cancel(&session).await?;
```

## Design invariants (traced to the Phase 5 planning doc)

- **heartbeat creates-or-resumes**, keyed by the caller's issue id as
  `external_ref` — an idempotency key derived from that issue id means
  repeat heartbeats before a session exists never create a duplicate task.
- **Outcomes are read from typed fields** (`status`, `stop_reason.kind`),
  never from parsing the `reason`/`dimension` string content.
- **Session state is exactly `{task_id, revision}`** — this crate holds no
  database; the caller persists and re-supplies it, Bastion is the source of
  truth.
- **Cancellation always calls the control API** (`POST /v1/tasks/{id}:cancel`)
  — never a local process kill (there is no local process to kill: Bastion
  runs tasks in its own daemon).

## Testing

`cargo test` runs `tests/adapter.rs` against a mocked server (`wiremock`) —
no live Bastion daemon required. A real end-to-end run against a live
`bastion-core` container (create -> poll -> cancel, real SQLite-backed task
store) was performed manually for Phase 5 sign-off; see the Control Plane
security doc's Phase 5 section for the summary.
