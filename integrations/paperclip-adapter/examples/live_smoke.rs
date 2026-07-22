//! Manual E2E smoke test against a REAL running Bastion Control Plane —
//! not part of `cargo test` (needs a live server + a seeded credential, see
//! `README.md`). Run with:
//!   BASTION_BASE_URL=http://127.0.0.1:8080 BASTION_TOKEN=... \
//!     cargo run --example live_smoke
//!
//! Exercises heartbeat (create) -> poll -> cancel against the real HTTP API,
//! printing each typed snapshot so a human can eyeball it.

use bastion_paperclip_adapter::BastionAdapter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let base_url = std::env::var("BASTION_BASE_URL").unwrap_or_else(|_| "http://127.0.0.1:8080".to_string());
    let token = std::env::var("BASTION_TOKEN").expect("BASTION_TOKEN must be set");
    let issue_id = format!("PAPERCLIP-SMOKE-{}", std::process::id());

    let adapter = BastionAdapter::new(&base_url, &token);

    println!("== heartbeat (create) for {issue_id} ==");
    let snap = adapter.heartbeat(&issue_id, "paperclip-adapter live smoke test", None).await?;
    println!("{:?}", snap.resource);
    let mut session = snap.session;
    assert_eq!(snap.resource.external_ref.as_deref(), Some(issue_id.as_str()));

    println!("== poll ==");
    let snap = adapter.poll(&session).await?;
    session = snap.session;
    println!("status={:?} outcome={:?}", snap.status, snap.outcome);

    println!("== cancel ==");
    let snap = adapter.cancel(&session).await?;
    println!("status={:?} outcome={:?}", snap.status, snap.outcome);
    assert!(snap.status.is_terminal());
    assert_eq!(snap.outcome, Some(bastion_paperclip_adapter::AdapterOutcome::Cancelled));

    println!("== heartbeat again with the SAME issue_id (idempotency check) ==");
    let snap2 = adapter.heartbeat(&issue_id, "paperclip-adapter live smoke test", None).await?;
    assert_eq!(snap2.resource.id, snap.resource.id, "same issue_id must resolve to the same task");

    println!("OK: task_id={}", snap.resource.id);
    Ok(())
}
