//! M8: `/model status` command glue.
//!
//! Takes `&BackendProfile` (who owns the loop) and the live provider's
//! name/model as plain values, rather than `&AgentLoop` — same shape as
//! `backend_command::conversation_label`/`backend_notice`, and for the same
//! reason: `backend_command.rs`'s own tests explicitly avoid constructing a
//! real `AgentLoop` ("out of scope for a pure unit test"; see that module's
//! `use_backend_rejects_unmapped_and_unconfigured_profile` test), so this
//! command reads `agent.backend_profile`/`agent.provider` at the `main.rs`
//! call site (the same place `/backend` already does) and passes the
//! extracted values in, keeping this module fully testable without a live
//! daemon.
//!
//! No real `ProviderCatalog`/`ProviderUsageSnapshot` is wired anywhere in
//! this repo yet (confirmed during M8 research) — `catalog`/`snapshot` are
//! always `None` here, which `subscription_view`'s pure functions turn into
//! the honest `CatalogUnavailable`/`SourceUnavailable` states, never an
//! invented verdict or number.

use std::sync::Arc;

use bastion_runtime::agent::backend::{BackendProfile, ConversationBackend};

use crate::subscription_auth::SubscriptionAuthService;
use crate::subscription_view::{
    corrective_action, model_status_from_catalog, render, usage_status_from_snapshot,
    ExecutionOwner, ModelStatus, SubscriptionStatusView, UsageStatus,
};

/// `/model status` entry point. `model_id`/`provider_id` are the live
/// provider's own `model_name()`/`name()` (empty/irrelevant when
/// `backend_profile.conversation` isn't `Model` — the caller doesn't need
/// to special-case that). `now` is the caller's clock reading
/// (`bastion_runtime::provider_auth::Clock::now_nanos`), threaded in rather
/// than read internally so this stays testable with a fake clock, same
/// convention the kernel's own lifecycle code uses.
pub async fn handle(
    backend_profile: &BackendProfile,
    model_id: &str,
    provider_id: &str,
    owner: &str,
    subscription_auth: Option<&Arc<SubscriptionAuthService>>,
    now: i64,
) -> String {
    render(
        &build_view(
            backend_profile,
            model_id,
            provider_id,
            owner,
            subscription_auth,
            now,
        )
        .await,
    )
}

async fn build_view(
    backend_profile: &BackendProfile,
    model_id: &str,
    provider_id: &str,
    owner: &str,
    subscription_auth: Option<&Arc<SubscriptionAuthService>>,
    now: i64,
) -> SubscriptionStatusView {
    let execution_owner = ExecutionOwner::from(&backend_profile.conversation);

    if !matches!(backend_profile.conversation, ConversationBackend::Model) {
        // A runtime harness owns the loop — /backend already reports its
        // own health/login; a model/subscription verdict here would answer
        // a question that doesn't apply to the active backend.
        return not_applicable(execution_owner, None, String::new());
    }

    let is_subscription = subscription_auth.is_some_and(|s| s.is_registered(provider_id));
    if !is_subscription {
        // An ordinary API-key provider — subscription status doesn't apply.
        return not_applicable(
            execution_owner,
            Some(provider_id.to_string()),
            model_id.to_string(),
        );
    }

    // Find which profile is authenticating this provider for this owner.
    // `list` is BAAUTH's own read path (`SubscriptionAuthService::list`);
    // if the owner has more than one profile connected under the same
    // provider, the first match is used — disambiguating further would
    // need the raw `provider/model@profile` selection string, which isn't
    // retained once the live `Provider` is built (only `model_id`
    // survives the swap).
    let service = subscription_auth.expect("checked is_subscription above");
    let records = service.list(owner).await.unwrap_or_default();
    let record = records.iter().find(|r| r.provider_id == provider_id);

    let reconnect_profile: Option<&str> = match record {
        Some(r) if r.state.as_ref().is_some_and(|s| s.needs_human()) => Some(&r.profile_id),
        None => Some(crate::subscription_auth::DEFAULT_PROFILE_ID),
        _ => None,
    };
    let profile_id = record.map(|r| r.profile_id.clone());

    let model_status = model_status_from_catalog(None, model_id, now);
    let usage = usage_status_from_snapshot(None, now, 0);
    let action = corrective_action(reconnect_profile, &model_status, &usage);

    SubscriptionStatusView {
        execution_owner,
        provider_id: Some(provider_id.to_string()),
        model_id: model_id.to_string(),
        profile_id,
        model_status,
        usage,
        corrective_action: action,
    }
}

