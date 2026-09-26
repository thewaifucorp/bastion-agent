//! Composition-root wiring of the `AgentRuntime` adapters Bastion knows
//! about into the kernel's `RuntimeRegistry`.
//!
//! Conditional registration: an adapter that fails its own `health()` probe
//! (missing binary, version out of the adapter's pinned range, unresolvable
//! auth) never enters the map. An owner whose `[backend]` config then
//! selects that runtime id gets the kernel's fail-closed typed error at turn
//! start (`RuntimeRegistry::resolve` / `BackendResolutionError`) — never a
//! silent fallback to `Model`, which would hide a real loss of policy
//! coverage.
//!
//! Deliberately app-level, not kernel: naming `CodexAppServerRuntime` /
//! `AcpxAgentRuntime` concretely is exactly what the kernel
//! (`bastion_runtime::agent::backend`) must never do — it only ever sees
//! `Arc<dyn AgentRuntime>`.
//!
//! # Health is deliberately `--version`, not "am I logged in" (Fase 2.7/2.9)
//!
//! `register_if_healthy` below only calls each adapter's own `health()`,
//! which today is a handful of `--version` subprocess spawns (see e.g.
//! `bastion_agent_runtime::codex::CodexAppServerRuntime::health`) — it does
//! NOT check whether the wrapped CLI is actually logged into a subscription.
//! This is intentional, not a gap this module should close: a runtime that
//! isn't logged in yet should still be listable (`/backend`, `RuntimeRegistry
//! ::descriptors()`) and selectable — the user needs to be ABLE to select
//! `runtime:acpx_claude` before running `/connect claude` so the login flow
//! has somewhere to attach. Login state is a property of the AUTH profile
//! (`auth_profile_registry.rs`), surfaced separately by `/backend`'s listing
//! and startup's `runtime_not_logged_in` warning (`main.rs`) — conflating the
//! two here would make an unauthenticated-but-installed runtime vanish from
//! the picker entirely, which is worse UX, not better safety (the fail-closed
//! guarantee already lives in `AuthResolver::resolve` at turn start).

use bastion_agent_runtime::acp::AcpAgentRuntime;
use bastion_agent_runtime::acpx::AcpxAgentRuntime;
use bastion_agent_runtime::codex::CodexAppServerRuntime;
use bastion_agent_runtime::{AgentRuntime, HarnessConfinement};
use bastion_runtime::agent::backend::RuntimeRegistry;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// State each harness's CLI writes under `$HOME`: its login, its session
/// store, the npm cache `npx` launches ACP adapters from. Only these (when
/// they exist) are visible to a confined harness — never the rest of the
/// home directory.
fn state_dirs(runtime: &str) -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    let rel: &[&str] = match runtime {
        "codex" => &[".codex"],
        "claude" => &[".acpx", ".npm", ".claude", ".claude.json", ".config/claude"],
        "opencode" => &[
            ".acpx",
            ".npm",
            ".local/share/opencode",
            ".local/state/opencode",
            ".config/opencode",
            ".cache/opencode",
        ],
        _ => &[],
    };
    rel.iter().map(|r| home.join(r)).collect()
}

/// The confinement for `runtime`'s sessions, when this host has a sandbox.
fn confinement(runtime: &str, workspace_base: &Path) -> Option<HarnessConfinement> {
    crate::sandbox::current().map(|sandbox| {
        HarnessConfinement::new(sandbox.clone(), workspace_base)
            .with_read_write(state_dirs(runtime))
    })
}

/// acpx-wrapped agents Bastion probes for — one `AcpxAgentRuntime` per entry,
/// registered only if both `acpx` and the wrapped CLI are present and
/// healthy on this host.
const ACPX_AGENTS: &[&str] = &["claude", "opencode"];

/// The Claude Code ACP bridge, pinned. Used through `npx` when no
/// `claude-agent-acp` is installed on PATH.
pub const CLAUDE_ACP_PACKAGE: &str = "@agentclientprotocol/claude-agent-acp@0.81.2";

/// The bridge command for `acp_claude` — Bastion speaking ACP to Claude Code
/// directly, so every edit Claude asks to make reaches Bastion's approval.
/// Offered only when the `claude` CLI is installed: the bridge runs Claude
/// Code under the operator's own login, which lives with that install.
fn claude_acp_command() -> Option<String> {
    crate::sandbox::resolve_on_path("claude")?;
    if crate::sandbox::resolve_on_path("claude-agent-acp").is_some() {
        return Some("claude-agent-acp".to_string());
    }
    crate::sandbox::resolve_on_path("npx")?;
    Some(format!("npx -y {CLAUDE_ACP_PACKAGE}"))
}

/// Probes every adapter Bastion knows how to construct and returns a
/// registry containing only the ones that are actually usable RIGHT NOW on
/// this host. Cheap even when `[backend]` is entirely absent from
/// bastion.toml — `health()` is a `--version` spawn or an ACP `initialize`
/// handshake, never a live session — and the probes run concurrently.
///
/// Each adapter is confined to `workspace_base/<owner>` plus its own state
/// directories when `crate::sandbox` found a backend at startup.
pub async fn build_runtime_registry(workspace_base: &Path) -> RuntimeRegistry {
    let mut candidates: Vec<Arc<dyn AgentRuntime>> = Vec::new();

    match CodexAppServerRuntime::new() {
        Ok(mut runtime) => {
            if let Some(confinement) = confinement("codex", workspace_base) {
                runtime = runtime.with_confinement(confinement);
            }
            candidates.push(Arc::new(runtime));
        }
        Err(e) => tracing::debug!(
            event = "agent_runtime_construct_failed",
            adapter = "codex_app_server",
            error = %e,
        ),
    }

    for agent in ACPX_AGENTS {
        match AcpxAgentRuntime::new(*agent) {
            Ok(mut runtime) => {
                if let Some(confinement) = confinement(agent, workspace_base) {
                    runtime = runtime.with_confinement(confinement);
                }
                candidates.push(Arc::new(runtime));
            }
            Err(e) => tracing::debug!(
                event = "agent_runtime_construct_failed",
                adapter = %agent,
                error = %e,
            ),
        }
    }

    if let Some(command) = claude_acp_command() {
        let mut runtime = AcpAgentRuntime::new(command);
        if let Some(confinement) = confinement("claude", workspace_base) {
            runtime = runtime.with_confinement(confinement);
        }
        candidates.push(Arc::new(runtime));
    }

    let probed = futures_util::future::join_all(
        candidates
            .into_iter()
            .map(|runtime| async move { (runtime.health().await, runtime) }),
    )
    .await;

    let mut registry = RuntimeRegistry::new();
    for (health, runtime) in probed {
        register_if_healthy(&mut registry, runtime, health);
    }
    registry
}

fn register_if_healthy(
    registry: &mut RuntimeRegistry,
    runtime: Arc<dyn AgentRuntime>,
    health: Result<bastion_agent_runtime::RuntimeHealth, bastion_agent_runtime::RuntimeError>,
) {
    let descriptor = runtime.descriptor();
    match health {
        Ok(health) if health.ready => {
            tracing::info!(
                event = "agent_runtime_registered",
                runtime_id = %descriptor.id,
                version = %health.detected_version,
            );
            registry.register(runtime);
        }
        Ok(health) => {
            tracing::info!(
                event = "agent_runtime_unhealthy",
                runtime_id = %descriptor.id,
                detail = ?health.detail,
            );
        }
        Err(e) => {
            tracing::info!(
                event = "agent_runtime_health_check_failed",
                runtime_id = %descriptor.id,
                error = %e,
            );
        }
    }
}
