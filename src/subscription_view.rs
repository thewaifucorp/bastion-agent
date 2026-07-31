//! M8 (BAUX-01..05): pure view types + pure decision functions for
//! `/model status` — no I/O, no rusqlite, no lifecycle dependency, so every
//! rule (which corrective action wins, when a usage snapshot counts as
//! stale) is unit-testable against fixtures without a live daemon.
//!
//! BAUX-02's contract in type form: an absent catalog/usage source is its
//! OWN state (`CatalogUnavailable`/`SourceUnavailable`), never collapsed
//! into `Refused`/a zeroed `Known` — confirmed during M8 research that no
//! connector in this repo produces a real `ProviderCatalog`/
//! `ProviderUsageSnapshot` yet, so `model_status_command.rs` always passes
//! `None` into the two `_from_*` functions below today. They're written and
//! tested against real `bastion_types::provider_catalog` fixtures so wiring
//! a real catalog later is a call-site change, not a rewrite.

use bastion_runtime::agent::backend::ConversationBackend;
use bastion_types::provider_catalog::{
    CatalogError, ProviderCatalog, ProviderUsageSnapshot, UsageSource,
};
use serde::Serialize;

/// Who executes the active turn's conversation loop — a concept that exists
/// only on the Agent side (Core has no notion of "runtime harness").
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExecutionOwner {
    Bastion,
    ExternalRuntime { runtime_id: String },
}

impl From<&ConversationBackend> for ExecutionOwner {
    fn from(backend: &ConversationBackend) -> Self {
        match backend {
            ConversationBackend::Model => ExecutionOwner::Bastion,
            ConversationBackend::Runtime(id) => ExecutionOwner::ExternalRuntime {
                runtime_id: id.clone(),
            },
        }
    }
}

impl ExecutionOwner {
    /// BAUX-01: text-only label so CLI/TUI never lean on color alone.
    pub fn label(&self) -> String {
        match self {
            ExecutionOwner::Bastion => "Bastion".to_string(),
            ExecutionOwner::ExternalRuntime { runtime_id } => {
                format!("runtime externo ({runtime_id})")
            }
        }
    }
}

/// Whether the active model is confirmed valid, explicitly refused by a
/// real catalog, or the catalog can't answer at all right now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ModelStatus {
    Known,
    Refused(CatalogError),
    /// No `ProviderCatalog` was available to ask — distinct from `Refused`
    /// so "nothing wired yet" can never read as "this model was rejected".
    CatalogUnavailable,
    /// The active conversation isn't model-selected at all (a runtime
    /// harness owns the loop) or the active provider isn't a registered
    /// subscription connector — the question doesn't apply.
    NotApplicable,
}

/// Same three-way "known / source unavailable / doesn't apply" shape as
/// [`ModelStatus`], for usage/quota instead of model validity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum UsageStatus {
    Known {
        #[serde(skip_serializing_if = "Option::is_none")]
        used: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        limit: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        remaining: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        period: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reset_at: Option<i64>,
        source: &'static str,
        observed_at: i64,
        stale: bool,
    },
    SourceUnavailable,
    NotApplicable,
}

/// BAUX-03's corrective action, in explicit priority order — resolved by
/// [`corrective_action`] below.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum CorrectiveAction {
    Reconnect { profile: String },
    SelectAnotherModel,
    WaitUntil { reset_at: i64 },
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SubscriptionStatusView {
    pub execution_owner: ExecutionOwner,
    pub provider_id: Option<String>,
    pub model_id: String,
    pub profile_id: Option<String>,
    pub model_status: ModelStatus,
    pub usage: UsageStatus,
    pub corrective_action: CorrectiveAction,
}

/// `catalog = None` means no live catalog was wired for this provider —
/// always `CatalogUnavailable`, never guessed. `catalog = Some(_)` runs the
/// real `ProviderCatalog::select_model` (no required capabilities — this
/// call only asks "is the model still valid", not "does it support X").
pub fn model_status_from_catalog(
    catalog: Option<&ProviderCatalog>,
    model_id: &str,
    now: i64,
) -> ModelStatus {
    match catalog {
        None => ModelStatus::CatalogUnavailable,
        Some(catalog) => match catalog.select_model(model_id, &[], now) {
            Ok(_) => ModelStatus::Known,
            Err(e) => ModelStatus::Refused(e),
        },
    }
}

