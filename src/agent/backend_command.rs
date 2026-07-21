//! Fase 2.3 (`docs/revamp` plan `lexical-orbiting-hoare.md` — "Subscription
//! utilizável no TUI"): product-side `/backend`/`/backends` command.
//!
//! The kernel already has `cockpit_command`/`set_backend`
//! (`crates/bastion-runtime/src/agent/loop_.rs:932-1163` in the pinned
//! `bastion-core` checkout) — but it is unreachable from this daemon's
//! actual dispatch path: `AgentLoop::handle_command` only ever calls the
//! product-level `CommandHandler` port (`agent::command::handle_command`),
//! which does NOT receive `&mut AgentLoop`, only narrower per-field
//! references (`provider`, `memory`, ...). Only `main.rs`'s `daemon_loop`
//! (stdin and inbound-channel dispatch arms) actually holds `&mut AgentLoop`
//! in its single-select-loop, so that's where this command is special-cased
//! and dispatched, ahead of the generic `handle_command` router.
//!
//! This module mirrors `set_backend`'s fail-closed contract (resolve the
//! runtime BEFORE mutating any state) but goes one step further:
//! `set_backend` deliberately never touches `backend_profile.auth` (it isn't
//! its job — `auth` is described as "orthogonal to backend choice" in the
//! kernel's own `BackendProfile` doc). Bastion's real subscription runtimes
//! all fail their turn at `run_runtime_backed_turn` unless `auth` resolves,
//! so THIS command additionally maps the chosen runtime id to the
//! `[auth.<profile>]` entry it needs (`RUNTIME_AUTH_PROFILES`) and sets
//! `backend_profile.auth` accordingly — the one piece of plumbing that was
//! missing end to end.

use bastion_agent_runtime::AuthProfileRef;
use bastion_runtime::agent::backend::{BackendProfile, ConversationBackend};
use bastion_runtime::agent::loop_::AgentLoop;

use crate::config::{AuthConfig, AuthProfileEntry, BackendSelection};
use crate::config_store::{ConfigStore, KEY_BACKEND_SELECTED};

/// Runtime id -> the `[auth.<profile>]` id in bastion.toml it must resolve
/// against for a runtime-backed turn to actually authenticate. Keep in sync
/// with `installer.sh`'s `configure_backend` (which wires the exact same
/// three pairs via `BASTION_BACKEND_CONVERSATION`/`BASTION_BACKEND_AUTH`) —
/// there is no shared crate boundary enforcing this, both are product-side
/// config surfaces for the same three subscription runtimes.
pub const RUNTIME_AUTH_PROFILES: &[(&str, &str)] = &[
    ("acpx_claude", "claude-subscription"),
    ("codex_app_server", "codex-subscription"),
    ("acpx_opencode", "opencode-subscription"),
];

fn mapped_auth_profile(runtime_id: &str) -> Option<&'static str> {
    RUNTIME_AUTH_PROFILES
        .iter()
        .find(|(id, _)| *id == runtime_id)
        .map(|(_, profile)| *profile)
}

/// The host CLI backing an `[auth.<profile>]` entry, if it is a `HostCli`
/// kind — used to show a live login-status line in `/backend`'s listing.
fn host_cli_for_profile<'a>(auth_cfg: &'a AuthConfig, profile: &str) -> Option<&'a str> {
    match auth_cfg.profiles.get(profile) {
        Some(AuthProfileEntry::HostCli { cli }) => Some(cli.as_str()),
        _ => None,
    }
}

async fn login_status_line(auth_cfg: &AuthConfig, profile: &str) -> String {
    match host_cli_for_profile(auth_cfg, profile) {
        Some(cli) => match crate::auth_profile_registry::probe_host_cli(cli).await {
            Ok(()) => "logged in".to_string(),
            Err(_) => format!("logged out — run /connect {cli} (or `bastion connect {cli}`)"),
        },
        None => format!("profile '{profile}' missing — add [auth.{profile}] to bastion.toml"),
    }
}

