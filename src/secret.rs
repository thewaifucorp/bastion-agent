//! Loop 3-D (`docs/revamp/C3-cloud-ready-design.md`, security point 1):
//! concrete, injectable [`bastion_types::SecretResolver`]s. The kernel
//! contracts crate only defines the trait
//! (`crates/bastion-types/src/secret.rs`) — resolution itself is always a
//! product/host concern, mirroring exactly how `AuthProfileRegistry`
//! (`src/auth_profile_registry.rs`) is the app-level backing for the
//! kernel's `AuthResolver` port.
//!
//! [`EnvSecretResolver`] covers the "env var" case (the default, local/dev
//! mode this codebase already used ad hoc everywhere — `APP_JWT_SECRET`,
//! `BASTION_INFER_TOKEN`, ...). [`MountedFileSecretResolver`] covers the
//! "arquivo montado" case named in the design doc (a hosted operator or a
//! Kubernetes Secret volume mounts one file per secret under a directory).
//! [`LayeredSecretResolver`] tries env first, then a mounted-secrets
//! directory, so ONE resolver — the one `main.rs` builds at boot — serves
//! both local and hosted deployments unchanged; a hosted operator that needs
//! a real secret manager (Vault, AWS Secrets Manager, ...) implements
//! `SecretResolver` themselves and injects it instead — the daemon never
//! needs to know that manager's API.

use bastion_types::{BastionError, SecretResolver, SecretValue};
use std::path::PathBuf;

/// Resolves a [`bastion_types::SecretRef`] by reading the OS environment
/// variable of the same name. Rejects a present-but-empty value the same as
/// absent — an accidentally-blank `APP_JWT_SECRET=` in a `.env` file must
/// fail closed, not silently produce a signing key of zero length.
#[derive(Debug, Default, Clone, Copy)]
pub struct EnvSecretResolver;

impl SecretResolver for EnvSecretResolver {
    fn resolve(&self, name: &str) -> Result<SecretValue, BastionError> {
        match std::env::var(name) {
            Ok(v) if !v.is_empty() => Ok(SecretValue::new(v)),
            _ => Err(BastionError::SecretNotFound {
                name: name.to_string(),
            }),
        }
    }
}

/// Resolves a secret by reading `<dir>/<name>` — one file per secret, the
/// shape a Kubernetes `Secret` volume or a Docker/Compose bind-mounted
/// secrets directory produces. Trailing newline is trimmed (many secret
/// volume implementations, and `echo` used to author one by hand, append
/// one).
#[derive(Debug, Clone)]
pub struct MountedFileSecretResolver {
    dir: PathBuf,
}

impl MountedFileSecretResolver {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }
}

impl SecretResolver for MountedFileSecretResolver {
    fn resolve(&self, name: &str) -> Result<SecretValue, BastionError> {
        // WR-style hardening: `name` only ever comes from our own config/
        // manifest types, never arbitrary external input, but reject a
        // path-traversal-shaped name defensively rather than trust that.
        if name.is_empty() || name.contains('/') || name.contains("..") {
            return Err(BastionError::SecretNotFound {
                name: name.to_string(),
            });
        }
        let path = self.dir.join(name);
        match std::fs::read_to_string(&path) {
            Ok(v) => {
                let trimmed = v.trim_end_matches(['\n', '\r']);
                if trimmed.is_empty() {
                    return Err(BastionError::SecretNotFound {
                        name: name.to_string(),
                    });
                }
                Ok(SecretValue::new(trimmed.to_string()))
            }
            Err(_) => Err(BastionError::SecretNotFound {
                name: name.to_string(),
            }),
        }
    }
}

/// Tries each inner resolver in order, returning the first success. This is
/// the resolver `main.rs` actually wires up: env var first (today's
/// behavior, byte-identical for every existing local/dev deployment that
/// never sets `BASTION_SECRETS_DIR`), then an optional mounted-secrets
/// directory. Neither branch is a cloud concept — a mounted directory is
/// just a path, equally at home in a local dev container or a hosted one;
/// this is the "operator is just another sink" pattern (same law as
/// observability sinks) applied to secrets.
pub struct LayeredSecretResolver {
    layers: Vec<Box<dyn SecretResolver>>,
}

impl LayeredSecretResolver {
    pub fn new(layers: Vec<Box<dyn SecretResolver>>) -> Self {
        Self { layers }
    }
}

impl SecretResolver for LayeredSecretResolver {
    fn resolve(&self, name: &str) -> Result<SecretValue, BastionError> {
        for layer in &self.layers {
            if let Ok(v) = layer.resolve(name) {
                return Ok(v);
            }
        }
        Err(BastionError::SecretNotFound {
            name: name.to_string(),
        })
    }
}