/// `snapshot = None` means no vendor/local usage source exists for this
/// provider — always `SourceUnavailable`. A present snapshot's staleness
/// uses the snapshot's own `is_stale_at` (never a locally reinvented age
/// check) against the caller's clock, so a fake clock in a test can
/// deterministically prove the `stale` label either way.
pub fn usage_status_from_snapshot(
    snapshot: Option<&ProviderUsageSnapshot>,
    now: i64,
    stale_budget_nanos: i64,
) -> UsageStatus {
    match snapshot {
        None => UsageStatus::SourceUnavailable,
        Some(s) => UsageStatus::Known {
            used: s.used,
            limit: s.limit,
            remaining: s.remaining,
            period: s.period.clone(),
            reset_at: s.reset_at,
            source: match s.source {
                UsageSource::VendorApi => "vendor_api",
                UsageSource::LocalAccounting => "local_accounting",
            },
            observed_at: s.observed_at,
            stale: s.is_stale_at(now, stale_budget_nanos),
        },
    }
}

/// BAUX-03: explicit priority — an operator locked out entirely
/// (`Reconnect`) always outranks a model-selection problem
/// (`SelectAnotherModel`), which always outranks a quota wait
/// (`WaitUntil`) — never invented when the state that would justify it
/// isn't fully known (e.g. `remaining == 0` with `reset_at` unknown yields
/// `None`, not a guessed `WaitUntil`).
pub fn corrective_action(
    reconnect_profile: Option<&str>,
    model_status: &ModelStatus,
    usage: &UsageStatus,
) -> CorrectiveAction {
    if let Some(profile) = reconnect_profile {
        return CorrectiveAction::Reconnect {
            profile: profile.to_string(),
        };
    }
    if matches!(model_status, ModelStatus::Refused(_)) {
        return CorrectiveAction::SelectAnotherModel;
    }
    if let UsageStatus::Known {
        remaining: Some(0),
        reset_at: Some(reset_at),
        ..
    } = usage
    {
        return CorrectiveAction::WaitUntil {
            reset_at: *reset_at,
        };
    }
    CorrectiveAction::None
}