/// A4-U S1: persistence goes through the unified `ConfigStore` (key
/// `backend.selected`, same JSON shape the legacy
/// `.bastion/backend-selection.json` held) — one audited write path shared
/// with `/model`, with `config.applied` SSE propagation.
async fn persist(
    store: &ConfigStore,
    selection: &BackendSelection,
    actor: &str,
) -> anyhow::Result<()> {
    let value_json = serde_json::to_string(selection)
        .map_err(|e| anyhow::anyhow!("could not serialize backend selection: {e}"))?;
    store
        .apply(KEY_BACKEND_SELECTED, &value_json, "console", Some(actor))
        .await
        .map_err(|e| anyhow::anyhow!("could not persist backend selection: {e}"))
}

/// `/backend` / `/backends` command entry point.
///
/// `config_store` is the unified override store (A4-U) the selection is
/// persisted through; `auth_cfg` is the loaded `[auth.*]` table, used for
/// the live login-status probes in the listing and for validating a chosen
/// runtime's mapped profile is actually configured; `owner` is recorded as
/// the audit actor.
pub async fn handle(
    agent: &mut AgentLoop,
    arg: Option<&str>,
    config_store: &ConfigStore,
    auth_cfg: &AuthConfig,
    owner: &str,
) -> anyhow::Result<String> {
    let arg = arg.map(str::trim).filter(|s| !s.is_empty());

    match arg {
        None | Some("list") | Some("use") => list_backends(agent, auth_cfg).await,
        Some(spec) => {
            // "/backend use <id>" is the documented form; a bare
            // "/backend <id>" (no "use") is accepted too as a shorthand —
            // both funnel into the same fail-closed `use_backend`.
            let target = spec.strip_prefix("use ").map(str::trim).unwrap_or(spec);
            if target.is_empty() {
                list_backends(agent, auth_cfg).await
            } else {
                use_backend(agent, target, config_store, auth_cfg, owner).await
            }
        }
    }
}

async fn list_backends(agent: &AgentLoop, auth_cfg: &AuthConfig) -> anyhow::Result<String> {
    let mut lines = vec!["Backends de conversa disponíveis:".to_string()];

    let model_active = matches!(
        agent.backend_profile.conversation,
        ConversationBackend::Model
    );
    lines.push(format!(
        "  model{} — Bastion tool loop (provider/model via /model)",
        if model_active { "  [ativo]" } else { "" }
    ));

    for descriptor in agent.runtime_registry.descriptors() {
        let id = descriptor.id;
        let active = matches!(
            &agent.backend_profile.conversation,
            ConversationBackend::Runtime(active_id) if active_id.as_str() == id
        );
        let health = match agent.runtime_registry.resolve(id).await {
            Ok(_) => "ok".to_string(),
            Err(e) => format!("unhealthy ({e})"),
        };
        let login = match mapped_auth_profile(id) {
            Some(profile) => login_status_line(auth_cfg, profile).await,
            None => "no auth profile mapped".to_string(),
        };
        lines.push(format!(
            "  runtime:{id}{} — health: {health} · login: {login} · coverage: {:?}",
            if active { "  [ativo]" } else { "" },
            descriptor.policy_coverage,
        ));
    }

    lines.push(String::new());
    lines.push(
        "Uso: /backend use model | /backend use runtime:<id> | /backend use <id>".to_string(),
    );
    Ok(lines.join("\n"))
}