fn not_applicable(
    execution_owner: ExecutionOwner,
    provider_id: Option<String>,
    model_id: String,
) -> SubscriptionStatusView {
    SubscriptionStatusView {
        execution_owner,
        provider_id,
        model_id,
        profile_id: None,
        model_status: ModelStatus::NotApplicable,
        usage: UsageStatus::NotApplicable,
        corrective_action: corrective_action(
            None,
            &ModelStatus::NotApplicable,
            &UsageStatus::NotApplicable,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn runtime_backed_conversation_is_not_applicable() {
        let profile = BackendProfile {
            conversation: ConversationBackend::Runtime("acpx_claude".to_string()),
            ..Default::default()
        };

        let view = build_view(&profile, "gpt-5", "codex", "_local", None, 0).await;
        assert_eq!(
            view.execution_owner,
            ExecutionOwner::ExternalRuntime {
                runtime_id: "acpx_claude".to_string()
            }
        );
        assert_eq!(view.model_status, ModelStatus::NotApplicable);
        assert_eq!(view.usage, UsageStatus::NotApplicable);
    }

    #[tokio::test]
    async fn api_key_provider_is_not_applicable() {
        let profile = BackendProfile::default();

        let view = build_view(&profile, "gpt-4o", "openai", "_local", None, 0).await;
        assert_eq!(view.execution_owner, ExecutionOwner::Bastion);
        assert_eq!(view.provider_id.as_deref(), Some("openai"));
        assert_eq!(view.model_status, ModelStatus::NotApplicable);
    }

    #[tokio::test]
    async fn unregistered_provider_prefix_is_not_applicable_even_with_a_service_present() {
        let f = NamedTempFile::new().unwrap();
        let service = Arc::new(
            crate::subscription_auth::fakes::service_with(
                f.path().to_str().unwrap(),
                "codex",
                Ok(()),
            )
            .await,
        );
        let profile = BackendProfile::default();

        // "openai" is never registered with this service — must not be
        // treated as subscription-backed just because SOME service exists.
        let view = build_view(&profile, "gpt-4o", "openai", "_local", Some(&service), 0).await;
        assert_eq!(view.model_status, ModelStatus::NotApplicable);
    }

    #[tokio::test]
    async fn unconnected_subscription_provider_recommends_reconnect() {
        let f = NamedTempFile::new().unwrap();
        let service = Arc::new(
            crate::subscription_auth::fakes::service_with(
                f.path().to_str().unwrap(),
                "codex",
                Ok(()),
            )
            .await,
        );
        // No `.connect(...)` call — the owner has never logged in.
        let profile = BackendProfile::default();

        let view = build_view(&profile, "gpt-5", "codex", "_local", Some(&service), 0).await;
        assert_eq!(view.model_status, ModelStatus::CatalogUnavailable);
        assert_eq!(view.usage, UsageStatus::SourceUnavailable);
        assert_eq!(
            view.corrective_action,
            crate::subscription_view::CorrectiveAction::Reconnect {
                profile: crate::subscription_auth::DEFAULT_PROFILE_ID.to_string()
            }
        );
    }

    #[tokio::test]
    async fn connected_subscription_provider_reports_the_profile_with_no_action() {
        let f = NamedTempFile::new().unwrap();
        let service = Arc::new(
            crate::subscription_auth::fakes::service_with(
                f.path().to_str().unwrap(),
                "codex",
                Ok(()),
            )
            .await,
        );
        service
            .connect("_local", "codex", "default", "Work")
            .await
            .unwrap();
        let profile = BackendProfile::default();

        let view = build_view(&profile, "gpt-5", "codex", "_local", Some(&service), 0).await;
        assert_eq!(view.profile_id.as_deref(), Some("default"));
        assert_eq!(
            view.corrective_action,
            crate::subscription_view::CorrectiveAction::None
        );
    }

    /// BAUX-05: the rendered text must never leak credential-adjacent
    /// material — proven with the same canary convention
    /// `subscription_auth.rs`'s own scrub test uses.
    #[tokio::test]
    async fn rendered_status_never_leaks_the_canary_material() {
        let f = NamedTempFile::new().unwrap();
        let service = Arc::new(
            crate::subscription_auth::fakes::service_with(
                f.path().to_str().unwrap(),
                "codex",
                Ok(()),
            )
            .await,
        );
        service
            .connect("_local", "codex", "default", "Work")
            .await
            .unwrap();
        let profile = BackendProfile::default();

        let text = handle(&profile, "gpt-5", "codex", "_local", Some(&service), 0).await;
        assert!(!text.contains("CANARY-do-not-leak"));
    }
}
