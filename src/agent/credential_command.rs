//! `/credential` — console cockpit for Control Plane credentials.
//!
//! Closes part of the threat model's "no credential-issuance surface" gap
//! (docs/en/control-plane-security.md, "Known gaps"): `issue`/`revoke` on
//! [`SqliteCredentialStore`] were only callable from trusted host code, so a
//! `bcp_` bearer for `/v1/*` (and for the bundled `/ui` dashboard) could not
//! be minted at all without editing source. The console IS the trusted host
//! surface — this command is deliberately `Scope::ConsoleOnly` (like `/as`):
//! the plaintext token is printed exactly once, to the operator's own
//! terminal, never over a remote channel.
//!
//! Like `/task` and `/backend`, it needs a store handle rather than
//! `&mut AgentLoop`, so it is special-cased in the daemon console dispatch,
//! not routed through the generic `CommandHandler` port.

use std::sync::Arc;

use crate::control_plane::credential::SqliteCredentialStore;
use crate::control_plane::scope::{Scope, ScopeSet};

/// Handle `/credential <sub> [args]`. `arg` is everything after `/credential`.
pub async fn handle(
    store: &Arc<SqliteCredentialStore>,
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
        "issue" => issue(store, owner, rest).await,
        "revoke" => revoke(store, owner, rest).await,
        other => Ok(format!(
            "unknown /credential subcommand '{other}'. Use: list | issue <label> [scopes] | \
             revoke <id>  (scopes: comma-separated from {ALL_SCOPE_NAMES}; default {DEFAULT_SCOPES})"
        )),
    }
}

const ALL_SCOPE_NAMES: &str = "tasks:read,tasks:create,tasks:control,webhooks:manage";
const DEFAULT_SCOPES: &str = "tasks:read";

/// The wire-facing scope names (`docs/en/control-plane-security.md`), mapped
/// to [`Scope`]. Kept here (not in `control_plane::scope`) because only this
/// operator-facing parser needs the string form — `/v1` auth compares
/// [`ScopeSet`]s, never names.
fn parse_scope(name: &str) -> Option<Scope> {
    match name {
        "tasks:read" => Some(Scope::TasksRead),
        "tasks:create" => Some(Scope::TasksCreate),
        "tasks:control" => Some(Scope::TasksControl),
        "webhooks:manage" => Some(Scope::WebhooksManage),
        _ => None,
    }
}

fn scope_name(scope: Scope) -> &'static str {
    match scope {
        Scope::TasksRead => "tasks:read",
        Scope::TasksCreate => "tasks:create",
        Scope::TasksControl => "tasks:control",
        Scope::WebhooksManage => "webhooks:manage",
    }
}

fn parse_scopes(csv: &str) -> Result<ScopeSet, String> {
    let mut scopes = Vec::new();
    for raw in csv.split(',') {
        let name = raw.trim();
        if name.is_empty() {
            continue;
        }
        match parse_scope(name) {
            Some(s) => scopes.push(s),
            None => return Err(format!("unknown scope '{name}'. Valid: {ALL_SCOPE_NAMES}")),
        }
    }
    if scopes.is_empty() {
        return Err(format!("no scopes given. Valid: {ALL_SCOPE_NAMES}"));
    }
    Ok(ScopeSet::new(scopes))
}

async fn issue(
    store: &Arc<SqliteCredentialStore>,
    owner: &str,
    rest: &str,
) -> anyhow::Result<String> {
    let (label, scopes_csv) = match rest.split_once(char::is_whitespace) {
        Some((l, s)) => (l, s.trim()),
        None => (rest, ""),
    };
    if label.is_empty() {
        return Ok(format!(
            "usage: /credential issue <label> [scopes]  (default scopes: {DEFAULT_SCOPES})"
        ));
    }
    let scopes = match parse_scopes(if scopes_csv.is_empty() {
        DEFAULT_SCOPES
    } else {
        scopes_csv
    }) {
        Ok(s) => s,
        Err(msg) => return Ok(msg),
    };

    let scope_names: Vec<&str> = scopes.0.iter().map(|s| scope_name(*s)).collect();
    let (id, token) = store.issue(owner, None, scopes, label).await?;
    Ok(format!(
        "credential issued: {label}\n  id:     {id}\n  scopes: {}\n  token:  {token}\n\
         The token is shown ONCE and only its hash is stored — copy it now. \
         Present it as `x-bastion-token` on /v1/* (and in the /ui dashboard).",
        scope_names.join(",")
    ))
}