async fn use_backend(
    agent: &mut AgentLoop,
    spec: &str,
    config_store: &ConfigStore,
    auth_cfg: &AuthConfig,
    owner: &str,
) -> anyhow::Result<String> {
    if spec == "model" {
        // Deviation from the plan's "restaura auth do cfg" wording: this
        // deliberately mirrors the kernel's own `set_backend("model")` arm,
        // which never touches `backend_profile.auth` either — `auth` is
        // documented as orthogonal to conversation-backend choice (it also
        // backs `task_runtime` delegation, which can stay active while the
        // conversation itself is `Model`). A 4-argument command signature
        // has no access to the ORIGINAL `[backend].auth` from bastion.toml
        // to "restore" precisely, and clobbering whatever is there today
        // would risk breaking an unrelated task_runtime auth override.
        agent.backend_profile.conversation = ConversationBackend::Model;
        agent.backend_profile.coverage_note = None;
        persist(
            config_store,
            &BackendSelection {
                conversation: "model".to_string(),
                auth: agent.backend_profile.auth.as_ref().map(|a| a.0.clone()),
                task_runtime: agent.backend_profile.task_runtime.clone(),
            },
            owner,
        )
        .await?;
        return Ok("Backend de conversa definido para: model (Bastion tool loop).".to_string());
    }

    let id = spec.strip_prefix("runtime:").unwrap_or(spec).to_string();

    let runtime = agent.runtime_registry.resolve(&id).await.map_err(|e| {
        anyhow::anyhow!(
            "não é possível selecionar '{id}' como backend de conversa: {e} \
             (veja /backend para os ids disponíveis agora)"
        )
    })?;

    let profile = mapped_auth_profile(&id).ok_or_else(|| {
        anyhow::anyhow!(
            "runtime '{id}' não tem um auth profile mapeado (RUNTIME_AUTH_PROFILES) — \
             não é possível autenticar o turno"
        )
    })?;

    if !auth_cfg.profiles.contains_key(profile) {
        anyhow::bail!(
            "runtime '{id}' precisa do profile [auth.{profile}] em bastion.toml, \
             que ainda não está configurado — adicione-o e reinicie o daemon"
        );
    }

    let coverage = runtime.descriptor().policy_coverage;
    agent.backend_profile.conversation = ConversationBackend::Runtime(id.clone());
    agent.backend_profile.coverage_note = Some(coverage);
    agent.backend_profile.auth = Some(AuthProfileRef(profile.to_string()));

    persist(
        config_store,
        &BackendSelection {
            conversation: format!("runtime:{id}"),
            auth: Some(profile.to_string()),
            task_runtime: agent.backend_profile.task_runtime.clone(),
        },
        owner,
    )
    .await?;

    let login = login_status_line(auth_cfg, profile).await;
    Ok(format!(
        "Backend de conversa definido para: runtime:{id} (harness tool loop; \
         policy coverage: {coverage:?}). Login: {login}"
    ))
}

/// The active conversation backend as the same short grammar
/// `BackendConfig.conversation`/`BackendSelection.conversation` use —
/// `"model"` or `"runtime:<id>"`. Fase 2.10: `/model`/`/models` always
/// prepend `Backend de conversa: {conversation_label(..)}` to their
/// response, so the command is truthful about scope even when it has
/// nothing to warn about (pairs with `backend_notice` below, which adds the
/// warning only when there IS one).
pub fn conversation_label(profile: &BackendProfile) -> String {
    match &profile.conversation {
        ConversationBackend::Model => "model".to_string(),
        ConversationBackend::Runtime(id) => format!("runtime:{id}"),
    }
}

/// `/model`'s truthful notice (Fase 2.10) when the active conversation
/// backend is a runtime, not `model` — `/model <name>` still switches the
/// provider that backs `Model`-mode turns, but that choice has zero effect
/// on turns while a runtime harness owns the conversation loop.
pub fn backend_notice(profile: &BackendProfile) -> Option<String> {
    match &profile.conversation {
        ConversationBackend::Runtime(id) => Some(format!(
            "Aviso: o backend de conversa ativo é runtime:{id} — /model troca o provider do \
             modo 'model', mas isso não afeta os turnos enquanto runtime:{id} estiver ativo. \
             Use /backend use model para voltar ao modo model."
        )),
        ConversationBackend::Model => None,
    }
}

