//! Loop 3-D acceptance criterion 1 (`docs/revamp/C3-cloud-ready-design.md`):
//! "mesma imagem/binário roda local (paths locais) e hosted-like (paths/
//! secrets injetados) sem recompilar — teste de boot nos dois modos."
//!
//! This is the SAME compiled test binary running the SAME boot-sequence
//! code twice, back to back, in one process, under two different injected
//! profiles — the strongest proof available at the Rust test level that no
//! recompilation is needed to switch modes (there is, structurally, no
//! opportunity for one here: both profiles run inside a single `cargo test`
//! invocation). "Local" uses env-var-injected paths/secrets (today's every
//! bare-metal/dev deployment); "hosted-like" uses a mounted-secrets
//! directory (`BASTION_SECRETS_DIR`, `MountedFileSecretResolver`) alongside
//! DIFFERENT injected paths — simulating an operator's volume/secret mounts
//! — while going through the exact same `bastion::config::load_config` +
//! `bastion::secret::default_secret_resolver` + `SessionManager::init_schema`
//! + `ReadinessState` code path either way.
//!
//! A real `docker build`/container boot is NOT exercised here (out of scope
//! for a Rust test, and this loop's sandbox is disk-constrained — see the
//! LOOP-REPORT for the explicit call-out); this test instead pins the
//! CONFIG/SECRET/READINESS contract that makes the same container image
//! hosted-ready in the first place.

use bastion_types::SecretResolver as _;
use tokio::sync::Mutex;

/// `BASTION_*`/`APP_JWT_SECRET` env vars are process-global — serialize the
/// two profiles in this file (and against any other test file that touches
/// the same names) the same way `tests/config_layering.rs` already does.
/// An async-aware `tokio::sync::Mutex` (not `std::sync::Mutex`) because
/// `boot_and_verify` awaits while holding the guard.
static ENV_LOCK: Mutex<()> = Mutex::const_new(());

struct BootProfile {
    db_path: String,
    log_path: String,
    jwt_secret_source: JwtSecretSource,
}

enum JwtSecretSource {
    /// "Local" mode: the secret comes from a plain env var — today's
    /// behavior for every bare-metal/dev deployment.
    EnvVar(String),
    /// "Hosted-like" mode: the secret comes from a mounted-secrets
    /// directory (`BASTION_SECRETS_DIR`) — the shape a Kubernetes Secret
    /// volume or an operator's own mount produces.
    MountedFile {
        dir: tempfile::TempDir,
        value: String,
    },
}

async fn boot_and_verify(profile: BootProfile, mode: &str) {
    let _guard = ENV_LOCK.lock().await;

    // 1. Paths injected by the "host" (env override on the SAME bastion.toml
    //    default — never a hardcoded path in the binary itself).
    std::env::set_var("BASTION__SESSION__DB_PATH", &profile.db_path);
    std::env::set_var("BASTION__LOGGING__LOG_PATH", &profile.log_path);

    // 2. Secret injected via whichever source this mode uses.
    std::env::remove_var("APP_JWT_SECRET");
    std::env::remove_var("BASTION_SECRETS_DIR");
    match &profile.jwt_secret_source {
        JwtSecretSource::EnvVar(v) => {
            std::env::set_var("APP_JWT_SECRET", v);
        }
        JwtSecretSource::MountedFile { dir, value } => {
            std::fs::write(dir.path().join("APP_JWT_SECRET"), value).unwrap();
            std::env::set_var("BASTION_SECRETS_DIR", dir.path());
        }
    }

    // 3. Load config — the SAME bastion.toml, paths overridden by the
    //    profile above, no code path difference between modes.
    let cfg = bastion::config::load_config("bastion.toml")
        .unwrap_or_else(|e| panic!("[{mode}] load_config failed: {e}"));
    assert_eq!(
        cfg.session.db_path, profile.db_path,
        "[{mode}] db_path not injected"
    );
    assert_eq!(
        cfg.logging.log_path, profile.log_path,
        "[{mode}] log_path not injected"
    );

    // 4. Session store boots at the injected path — proves "volume
    //    persistente: paths injetados pelo host" holds functionally, not
    //    just as a config-struct field.
    let session = bastion_runtime::session::SessionManager::new(&cfg.session.db_path);
    session
        .init_schema()
        .await
        .unwrap_or_else(|e| panic!("[{mode}] session init_schema failed: {e}"));

    // 5. Secret resolves via the injected SecretResolver — same resolver
    //    type, same call, regardless of which underlying layer serves it.
    let resolver = bastion::secret::default_secret_resolver();
    let resolved = resolver
        .resolve("APP_JWT_SECRET")
        .unwrap_or_else(|e| panic!("[{mode}] APP_JWT_SECRET did not resolve: {e:?}"));
    let expected = match &profile.jwt_secret_source {
        JwtSecretSource::EnvVar(v) => v.clone(),
        JwtSecretSource::MountedFile { value, .. } => value.clone(),
    };
    assert_eq!(
        resolved.expose_secret(),
        expected,
        "[{mode}] resolved secret does not match the injected value"
    );

    // 6. Readiness reaches `ready` identically in both modes — the same
    //    boot-sequence signal an orchestrator would probe.
    let readiness = bastion::channel::operational::ReadinessState::new();
    readiness.mark_session_ready();
    readiness.mark_memory_ready();
    readiness.mark_provider_ready();
    readiness.mark_channels_ready();
    let app = axum::Router::new()
        .route(
            "/readyz",
            axum::routing::get(bastion::channel::operational::readiness_handler),
        )
        .with_state(readiness);
    let req = http::Request::builder()
        .uri("/readyz")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = tower::ServiceExt::oneshot(app, req).await.unwrap();
    assert_eq!(
        resp.status(),
        http::StatusCode::OK,
        "[{mode}] /readyz did not report ready after boot"
    );

    // Clean up this profile's env before the next one runs.
    std::env::remove_var("BASTION__SESSION__DB_PATH");
    std::env::remove_var("BASTION__LOGGING__LOG_PATH");
    std::env::remove_var("APP_JWT_SECRET");
    std::env::remove_var("BASTION_SECRETS_DIR");
}

