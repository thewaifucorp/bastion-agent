//! BAAUTH-03 — the `/auth` cockpit: subscription login and profile
//! management (`connect`, `list`, `status`, `disconnect`). Owner-scoped, one
//! implementation reused by every surface (mirrors `/schedule`'s pattern,
//! `agent::schedule_command`) — needs the subscription-auth service rather
//! than `&mut AgentLoop`, so special-cased in dispatch like `/schedule`.
//!
//! `bastion connect <provider>` (the CLI subcommand) already means something
//! else — signing a host CLI (claude/codex/opencode) into its OWN session
//! inside the container (`main.rs::connect_subscription`). This is a
//! deliberately separate command family (`/auth connect`, `bastion auth
//! connect`) for the unrelated subscription-credential-as-inference-provider
//! flow, to avoid silently changing what an existing verb does.

use std::sync::Arc;

use bastion_types::provider_auth::ProviderAuthState;

use crate::subscription_auth::{
    SubscriptionAuthService, SubscriptionProfileRecord, DEFAULT_PROFILE_ID,
};

/// Handle `/auth <sub> [args]`. `arg` is everything after `/auth`.
pub async fn handle(
    service: &Arc<SubscriptionAuthService>,
    arg: Option<&str>,
    owner: &str,
) -> anyhow::Result<String> {
    let arg = arg.unwrap_or("").trim();
    let (sub, rest) = match arg.split_once(char::is_whitespace) {
        Some((s, r)) => (s, r.trim()),
        None => (arg, ""),
    };
    match sub {
        "" | "list" => list(service, owner).await,
        "connect" => connect(service, owner, rest).await,
        "status" => status(service, owner, rest).await,
        "disconnect" => disconnect(service, owner, rest).await,
        other => Ok(format!(
            "unknown /auth subcommand '{other}'. Use: list | connect <provider> [profile] [label...] \
             | status [profile] | disconnect <profile>|<provider>/<profile>"
        )),
    }
}

fn render_state(state: Option<&ProviderAuthState>) -> &'static str {
    match state {
        None => "unknown",
        Some(ProviderAuthState::Ready) => "ready",
        Some(ProviderAuthState::Refreshing) => "refreshing",
        Some(ProviderAuthState::Cooldown { .. }) => "cooldown",
        Some(ProviderAuthState::ReauthRequired { .. }) => "reauth required",
        Some(ProviderAuthState::Revoked) => "revoked",
    }
}

fn render_record(r: &SubscriptionProfileRecord) -> String {
    format!(
        "  {}/{}  [{}]  {}",
        r.provider_id,
        r.profile_id,
        render_state(r.state.as_ref()),
        r.label,
    )
}

async fn list(service: &Arc<SubscriptionAuthService>, owner: &str) -> anyhow::Result<String> {
    let records = service.list(owner).await?;
    if records.is_empty() {
        return Ok("no subscription profiles connected.".to_string());
    }
    let mut out = String::from("subscription profiles:\n");
    for r in &records {
        out.push_str(&render_record(r));
        out.push('\n');
    }
    Ok(out.trim_end().to_string())
}

async fn connect(
    service: &Arc<SubscriptionAuthService>,
    owner: &str,
    rest: &str,
) -> anyhow::Result<String> {
    let usage = "usage: /auth connect <provider> [profile] [label...]";
    let mut parts = rest.split_whitespace();
    let Some(provider) = parts.next() else {
        return Ok(usage.to_string());
    };
    let profile = parts.next().unwrap_or(DEFAULT_PROFILE_ID);
    let label_rest: Vec<&str> = parts.collect();
    let label = if label_rest.is_empty() {
        profile.to_string()
    } else {
        label_rest.join(" ")
    };

    let record = service.connect(owner, provider, profile, &label).await?;
    Ok(format!(
        "connected {}/{} as \"{}\" — {}",
        record.provider_id,
        record.profile_id,
        record.label,
        render_state(record.state.as_ref()),
    ))
}

async fn status(
    service: &Arc<SubscriptionAuthService>,
    owner: &str,
    rest: &str,
) -> anyhow::Result<String> {
    if rest.is_empty() {
        return list(service, owner).await;
    }
    let matches = service.status(owner, rest).await?;
    match matches.as_slice() {
        [] => Ok(format!("no subscription profile named '{rest}'.")),
        [one] => Ok(render_record(one).trim_start().to_string()),
        many => {
            let mut out = format!("'{rest}' matches {} providers:\n", many.len());
            for r in many {
                out.push_str(&render_record(r));
                out.push('\n');
            }
            out.push_str("specify <provider>/<profile> to disambiguate.");
            Ok(out)
        }
    }
}