async fn list(store: &Arc<SqliteCredentialStore>, owner: &str) -> anyhow::Result<String> {
    let creds = store.list_for_owner(owner).await?;
    if creds.is_empty() {
        return Ok("no credentials. Use: /credential issue <label> [scopes]".to_string());
    }
    let mut out = String::from("credentials:\n");
    for c in &creds {
        let scope_names: Vec<&str> = c.scopes.0.iter().map(|s| scope_name(*s)).collect();
        let status = if c.revoked_at.is_some() {
            "REVOKED"
        } else {
            "active"
        };
        out.push_str(&format!(
            "  {}  [{status}]  {}  scopes={}\n",
            c.id,
            c.label,
            scope_names.join(",")
        ));
    }
    Ok(out)
}

async fn revoke(
    store: &Arc<SqliteCredentialStore>,
    owner: &str,
    rest: &str,
) -> anyhow::Result<String> {
    if rest.is_empty() {
        return Ok("usage: /credential revoke <id>".to_string());
    }
    match store.revoke(owner, rest).await {
        Ok(()) => Ok(format!("credential {rest} revoked.")),
        // Domain errors read as a normal reply, matching /task's tone.
        Err(e) => Ok(format!("revoke failed: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_store() -> (tempfile::NamedTempFile, Arc<SqliteCredentialStore>) {
        let f = tempfile::NamedTempFile::new().unwrap();
        let store = Arc::new(SqliteCredentialStore::new(
            f.path().to_str().unwrap().to_owned(),
        ));
        store.init_schema().await.unwrap();
        (f, store)
    }

    #[test]
    fn parse_scopes_accepts_wire_names_and_rejects_unknown() {
        let set = parse_scopes("tasks:read, tasks:control").unwrap();
        assert!(set.has(Scope::TasksRead));
        assert!(set.has(Scope::TasksControl));
        assert!(!set.has(Scope::WebhooksManage));

        assert!(parse_scopes("tasks:everything").is_err());
        assert!(parse_scopes("").is_err());
    }

    #[tokio::test]
    async fn issue_prints_token_once_and_list_never_shows_it() {
        let (_f, store) = test_store().await;
        let out = handle(
            &store,
            Some("issue dashboard tasks:read,tasks:control"),
            "alice",
        )
        .await
        .unwrap();
        assert!(
            out.contains("token:  bcp_"),
            "issue must print the token: {out}"
        );

        let token = out
            .lines()
            .find_map(|l| l.trim().strip_prefix("token:  "))
            .unwrap()
            .to_string();
        let listed = handle(&store, Some("list"), "alice").await.unwrap();
        assert!(listed.contains("dashboard"));
        assert!(
            !listed.contains(&token),
            "list must never echo the plaintext token"
        );
    }

    #[tokio::test]
    async fn issue_defaults_to_tasks_read() {
        let (_f, store) = test_store().await;
        let out = handle(&store, Some("issue ro-probe"), "alice")
            .await
            .unwrap();
        assert!(out.contains("scopes: tasks:read"));
    }

    #[tokio::test]
    async fn revoke_is_owner_scoped() {
        let (_f, store) = test_store().await;
        let out = handle(&store, Some("issue x tasks:read"), "alice")
            .await
            .unwrap();
        let id = out
            .lines()
            .find_map(|l| l.trim().strip_prefix("id:     "))
            .unwrap()
            .to_string();

        let denied = handle(&store, Some(&format!("revoke {id}")), "mallory")
            .await
            .unwrap();
        assert!(denied.contains("revoke failed"));

        let ok = handle(&store, Some(&format!("revoke {id}")), "alice")
            .await
            .unwrap();
        assert!(ok.contains("revoked"));
    }
}