#[tokio::test]
async fn same_binary_boots_local_mode_with_env_injected_paths_and_secret() {
    let db = tempfile::NamedTempFile::new().unwrap();
    let log = tempfile::NamedTempFile::new().unwrap();
    boot_and_verify(
        BootProfile {
            db_path: db.path().to_str().unwrap().to_string(),
            log_path: log.path().to_str().unwrap().to_string(),
            jwt_secret_source: JwtSecretSource::EnvVar("local-dev-jwt-secret".to_string()),
        },
        "local",
    )
    .await;
}

#[tokio::test]
async fn same_binary_boots_hosted_like_mode_with_mounted_paths_and_secret() {
    let data_dir = tempfile::tempdir().unwrap();
    let db_path = data_dir.path().join("sessions.db");
    let log_path = data_dir.path().join("bastion.log");
    let secrets_dir = tempfile::tempdir().unwrap();
    boot_and_verify(
        BootProfile {
            db_path: db_path.to_str().unwrap().to_string(),
            log_path: log_path.to_str().unwrap().to_string(),
            jwt_secret_source: JwtSecretSource::MountedFile {
                dir: secrets_dir,
                value: "hosted-operator-mounted-jwt-secret".to_string(),
            },
        },
        "hosted-like",
    )
    .await;
}

/// Runs BOTH profiles back to back in the SAME test binary invocation — the
/// literal "no recompile between modes" proof: there is no compilation step
/// between these two calls, only different injected env state.
#[tokio::test]
async fn same_binary_boots_both_modes_sequentially_without_recompiling() {
    let db1 = tempfile::NamedTempFile::new().unwrap();
    let log1 = tempfile::NamedTempFile::new().unwrap();
    boot_and_verify(
        BootProfile {
            db_path: db1.path().to_str().unwrap().to_string(),
            log_path: log1.path().to_str().unwrap().to_string(),
            jwt_secret_source: JwtSecretSource::EnvVar("first-pass-local-secret".to_string()),
        },
        "local (first pass)",
    )
    .await;

    let data_dir = tempfile::tempdir().unwrap();
    let secrets_dir = tempfile::tempdir().unwrap();
    boot_and_verify(
        BootProfile {
            db_path: data_dir
                .path()
                .join("sessions.db")
                .to_str()
                .unwrap()
                .to_string(),
            log_path: data_dir
                .path()
                .join("bastion.log")
                .to_str()
                .unwrap()
                .to_string(),
            jwt_secret_source: JwtSecretSource::MountedFile {
                dir: secrets_dir,
                value: "second-pass-hosted-secret".to_string(),
            },
        },
        "hosted-like (second pass, SAME binary)",
    )
    .await;
}
