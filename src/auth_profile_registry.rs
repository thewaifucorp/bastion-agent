//! M4-07 (`docs/revamp/BACKLOG.md`): resolves `[auth.<profile>]` config
//! entries into a verified, usable-right-now state — WITHOUT ever touching
//! secret/token material — and backs
//! [`bastion_runtime::agent::ports::AuthResolver`], the kernel port
//! `AgentLoop` calls before starting/resuming a runtime-backed session
//! (`crates/bastion-runtime/src/agent/loop_.rs`).
//!
//! Composition-root wiring (`main.rs`): built once at startup —
//! [`AuthProfileRegistry::build`] verifies every configured profile against
//! the live host and keeps only the ones that pass, mirroring
//! `agent_runtime_registry::build_runtime_registry`'s conditional-
//! registration idiom (an adapter/profile that fails its own probe never
//! enters the map; an id that isn't there resolves to a typed error at turn
//! start, never a silent fallback). Injected via
//! `AgentLoop::with_auth_resolver`.
//!
//! # Deviation (Fase 2.7): lazy re-verify on a configured-but-unverified miss
//!
//! The doc comment on [`AuthProfileRegistry`] below originally promised
//! "verified once, at build() time — resolve() is a cheap in-memory lookup,
//! never a fresh CLI spawn per turn". That still holds for the common case.
//! But a `HostCli` profile that failed its startup probe (not logged in yet)
//! and is then fixed mid-session via `/connect`/`bastion connect` must not
//! require a full daemon restart to start working — the whole point of
//! Fase 2 is a login that "just works" without a restart. So `resolve()` now
//! does ONE extra thing on a miss: if the profile id is present in
//! `configured` (i.e. it exists in bastion.toml, it just didn't pass its
//! startup probe) and is a `HostCli` entry, it re-probes exactly once and
//! caches success into `verified` (behind a `tokio::sync::RwLock`, not a
//! plain `HashMap`, precisely to allow this late write). This is still not a
//! probe-per-turn: once cached, subsequent `resolve()` calls for that id hit
//! the fast path again. `ApiKey` entries are NOT re-probed on miss — an env
//! var doesn't change mid-process in a way this module could usefully react
//! to, so the original "verified once" contract is unchanged for them.
//!
//! # Credential handling (non-negotiable)
//!
//! `HostCli` verification spawns the named CLI's OWN read-only status
//! command (`claude auth status`, `codex login status`, `opencode auth
//! list`) and inspects ONLY the process exit code — stdout (which can carry
//! account labels/org names/emails, never a bearer token, but is not part of
//! any contract this module controls) is read only to satisfy `Command`'s
//! API and is never logged, stored, or returned. `ApiKey` verification
//! checks only that the named env var is SET (`std::env::var(..).is_ok()`)
//! — its value is never read into a variable this module keeps or logs.
//! Nothing here writes to disk, nothing here performs a login — that stays
//! entirely the user's own action via each CLI's native flow.

use bastion_agent_runtime::{AuthProfileRef, RuntimeError};
use bastion_runtime::agent::ports::AuthResolver;
use std::collections::HashMap;
use tokio::process::Command;

use crate::config::{AuthConfig, AuthProfileEntry};

/// One profile's post-verification state — carries no secret material,
/// only what `resolve` needs to answer "is this reference usable".
#[derive(Debug, Clone, PartialEq, Eq)]
enum Verified {
    HostCli { cli: String },
    ApiKey,
}

