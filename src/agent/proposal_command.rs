//! `/proposal` — the trusted-surface half of staged configuration changes
//! (observability A3). The web app PROPOSES (`POST /proposals`, pending);
//! only this console command APPLIES or rejects, mirroring `/credential`'s
//! Scope::ConsoleOnly rationale: mutations of the agent's constitution
//! (personas, model config, provider secrets — A4 S2) happen where the
//! operator's hands are, never over a bearer token in a browser.

use std::sync::Arc;

use crate::proposals::{
    self, ApplyResources, Proposal, ProposalPayload, ProposalStatus, SqliteProposalStore,
};

/// Handle `/proposal <sub> [args]`. `arg` is everything after `/proposal`.
/// `res` carries what approve-time apply may need (config store, live
/// provider, pending secret values) — see [`proposals::ApplyResources`].
pub async fn handle(
    store: &Arc<SqliteProposalStore>,
    arg: Option<&str>,
    owner: &str,
    res: &ApplyResources,
) -> anyhow::Result<String> {
    let arg = arg.unwrap_or("").trim();
    let (sub, rest) = match arg.split_once(char::is_whitespace) {
        Some((s, r)) => (s, r.trim()),
        None => (arg, ""),
    };

    match sub {
        "" | "list" => list(store, owner).await,
        "show" => show(store, owner, rest).await,
        "approve" => approve(store, owner, rest, res).await,
        "reject" => reject(store, owner, rest).await,
        other => Ok(format!(
            "unknown /proposal subcommand '{other}'. Use: list | show <id> | approve <id> | reject <id>"
        )),
    }
}

fn describe(p: &Proposal) -> String {
    let what = match &p.payload {
        ProposalPayload::PersonaEdit { slug, content } => {
            format!(
                "persona_edit personas/{slug}/SOUL.md ({} bytes)",
                content.len()
            )
        }
        ProposalPayload::ModelConfig {
            default_model,
            fallback_models,
        } => {
            let mut parts = Vec::new();
            if let Some(m) = default_model {
                parts.push(format!("default={m}"));
            }
            if let Some(f) = fallback_models {
                parts.push(format!("fallbacks=[{}]", f.join(", ")));
            }
            format!("model_config {}", parts.join(" "))
        }
        ProposalPayload::SecretSet {
            provider_id,
            env_key,
        } => {
            // The value is never in the row — nothing to redact here.
            format!("secret_set {env_key} ({provider_id}) — value held in memory until approve")
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

async fn show(store: &Arc<SqliteProposalStore>, owner: &str, id: &str) -> anyhow::Result<String> {
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
                ProposalPayload::ModelConfig { .. } => {
                    out.push_str(
                        "applies through the unified config store (origin web); the default \
                         model hot-swaps like /model, the fallback ladder loads at the next \
                         restart.",
                    );
                }
                ProposalPayload::SecretSet { env_key, .. } => {
                    out.push_str(&format!(
                        "on approve, the in-memory value is written to \
                         BASTION_SECRETS_DIR/{env_key} (0600) and dropped. The value is never \
                         shown and never stored in the proposal table."
                    ));
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
    res: &ApplyResources,
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
    // The config-override actor is whoever APPROVED, not who proposed —
    // provenance follows the authority that made the change real.
    let mut res = res.clone();
    res.actor = Some(owner.to_string());
    // Approved first, then applied: a failed apply is visible in the log and
    // the row stays approved (audit shows intent), never silently retried.
    match proposals::apply(&proposals::personas_root(), &p.id, &p.payload, &res).await {
        Ok(msg) => Ok(format!("proposal {id} approved. {msg}")),
        Err(e) => Ok(format!(
            "proposal {id} approved but APPLY FAILED: {e}. Fix and re-submit from the web."
        )),
    }
}

async fn reject(store: &Arc<SqliteProposalStore>, owner: &str, id: &str) -> anyhow::Result<String> {
    if id.is_empty() {
        return Ok("usage: /proposal reject <id>".to_string());
    }
    if store.resolve(owner, id, ProposalStatus::Rejected).await? {
        Ok(format!("proposal {id} rejected."))
    } else {
        Ok(format!("proposal {id} is not pending — nothing changed."))
    }
}
