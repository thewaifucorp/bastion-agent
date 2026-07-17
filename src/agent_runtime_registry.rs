//! Ciclo 2.4 (`docs/revamp/C2-backend-profile-design.md` §2): composition-root
//! wiring of the `AgentRuntime` adapters Bastion knows about into the
//! kernel's `RuntimeRegistry`.
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

use bastion_agent_runtime::acpx::AcpxAgentRuntime;
use bastion_agent_runtime::codex::CodexAppServerRuntime;
use bastion_agent_runtime::AgentRuntime;
use bastion_runtime::agent::backend::RuntimeRegistry;
use std::sync::Arc;

/// acpx-wrapped agents Bastion probes for — one `AcpxAgentRuntime` per entry,
/// registered only if both `acpx` and the wrapped CLI are present and
/// healthy on this host.
const ACPX_AGENTS: &[&str] = &["claude", "opencode"];

/// Probes every adapter Bastion knows how to construct and returns a
/// registry containing only the ones that are actually usable RIGHT NOW on
/// this host. Cheap even when `[backend]` is entirely absent from
/// bastion.toml — `health()` here is a handful of `--version` subprocess
/// spawns, never a live session.
pub async fn build_runtime_registry() -> RuntimeRegistry {
    let mut registry = RuntimeRegistry::new();

    match CodexAppServerRuntime::new() {
        Ok(runtime) => register_if_healthy(&mut registry, Arc::new(runtime)).await,
        Err(e) => tracing::debug!(
            event = "agent_runtime_construct_failed",
            adapter = "codex_app_server",
            error = %e,
        ),
    }

    for agent in ACPX_AGENTS {
        match AcpxAgentRuntime::new(*agent) {
            Ok(runtime) => register_if_healthy(&mut registry, Arc::new(runtime)).await,
            Err(e) => tracing::debug!(
                event = "agent_runtime_construct_failed",
                adapter = %agent,
                error = %e,
            ),
        }
    }

    registry
}

async fn register_if_healthy(registry: &mut RuntimeRegistry, runtime: Arc<dyn AgentRuntime>) {
    let descriptor = runtime.descriptor();
    match runtime.health().await {
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