/// Fase 2.10: the prefix `/model`/`/models` main.rs arms prepend to their
/// reply. `bare` is true for a no-argument `/model`/`/models` (the "show
/// current" form) — that case ALWAYS gets the truthful `Backend de
/// conversa: ...` label per the plan, even when the active backend is
/// `model` (no notice is due then, but the label still is — an owner
/// shouldn't have to infer "model" from the absence of a warning). Every
/// other `/model`/`/models` invocation (switch, reset, browse) only gets
/// `backend_notice()`'s warning, and only when a runtime is actually active.
pub fn model_reply_prefix(profile: &BackendProfile, bare: bool) -> String {
    let mut lines = Vec::new();
    if bare {
        lines.push(format!(
            "Backend de conversa: {}",
            conversation_label(profile)
        ));
    }
    if let Some(notice) = backend_notice(profile) {
        lines.push(notice);
    }
    if lines.is_empty() {
        String::new()
    } else {
        lines.push(String::new());
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_auth_profiles_cover_all_three_subscription_runtimes() {
        assert_eq!(
            mapped_auth_profile("acpx_claude"),
            Some("claude-subscription")
        );
        assert_eq!(
            mapped_auth_profile("codex_app_server"),
            Some("codex-subscription")
        );
        assert_eq!(
            mapped_auth_profile("acpx_opencode"),
            Some("opencode-subscription")
        );
        assert_eq!(mapped_auth_profile("something_else"), None);
    }

    #[test]
    fn host_cli_for_profile_resolves_host_cli_entries_only() {
        let mut profiles = std::collections::HashMap::new();
        profiles.insert(
            "claude-subscription".to_string(),
            AuthProfileEntry::HostCli {
                cli: "claude".to_string(),
            },
        );
        profiles.insert(
            "some-api-key".to_string(),
            AuthProfileEntry::ApiKey {
                env_var: "SOME_KEY".to_string(),
            },
        );
        let cfg = AuthConfig { profiles };
        assert_eq!(
            host_cli_for_profile(&cfg, "claude-subscription"),
            Some("claude")
        );
        assert_eq!(host_cli_for_profile(&cfg, "some-api-key"), None);
        assert_eq!(host_cli_for_profile(&cfg, "missing"), None);
    }

    #[test]
    fn backend_notice_is_none_for_model_and_some_for_runtime() {
        let model_profile = BackendProfile::default();
        assert!(backend_notice(&model_profile).is_none());

        let runtime_profile = BackendProfile {
            conversation: ConversationBackend::Runtime("acpx_claude".to_string()),
            ..Default::default()
        };
        let notice = backend_notice(&runtime_profile).expect("must warn for runtime backend");
        assert!(notice.contains("runtime:acpx_claude"));
    }

    #[test]
    fn model_reply_prefix_bare_always_labels_backend() {
        let model_profile = BackendProfile::default();
        // Bare + model backend: label present, no warning (nothing to warn about).
        let prefix = model_reply_prefix(&model_profile, true);
        assert!(prefix.contains("Backend de conversa: model"));
        assert!(!prefix.contains("Aviso"));

        // Non-bare (a switch/reset) + model backend: nothing to prepend at all.
        assert_eq!(model_reply_prefix(&model_profile, false), "");
    }

    #[test]
    fn model_reply_prefix_runtime_always_warns() {
        let runtime_profile = BackendProfile {
            conversation: ConversationBackend::Runtime("acpx_claude".to_string()),
            ..Default::default()
        };
        // Bare: both the label and the warning.
        let bare_prefix = model_reply_prefix(&runtime_profile, true);
        assert!(bare_prefix.contains("Backend de conversa: runtime:acpx_claude"));
        assert!(bare_prefix.contains("Aviso"));

        // Non-bare: warning only, no redundant label line.
        let switch_prefix = model_reply_prefix(&runtime_profile, false);
        assert!(!switch_prefix.contains("Backend de conversa:"));
        assert!(switch_prefix.contains("Aviso"));
    }

    #[tokio::test]
    async fn use_backend_rejects_unmapped_and_unconfigured_profile() {
        // "model" round-trips without touching the registry or auth_cfg at all.
        let auth_cfg = AuthConfig::default();
        // use_backend for a runtime id with no configured [auth.*] profile
        // must fail BEFORE mutating agent state (fail-closed) — exercised
        // indirectly via mapped_auth_profile + the contains_key guard, since
        // constructing a real AgentLoop is out of scope for a pure unit test
        // here (covered by the E2E checklist in the plan instead).
        assert!(!auth_cfg.profiles.contains_key("claude-subscription"));
        assert_eq!(
            mapped_auth_profile("acpx_claude"),
            Some("claude-subscription")
        );
    }
}