async fn disconnect(
    service: &Arc<SubscriptionAuthService>,
    owner: &str,
    rest: &str,
) -> anyhow::Result<String> {
    let usage = "usage: /auth disconnect <profile>|<provider>/<profile>";
    if rest.is_empty() {
        return Ok(usage.to_string());
    }
    let (provider, profile) = if let Some((p, f)) = rest.split_once('/') {
        (p.to_string(), f.to_string())
    } else {
        let matches = service.status(owner, rest).await?;
        match matches.as_slice() {
            [] => return Ok(format!("no subscription profile named '{rest}'.")),
            [one] => (one.provider_id.clone(), one.profile_id.clone()),
            many => {
                let mut out = format!(
                    "'{rest}' matches {} providers, specify <provider>/<profile>:\n",
                    many.len()
                );
                for r in many {
                    out.push_str(&render_record(r));
                    out.push('\n');
                }
                return Ok(out.trim_end().to_string());
            }
        }
    };
    service.disconnect(owner, &provider, &profile).await?;
    Ok(format!("disconnected {provider}/{profile}."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subscription_auth::fakes::service_with;
    use bastion_types::provider_auth::ProviderAuthError;
    use tempfile::NamedTempFile;

    async fn service(
        outcome: Result<(), ProviderAuthError>,
    ) -> (NamedTempFile, Arc<SubscriptionAuthService>) {
        let f = NamedTempFile::new().unwrap();
        let service = service_with(f.path().to_str().unwrap(), "codex", outcome).await;
        (f, Arc::new(service))
    }

    #[tokio::test]
    async fn list_with_no_profiles_says_so() {
        let (_f, service) = service(Ok(())).await;
        assert_eq!(
            handle(&service, Some("list"), "alice").await.unwrap(),
            "no subscription profiles connected."
        );
    }

    #[tokio::test]
    async fn bare_arg_defaults_to_list() {
        let (_f, service) = service(Ok(())).await;
        assert_eq!(
            handle(&service, None, "alice").await.unwrap(),
            "no subscription profiles connected."
        );
    }

    #[tokio::test]
    async fn connect_without_a_profile_defaults_to_default_and_echoes_state() {
        let (_f, service) = service(Ok(())).await;
        let out = handle(&service, Some("connect codex"), "alice")
            .await
            .unwrap();
        assert!(out.contains("codex/default"), "{out}");
        assert!(out.contains("ready"), "{out}");
    }

    #[tokio::test]
    async fn connect_with_profile_and_label_uses_both() {
        let (_f, service) = service(Ok(())).await;
        let out = handle(&service, Some("connect codex work Work Account"), "alice")
            .await
            .unwrap();
        assert!(out.contains("codex/work"), "{out}");
        assert!(out.contains("Work Account"), "{out}");
    }

    #[tokio::test]
    async fn connect_to_unknown_provider_surfaces_a_clear_error() {
        let (_f, service) = service(Ok(())).await;
        let err = handle(&service, Some("connect nope"), "alice")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("unknown provider"));
    }

    #[tokio::test]
    async fn status_bare_lists_everything() {
        let (_f, service) = service(Ok(())).await;
        handle(&service, Some("connect codex"), "alice")
            .await
            .unwrap();
        let out = handle(&service, Some("status"), "alice").await.unwrap();
        assert!(out.starts_with("subscription profiles:"), "{out}");
    }

    #[tokio::test]
    async fn status_unknown_profile_says_so_without_erroring() {
        let (_f, service) = service(Ok(())).await;
        let out = handle(&service, Some("status ghost"), "alice")
            .await
            .unwrap();
        assert_eq!(out, "no subscription profile named 'ghost'.");
    }

    #[tokio::test]
    async fn disconnect_bare_removes_the_only_match() {
        let (_f, service) = service(Ok(())).await;
        handle(&service, Some("connect codex work"), "alice")
            .await
            .unwrap();
        let out = handle(&service, Some("disconnect work"), "alice")
            .await
            .unwrap();
        assert_eq!(out, "disconnected codex/work.");
        assert_eq!(
            handle(&service, Some("list"), "alice").await.unwrap(),
            "no subscription profiles connected."
        );
    }

    #[tokio::test]
    async fn disconnect_qualified_form_bypasses_lookup() {
        let (_f, service) = service(Ok(())).await;
        handle(&service, Some("connect codex work"), "alice")
            .await
            .unwrap();
        let out = handle(&service, Some("disconnect codex/work"), "alice")
            .await
            .unwrap();
        assert_eq!(out, "disconnected codex/work.");
    }

    #[tokio::test]
    async fn unknown_subcommand_lists_usage_instead_of_erroring() {
        let (_f, service) = service(Ok(())).await;
        let out = handle(&service, Some("bogus"), "alice").await.unwrap();
        assert!(out.contains("unknown /auth subcommand"), "{out}");
    }
}