/// Config-driven [`AuthResolver`]. Verification happens once, at `build()`
/// time (startup), for the common case — `resolve` is then a cheap in-memory
/// lookup, not a fresh CLI spawn per turn. See the "Deviation" section in the
/// module doc above for the one exception: a configured-but-not-yet-verified
/// `HostCli` profile gets a single live re-probe on a `resolve()` miss, so a
/// login completed mid-session (`/connect`) works without a daemon restart.
pub struct AuthProfileRegistry {
    /// The `[auth.<profile>]` table as configured, kept around (not just the
    /// verified subset) so `resolve()` can tell "never configured" apart
    /// from "configured but not verified yet" and re-probe only the latter.
    configured: HashMap<String, AuthProfileEntry>,
    /// Profiles verified as usable. `RwLock`, not a plain map, because
    /// `resolve()` can insert into it on a cache miss (see module doc).
    verified: tokio::sync::RwLock<HashMap<String, Verified>>,
}

impl AuthProfileRegistry {
    /// Verifies every `[auth.<profile>]` entry in `cfg` and keeps only the
    /// ones that pass. A profile that fails is logged (never the profile id
    /// alone is silent) and simply excluded — a later `resolve()` against
    /// its id fails typed, exactly like an unregistered runtime id in
    /// `RuntimeRegistry::resolve` (modulo the lazy re-probe deviation above).
    pub async fn build(cfg: &AuthConfig) -> Self {
        let mut verified = HashMap::new();
        for (profile_id, entry) in &cfg.profiles {
            match entry {
                AuthProfileEntry::HostCli { cli } => match probe_host_cli(cli).await {
                    Ok(()) => {
                        tracing::info!(
                            event = "auth_profile_verified",
                            profile = %profile_id,
                            kind = "host-cli",
                            cli = %cli,
                        );
                        verified.insert(profile_id.clone(), Verified::HostCli { cli: cli.clone() });
                    }
                    Err(detail) => {
                        tracing::warn!(
                            event = "auth_profile_verification_failed",
                            profile = %profile_id,
                            kind = "host-cli",
                            cli = %cli,
                            detail = %detail,
                            "profile excluded from the registry for now — a runtime-backed \
                             turn selecting it fails with a typed error until a later \
                             resolve() re-probe succeeds (e.g. after `/connect`), never a \
                             silent fallback",
                        );
                    }
                },
                AuthProfileEntry::ApiKey { env_var } => {
                    if std::env::var(env_var).is_ok() {
                        tracing::info!(
                            event = "auth_profile_verified",
                            profile = %profile_id,
                            kind = "api-key",
                        );
                        verified.insert(profile_id.clone(), Verified::ApiKey);
                    } else {
                        tracing::warn!(
                            event = "auth_profile_verification_failed",
                            profile = %profile_id,
                            kind = "api-key",
                            detail = "env var not set",
                        );
                    }
                }
            }
        }
        Self {
            configured: cfg.profiles.clone(),
            verified: tokio::sync::RwLock::new(verified),
        }
    }
}

#[async_trait::async_trait]
impl AuthResolver for AuthProfileRegistry {
    async fn resolve(&self, auth: &AuthProfileRef) -> Result<(), RuntimeError> {
        if self.verified.read().await.contains_key(&auth.0) {
            return Ok(());
        }
        // Lazy re-probe (module doc "Deviation"): only for a HostCli profile
        // that IS configured but wasn't verified yet — never for an id
        // nobody configured at all, and never a second time per successful
        // cache insert.
        if let Some(AuthProfileEntry::HostCli { cli }) = self.configured.get(&auth.0) {
            if probe_host_cli(cli).await.is_ok() {
                tracing::info!(
                    event = "auth_profile_verified_lazily",
                    profile = %auth.0,
                    cli = %cli,
                    "profile passed a live re-probe during resolve() — likely a login \
                     completed after startup",
                );
                self.verified
                    .write()
                    .await
                    .insert(auth.0.clone(), Verified::HostCli { cli: cli.clone() });
                return Ok(());
            }
        }
        Err(RuntimeError::Auth(format!(
            "auth profile '{}' is not configured, or failed host verification (see startup \
             logs, or try logging in again) — never silently proceeding",
            auth.0
        )))
    }
}

