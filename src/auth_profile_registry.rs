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
/// time (startup) — `resolve` itself is a cheap in-memory lookup, not a
/// fresh CLI spawn per turn (a per-message subprocess for every
/// runtime-backed turn would be wasteful; a profile that stops being valid
/// between startup and a turn still surfaces — as whatever error the
/// adapter's own transport produces when it actually tries to use the host
/// session, same failure mode as today, just without a second confirming
/// probe here).
pub struct AuthProfileRegistry {
    verified: HashMap<String, Verified>,
}

impl AuthProfileRegistry {
    /// Verifies every `[auth.<profile>]` entry in `cfg` and keeps only the
    /// ones that pass. A profile that fails is logged (never the profile id
    /// alone is silent) and simply excluded — a later `resolve()` against
    /// its id fails typed, exactly like an unregistered runtime id in
    /// `RuntimeRegistry::resolve`.
    pub async fn build(cfg: &AuthConfig) -> Self {
        let mut verified = HashMap::new();
        for (profile_id, entry) in &cfg.profiles {
            match entry {
                AuthProfileEntry::HostCli { cli } => match verify_host_cli(cli).await {
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
                            "profile excluded from the registry — a runtime-backed turn \
                             selecting it will fail with a typed error, never a silent \
                             fallback",
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
        Self { verified }
    }
}

#[async_trait::async_trait]
impl AuthResolver for AuthProfileRegistry {
    async fn resolve(&self, auth: &AuthProfileRef) -> Result<(), RuntimeError> {
        self.verified.get(&auth.0).map(|_| ()).ok_or_else(|| {
            RuntimeError::Auth(format!(
                "auth profile '{}' is not configured, or failed host verification at \
                 startup (see startup logs for the reason) — never silently proceeding",
                auth.0
            ))
        })
    }
}

/// Read-only "whoami" check for one host CLI — spawns the CLI's own status
/// surface and inspects ONLY `ExitStatus::success()`. Inherits the full
/// parent environment deliberately (unlike an `AgentRuntime` adapter
/// spawning an UNTRUSTED-input-bearing turn, this is the daemon checking
/// its OWN already-trusted CLI's status at startup — the CLI needs `HOME`/
/// `PATH` to find its own config store, same requirement `acpx.rs`
/// documents for the exact same reason).
async fn verify_host_cli(cli: &str) -> Result<(), String> {
    let (program, args): (&str, &[&str]) = match cli {
        "claude" => ("claude", &["auth", "status"]),
        "codex" => ("codex", &["login", "status"]),
        "opencode" => ("opencode", &["auth", "list"]),
        other => {
            return Err(format!(
                "unknown host-cli kind '{other}' — not one of claude/codex/opencode"
            ))
        }
    };
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
}