/// The default resolver every `bastion` deployment boots with: env var
/// first, then `BASTION_SECRETS_DIR` (if set) as a mounted-file fallback.
/// Byte-identical to pre-Loop-3-D behavior when `BASTION_SECRETS_DIR` is
/// unset (every deployment today) — this only ADDS a resolution path, never
/// removes the env-var one.
pub fn default_secret_resolver() -> LayeredSecretResolver {
    let mut layers: Vec<Box<dyn SecretResolver>> = vec![Box::new(EnvSecretResolver)];
    if let Ok(dir) = std::env::var("BASTION_SECRETS_DIR") {
        layers.push(Box::new(MountedFileSecretResolver::new(dir)));
    }
    LayeredSecretResolver::new(layers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // std::env::set_var is process-global — serialize every test in this
    // module that touches it so parallel `cargo test` runs don't race.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn env_resolver_resolves_present_var() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("BASTION_TEST_SECRET_A", "top-secret-value");
        let r = EnvSecretResolver.resolve("BASTION_TEST_SECRET_A").unwrap();
        assert_eq!(r.expose_secret(), "top-secret-value");
        std::env::remove_var("BASTION_TEST_SECRET_A");
    }

    #[test]
    fn env_resolver_fails_closed_on_missing_var() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var("BASTION_TEST_SECRET_MISSING");
        let err = EnvSecretResolver
            .resolve("BASTION_TEST_SECRET_MISSING")
            .unwrap_err();
        match err {
            BastionError::SecretNotFound { name } => {
                assert_eq!(name, "BASTION_TEST_SECRET_MISSING")
            }
            other => panic!("expected SecretNotFound, got {other:?}"),
        }
    }

    #[test]
    fn env_resolver_fails_closed_on_empty_var() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("BASTION_TEST_SECRET_EMPTY", "");
        let err = EnvSecretResolver
            .resolve("BASTION_TEST_SECRET_EMPTY")
            .unwrap_err();
        assert!(matches!(err, BastionError::SecretNotFound { .. }));
        std::env::remove_var("BASTION_TEST_SECRET_EMPTY");
    }

    #[test]
    fn mounted_file_resolver_reads_and_trims_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("MY_SECRET"), "file-value-123\n").unwrap();
        let resolver = MountedFileSecretResolver::new(dir.path());
        let v = resolver.resolve("MY_SECRET").unwrap();
        assert_eq!(v.expose_secret(), "file-value-123");
    }

    #[test]
    fn mounted_file_resolver_fails_closed_on_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let resolver = MountedFileSecretResolver::new(dir.path());
        let err = resolver.resolve("NOPE").unwrap_err();
        assert!(matches!(err, BastionError::SecretNotFound { .. }));
    }

    #[test]
    fn mounted_file_resolver_rejects_path_traversal_name() {
        let dir = tempfile::tempdir().unwrap();
        let resolver = MountedFileSecretResolver::new(dir.path());
        let err = resolver.resolve("../../etc/passwd").unwrap_err();
        assert!(matches!(err, BastionError::SecretNotFound { .. }));
    }

    #[test]
    fn layered_resolver_tries_env_before_file() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("SHARED_NAME"), "from-file").unwrap();
        std::env::set_var("SHARED_NAME", "from-env");

        let resolver = LayeredSecretResolver::new(vec![
            Box::new(EnvSecretResolver),
            Box::new(MountedFileSecretResolver::new(dir.path())),
        ]);
        let v = resolver.resolve("SHARED_NAME").unwrap();
        assert_eq!(v.expose_secret(), "from-env");
        std::env::remove_var("SHARED_NAME");
    }

    #[test]
    fn layered_resolver_falls_back_to_file_when_env_absent() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var("ONLY_IN_FILE");
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("ONLY_IN_FILE"), "file-only-value").unwrap();

        let resolver = LayeredSecretResolver::new(vec![
            Box::new(EnvSecretResolver),
            Box::new(MountedFileSecretResolver::new(dir.path())),
        ]);
        let v = resolver.resolve("ONLY_IN_FILE").unwrap();
        assert_eq!(v.expose_secret(), "file-only-value");
    }

    #[test]
    fn layered_resolver_fails_closed_when_no_layer_has_it() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var("NOWHERE_TO_BE_FOUND");
        let dir = tempfile::tempdir().unwrap();
        let resolver = LayeredSecretResolver::new(vec![
            Box::new(EnvSecretResolver),
            Box::new(MountedFileSecretResolver::new(dir.path())),
        ]);
        let err = resolver.resolve("NOWHERE_TO_BE_FOUND").unwrap_err();
        assert!(matches!(err, BastionError::SecretNotFound { .. }));
    }

    #[test]
    fn default_secret_resolver_is_env_only_when_secrets_dir_unset() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var("BASTION_SECRETS_DIR");
        std::env::set_var("BASTION_TEST_SECRET_DEFAULT", "env-value");
        let resolver = default_secret_resolver();
        let v = resolver.resolve("BASTION_TEST_SECRET_DEFAULT").unwrap();
        assert_eq!(v.expose_secret(), "env-value");
        std::env::remove_var("BASTION_TEST_SECRET_DEFAULT");
    }
}
