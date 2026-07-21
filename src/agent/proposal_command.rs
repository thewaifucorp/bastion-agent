//! `/proposal` — the trusted-surface half of staged configuration changes
//! (observability A3). The web app PROPOSES (`POST /proposals`, pending);
//! only this console command APPLIES or rejects, mirroring `/credential`'s
//! Scope::ConsoleOnly rationale: mutations of the agent's constitution
//! (personas today) happen where the operator's hands are, never over a
//! bearer token in a browser.

use std::sync::Arc;

use crate::proposals::{self, Proposal, ProposalPayload, ProposalStatus, SqliteProposalStore};

/// Handle `/proposal <sub> [args]`. `arg` is everything after `/proposal`.
pub async fn handle(
    store: &Arc<SqliteProposalStore>,
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
        "show" => show(store, owner, rest).await,
        "approve" => approve(store, owner, rest).await,
        "reject" => reject(store, owner, rest).await,
        other => Ok(format!(
            "unknown /proposal subcommand '{other}'. Use: list | show <id> | approve <id> | reject <id>"
        )),
    }
}

fn describe(p: &Proposal) -> String {
    let what = match &p.payload {
        ProposalPayload::PersonaEdit { slug, content } => {
            format!("persona_edit personas/{slug}/SOUL.md ({} bytes)", content.len())
        }
    };
    let status = match p.status {
        ProposalStatus::Pending => "PENDING",
        ProposalStatus::Approved => "approved",
        ProposalStatus::Rejected => "rejected",
    };
    format!("  {}  [{status}]  from {}  {what}", p.id, p.origin)
}

async fn list(store: &Arc<SqliteProposalStore>, owner: &str) -> anyhow::Result<String> {
    let items = store.list_for_owner(owner).await?;
    if items.is_empty() {
        return Ok("no proposals. The web app submits them via POST /proposals.".to_string());
    }
    let mut out = String::from("proposals (newest first):\n");
    for p in &items {
        out.push_str(&describe(p));
        out.push('\n');
    }
    out.push_str("use /proposal show <id> to read the full content before approving.");
    Ok(out)
}

async fn show(
    store: &Arc<SqliteProposalStore>,
    owner: &str,
    id: &str,
) -> anyhow::Result<String> {
    if id.is_empty() {
        return Ok("usage: /proposal show <id>".to_string());
    }
    match store.get(owner, id).await? {
        None => Ok(format!("no proposal {id} for this owner.")),
        Some(p) => {
            let mut out = describe(&p);
            out.push('\n');
            match &p.payload {
                ProposalPayload::PersonaEdit { content, .. } => {
                    out.push_str("---- proposed SOUL.md ----\n");
                    out.push_str(content);
                    out.push_str("\n---- end ----");
                }
            }
            Ok(out)
        }
    }
}

async fn approve(
    store: &Arc<SqliteProposalStore>,
    owner: &str,
    id: &str,
) -> anyhow::Result<String> {
    if id.is_empty() {
        return Ok("usage: /proposal approve <id>".to_string());
    }
    let Some(p) = store.get(owner, id).await? else {
        return Ok(format!("no proposal {id} for this owner."));
    };
    if !store.resolve(owner, id, ProposalStatus::Approved).await? {
        return Ok(format!("proposal {id} is not pending — nothing applied."));
    }
    // Approved first, then applied: a failed apply is visible in the log and
    // the row stays approved (audit shows intent), never silently retried.
    match proposals::apply(&proposals::personas_root(), &p.payload).await {
        Ok(msg) => Ok(format!("proposal {id} approved. {msg}")),
        Err(e) => Ok(format!(
            "proposal {id} approved but APPLY FAILED: {e}. Fix and re-submit from the web."
        )),
    }
}

async fn reject(
    store: &Arc<SqliteProposalStore>,
    owner: &str,
    id: &str,
) -> anyhow::Result<String> {
    if id.is_empty() {
        return Ok("usage: /proposal reject <id>".to_string());
    }
    if store.resolve(owner, id, ProposalStatus::Rejected).await? {
        Ok(format!("proposal {id} rejected."))
    } else {
        Ok(format!("proposal {id} is not pending — nothing changed."))
    }
}