/// Shared (program, status-verb-args) table for each supported subscription
/// host CLI's own read-only "am I logged in" surface. `pub` (not
/// `pub(crate)`) because `main.rs`'s `bastion connect` — the binary crate,
/// a separate compilation unit from this library crate even though they
/// share one Cargo package — needs the SAME verbs to verify a login it just
/// ran inside the `core` container (`docker compose exec -T core <program>
/// <args>`), so the two surfaces can never drift on what "logged in" means.
pub fn host_cli_status_args(cli: &str) -> Option<(&'static str, &'static [&'static str])> {
    match cli {
        "claude" => Some(("claude", &["auth", "status"])),
        "codex" => Some(("codex", &["login", "status"])),
        "opencode" => Some(("opencode", &["auth", "list"])),
        _ => None,
    }
}

/// Read-only "whoami" check for one host CLI — spawns the CLI's own status
/// surface (via [`host_cli_status_args`]) and inspects ONLY
/// `ExitStatus::success()`. Inherits the full parent environment
/// deliberately (unlike an `AgentRuntime` adapter spawning an
/// UNTRUSTED-input-bearing turn, this is the daemon checking its OWN
/// already-trusted CLI's status, be it at startup or from a live
/// `resolve()`/`/backend`/`/connect` probe — the CLI needs `HOME`/`PATH` to
/// find its own config store, same requirement `acpx.rs` documents for the
/// exact same reason). Renamed from `verify_host_cli` (Fase 2.7) and made
/// `pub` — `backend_command.rs` and `command.rs`'s `/connect`/`/backend`
/// status listings call it directly, not just this module's own `build()`.
pub async fn probe_host_cli(cli: &str) -> Result<(), String> {
    let (program, args) = host_cli_status_args(cli).ok_or_else(|| {
        format!("unknown host-cli kind '{cli}' — not one of claude/codex/opencode")
    })?;
    let output = Command::new(program)
        .args(args)
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| format!("failed to spawn '{program}': {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "'{program} {}' exited non-zero (not logged in, or CLI missing)",
            args.join(" ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn empty_config_verifies_nothing() {
        let registry = AuthProfileRegistry::build(&AuthConfig::default()).await;
        let err = registry
            .resolve(&AuthProfileRef("anything".to_string()))
            .await
            .expect_err("must be Err — nothing configured");
        assert!(matches!(err, RuntimeError::Auth(_)));
    }

    #[tokio::test]
    async fn api_key_profile_resolves_when_env_var_is_set() {
        let var_name = "BASTION_TEST_AUTH_PROFILE_API_KEY_PRESENT";
        // SAFETY-ish: test-only env var, unique name, cleaned up below —
        // `std::env::set_var` is the standard way to exercise this without
        // spawning a real subprocess.
        std::env::set_var(var_name, "not-a-real-secret-just-a-marker");

        let mut profiles = HashMap::new();
        profiles.insert(
            "my-key".to_string(),
            AuthProfileEntry::ApiKey {
                env_var: var_name.to_string(),
            },
        );
        let registry = AuthProfileRegistry::build(&AuthConfig { profiles }).await;
        let result = registry
            .resolve(&AuthProfileRef("my-key".to_string()))
            .await;

        std::env::remove_var(var_name);

        assert!(result.is_ok(), "expected Ok, got {result:?}");
    }

    #[tokio::test]
    async fn api_key_profile_fails_typed_when_env_var_absent() {
        let mut profiles = HashMap::new();
        profiles.insert(
            "missing-key".to_string(),
            AuthProfileEntry::ApiKey {
                env_var: "BASTION_TEST_AUTH_PROFILE_DEFINITELY_NOT_SET".to_string(),
            },
        );
        let registry = AuthProfileRegistry::build(&AuthConfig { profiles }).await;
        let err = registry
            .resolve(&AuthProfileRef("missing-key".to_string()))
            .await
            .expect_err("must be Err — env var not set");
        assert!(matches!(err, RuntimeError::Auth(_)));
    }

    #[tokio::test]
    async fn unknown_host_cli_kind_fails_typed_never_panics() {
        let mut profiles = HashMap::new();
        profiles.insert(
            "weird".to_string(),
            AuthProfileEntry::HostCli {
                cli: "not-a-real-cli-xyz".to_string(),
            },
        );
        let registry = AuthProfileRegistry::build(&AuthConfig { profiles }).await;
        let err = registry
            .resolve(&AuthProfileRef("weird".to_string()))
            .await
            .expect_err("must be Err — unknown CLI kind never verifies");
        assert!(matches!(err, RuntimeError::Auth(_)));
    }

    /// M4-07 acceptance criterion: an `AuthResolver::Err` never contains
    /// secret material — this profile id itself isn't a secret, but this
    /// test documents the expectation the error message must uphold as the
    /// module evolves.
    #[tokio::test]
    async fn resolution_error_never_echoes_env_var_value() {
        let var_name = "BASTION_TEST_AUTH_PROFILE_SECRET_MUST_NOT_LEAK";
        std::env::set_var(var_name, "sk-totally-fake-secret-should-never-appear");

        let mut profiles = HashMap::new();
        profiles.insert(
            "present-but-testing-error-shape".to_string(),
            AuthProfileEntry::ApiKey {
                env_var: "BASTION_TEST_AUTH_PROFILE_ABSENT_VAR".to_string(),
            },
        );
        let registry = AuthProfileRegistry::build(&AuthConfig { profiles }).await;
        let err = registry
            .resolve(&AuthProfileRef(
                "present-but-testing-error-shape".to_string(),
            ))
            .await
            .expect_err("must be Err");

        std::env::remove_var(var_name);

        let msg = err.to_string();
        assert!(!msg.contains("sk-totally-fake-secret-should-never-appear"));
    }

    /// Fase 2.7 deviation: a profile CONFIGURED as `HostCli` but not present
    /// in `verified` (simulating "failed its startup probe, or the daemon
    /// hasn't restarted since login") must get exactly one live re-probe
    /// from `resolve()`, and a success must be cached.
    #[tokio::test]
    async fn resolve_lazily_verifies_and_caches_on_miss() {
        let dir = tempfile::tempdir().expect("temp dir for fake CLI");
        let fake_claude = dir.path().join("claude");
        std::fs::write(&fake_claude, "#!/bin/sh\nexit 0\n").expect("write fake claude");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake_claude, std::fs::Permissions::from_mode(0o755))
                .expect("chmod fake claude");
        }
        let original_path = std::env::var("PATH").unwrap_or_default();
        std::env::set_var(
            "PATH",
            format!("{}:{}", dir.path().display(), original_path),
        );

        let mut configured = HashMap::new();
        configured.insert(
            "lazy-claude".to_string(),
            AuthProfileEntry::HostCli {
                cli: "claude".to_string(),
            },
        );
        let registry = AuthProfileRegistry {
            configured,
            verified: tokio::sync::RwLock::new(HashMap::new()),
        };

        let result = registry
            .resolve(&AuthProfileRef("lazy-claude".to_string()))
            .await;
        let cached = registry.verified.read().await.contains_key("lazy-claude");

        std::env::set_var("PATH", original_path);

        assert!(result.is_ok(), "lazy re-probe must succeed: {result:?}");
        assert!(
            cached,
            "successful lazy probe must be cached into `verified`"
        );
    }

    /// An id that was never configured at all must never trigger a probe —
    /// only "configured but unverified" ids get the lazy re-probe.
    #[tokio::test]
    async fn resolve_never_probes_an_unconfigured_id() {
        let registry = AuthProfileRegistry::build(&AuthConfig::default()).await;
        let err = registry
            .resolve(&AuthProfileRef("never-configured".to_string()))
            .await
            .expect_err("unconfigured id must fail, never probe anything");
        assert!(matches!(err, RuntimeError::Auth(_)));
    }
}