/// BAUX-02: human-readable render — every absent field says "desconhecido"
/// explicitly, never a blank, a 0, or "ilimitado".
pub fn render(view: &SubscriptionStatusView) -> String {
    let mut lines = vec![format!("Dono do loop: {}", view.execution_owner.label())];

    if matches!(view.model_status, ModelStatus::NotApplicable) {
        lines
            .push("Modelo: n/a (backend ativo não é uma seleção de modelo/assinatura)".to_string());
    } else {
        let provider = view.provider_id.as_deref().unwrap_or("desconhecido");
        lines.push(format!("Modelo ativo: {provider}/{}", view.model_id));
        let profile = view.profile_id.as_deref().unwrap_or("desconhecido");
        lines.push(format!("Perfil: {profile}"));
        let status_line = match &view.model_status {
            ModelStatus::Known => "válido no catálogo".to_string(),
            ModelStatus::Refused(reason) => format!("recusado pelo catálogo: {reason}"),
            ModelStatus::CatalogUnavailable => {
                "desconhecido (nenhum catálogo disponível)".to_string()
            }
            ModelStatus::NotApplicable => unreachable!("handled above"),
        };
        lines.push(format!("Status do modelo: {status_line}"));
    }

    match &view.usage {
        UsageStatus::NotApplicable => lines.push("Uso: n/a".to_string()),
        UsageStatus::SourceUnavailable => {
            lines.push("Uso: desconhecido (nenhuma fonte de uso disponível)".to_string())
        }
        UsageStatus::Known {
            used,
            limit,
            remaining,
            period,
            reset_at,
            source,
            observed_at,
            stale,
        } => {
            fn opt<T: std::fmt::Display>(v: &Option<T>) -> String {
                v.as_ref()
                    .map(|x| x.to_string())
                    .unwrap_or_else(|| "desconhecido".to_string())
            }
            lines.push(format!(
                "Uso (fonte: {source}, observado em {observed_at}{}):",
                if *stale { ", DESATUALIZADO" } else { "" }
            ));
            lines.push(format!("  usado: {}", opt(used)));
            lines.push(format!("  limite: {}", opt(limit)));
            lines.push(format!("  restante: {}", opt(remaining)));
            lines.push(format!("  período: {}", opt(period)));
            lines.push(format!("  reinicia em: {}", opt(reset_at)));
        }
    }

    let action_line = match &view.corrective_action {
        CorrectiveAction::Reconnect { profile } => {
            format!("Ação recomendada: reconectar — /auth connect <provider> {profile}")
        }
        CorrectiveAction::SelectAnotherModel => {
            "Ação recomendada: escolher outro modelo — /model <provider>/<modelo>".to_string()
        }
        CorrectiveAction::WaitUntil { reset_at } => {
            format!("Ação recomendada: aguardar até {reset_at} (reinício de quota)")
        }
        CorrectiveAction::None => "Ação recomendada: nenhuma".to_string(),
    };
    lines.push(action_line);

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use bastion_types::provider_catalog::{
        ModelCapability, ProviderAuthFlow, ProviderModelDescriptor, ProviderSupportDescriptor,
    };

    fn catalog_with(model_id: &str, observed_at: i64, valid_until: Option<i64>) -> ProviderCatalog {
        let mut descriptor = ProviderModelDescriptor::new(model_id, observed_at)
            .with_capabilities([ModelCapability::Streaming]);
        if let Some(v) = valid_until {
            descriptor = descriptor.with_valid_until(v);
        }
        ProviderCatalog::new(
            ProviderSupportDescriptor::experimental("codex", ProviderAuthFlow::DeviceCode),
            vec![descriptor],
        )
    }

    #[test]
    fn execution_owner_from_conversation_backend() {
        assert_eq!(
            ExecutionOwner::from(&ConversationBackend::Model),
            ExecutionOwner::Bastion
        );
        assert_eq!(
            ExecutionOwner::from(&ConversationBackend::Runtime("acpx_claude".to_string())),
            ExecutionOwner::ExternalRuntime {
                runtime_id: "acpx_claude".to_string()
            }
        );
    }

    #[test]
    fn model_status_is_catalog_unavailable_without_a_catalog() {
        assert_eq!(
            model_status_from_catalog(None, "gpt-5", 1000),
            ModelStatus::CatalogUnavailable
        );
    }

    #[test]
    fn model_status_is_known_for_a_current_model_in_a_real_catalog() {
        let catalog = catalog_with("gpt-5", 0, None);
        assert_eq!(
            model_status_from_catalog(Some(&catalog), "gpt-5", 1000),
            ModelStatus::Known
        );
    }

    #[test]
    fn model_status_is_refused_for_an_unknown_model_in_a_real_catalog() {
        let catalog = catalog_with("gpt-5", 0, None);
        assert_eq!(
            model_status_from_catalog(Some(&catalog), "gpt-6", 1000),
            ModelStatus::Refused(CatalogError::UnknownModel {
                model_id: "gpt-6".to_string()
            })
        );
    }

    #[test]
    fn model_status_is_refused_for_an_expired_model_entry() {
        let catalog = catalog_with("gpt-5", 0, Some(500));
        assert_eq!(
            model_status_from_catalog(Some(&catalog), "gpt-5", 1000),
            ModelStatus::Refused(CatalogError::ModelExpired {
                model_id: "gpt-5".to_string(),
                valid_until: 500
            })
        );
    }

    #[test]
    fn usage_status_is_source_unavailable_without_a_snapshot() {
        assert_eq!(
            usage_status_from_snapshot(None, 1000, 100),
            UsageStatus::SourceUnavailable
        );
    }

    /// BAUX-04: a fake clock is what proves the `stale` label deterministically
    /// — same `is_stale_at` budget, `now` moved past it.
    #[test]
    fn usage_status_marks_stale_snapshots_via_a_fake_clock() {
        let snapshot = ProviderUsageSnapshot {
            provider_id: "codex".to_string(),
            profile_id: "default".to_string(),
            used: Some(10),
            limit: Some(100),
            remaining: Some(90),
            period: Some("daily".to_string()),
            reset_at: Some(2_000),
            source: UsageSource::VendorApi,
            observed_at: 1_000,
        };
        let fresh = usage_status_from_snapshot(Some(&snapshot), 1_050, 100);
        assert!(matches!(fresh, UsageStatus::Known { stale: false, .. }));

        let stale = usage_status_from_snapshot(Some(&snapshot), 1_200, 100);
        assert!(matches!(stale, UsageStatus::Known { stale: true, .. }));
    }

    /// BAUX-02: an absent quantity field never becomes a fabricated number —
    /// `used`/`limit`/`remaining` all stay `None` straight through.
    #[test]
    fn usage_status_never_invents_missing_quantities() {
        let snapshot =
            ProviderUsageSnapshot::unknown("codex", "default", UsageSource::VendorApi, 0);
        let status = usage_status_from_snapshot(Some(&snapshot), 0, 100);
        match status {
            UsageStatus::Known {
                used,
                limit,
                remaining,
                ..
            } => {
                assert_eq!(used, None);
                assert_eq!(limit, None);
                assert_eq!(remaining, None);
            }
            other => panic!("expected Known with all-None quantities, got {other:?}"),
        }
    }

    #[test]
    fn corrective_action_prioritizes_reconnect_over_everything_else() {
        let action = corrective_action(
            Some("default"),
            &ModelStatus::Refused(CatalogError::UnknownModel {
                model_id: "gpt-6".to_string(),
            }),
            &UsageStatus::Known {
                used: None,
                limit: None,
                remaining: Some(0),
                period: None,
                reset_at: Some(2_000),
                source: "vendor_api",
                observed_at: 0,
                stale: false,
            },
        );
        assert_eq!(
            action,
            CorrectiveAction::Reconnect {
                profile: "default".to_string()
            }
        );
    }

    #[test]
    fn corrective_action_prioritizes_select_another_model_over_wait_until() {
        let action = corrective_action(
            None,
            &ModelStatus::Refused(CatalogError::UnknownModel {
                model_id: "gpt-6".to_string(),
            }),
            &UsageStatus::Known {
                used: None,
                limit: None,
                remaining: Some(0),
                period: None,
                reset_at: Some(2_000),
                source: "vendor_api",
                observed_at: 0,
                stale: false,
            },
        );
        assert_eq!(action, CorrectiveAction::SelectAnotherModel);
    }

    #[test]
    fn corrective_action_waits_only_when_remaining_and_reset_at_are_both_known() {
        let action = corrective_action(
            None,
            &ModelStatus::Known,
            &UsageStatus::Known {
                used: None,
                limit: None,
                remaining: Some(0),
                period: None,
                reset_at: Some(2_000),
                source: "vendor_api",
                observed_at: 0,
                stale: false,
            },
        );
        assert_eq!(action, CorrectiveAction::WaitUntil { reset_at: 2_000 });
    }

    /// `remaining == 0` alone never manufactures a `WaitUntil` — without a
    /// known `reset_at` there is nothing honest to tell the operator to wait
    /// for.
    #[test]
    fn corrective_action_never_invents_wait_until_without_a_known_reset_at() {
        let action = corrective_action(
            None,
            &ModelStatus::Known,
            &UsageStatus::Known {
                used: None,
                limit: None,
                remaining: Some(0),
                period: None,
                reset_at: None,
                source: "vendor_api",
                observed_at: 0,
                stale: false,
            },
        );
        assert_eq!(action, CorrectiveAction::None);
    }

    #[test]
    fn render_never_shows_zero_or_unlimited_for_a_fully_unknown_usage_source() {
        let view = SubscriptionStatusView {
            execution_owner: ExecutionOwner::Bastion,
            provider_id: Some("codex".to_string()),
            model_id: "gpt-5".to_string(),
            profile_id: Some("default".to_string()),
            model_status: ModelStatus::CatalogUnavailable,
            usage: UsageStatus::SourceUnavailable,
            corrective_action: CorrectiveAction::None,
        };
        let text = render(&view);
        assert!(text.contains("desconhecido"));
        assert!(!text.contains("0%"));
        assert!(!text.contains("ilimitado"));
    }
}
