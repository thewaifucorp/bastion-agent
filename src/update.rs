//! Release discovery for the self-hosted Bastion installer.
//!
//! The daemon may *report* a newer release to every surface, but it never
//! replaces its own container. Applying an update remains a local host-CLI
//! operation (`bastion update --apply --yes`), where the installer can build,
//! health-check, and roll back the Compose deployment safely.

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;

pub const REPOSITORY: &str = "thewaifucorp/bastion-agent";
const RELEASE_URL: &str = "https://api.github.com/repos/thewaifucorp/bastion-agent/releases/latest";
const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const MAX_UPDATER_REQUEST_BYTES: usize = 4 * 1024;

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct UpdateSnapshot {
    pub current_version: String,
    pub latest_version: Option<String>,
    pub release_tag: Option<String>,
    pub release_url: Option<String>,
    pub available: bool,
    pub checked_at_unix_secs: Option<u64>,
    pub error: Option<String>,
}

impl UpdateSnapshot {
    pub fn current() -> Self {
        Self {
            current_version: env!("CARGO_PKG_VERSION").to_string(),
            ..Self::default()
        }
    }
}

pub type SharedUpdateState = Arc<RwLock<UpdateSnapshot>>;

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn version_from_tag(tag: &str) -> anyhow::Result<semver::Version> {
    semver::Version::parse(tag.trim_start_matches('v'))
        .map_err(|_| anyhow::anyhow!("release tag '{tag}' is not a semantic version"))
}

fn snapshot_for_release(current: &str, release: GithubRelease) -> anyhow::Result<UpdateSnapshot> {
    let current_version = semver::Version::parse(current)
        .map_err(|_| anyhow::anyhow!("Bastion build version '{current}' is invalid"))?;
    let latest_version = version_from_tag(&release.tag_name)?;
    Ok(UpdateSnapshot {
        current_version: current.to_string(),
        latest_version: Some(latest_version.to_string()),
        release_tag: Some(release.tag_name),
        release_url: Some(release.html_url),
        available: latest_version > current_version,
        checked_at_unix_secs: Some(now_unix_secs()),
        error: None,
    })
}

/// Fetch the latest stable GitHub Release. The endpoint is deliberately fixed
/// to the official repository; callers cannot turn a routine update check into
/// an arbitrary URL fetch.
pub async fn check_latest() -> anyhow::Result<UpdateSnapshot> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .user_agent(format!("bastion-agent/{}", env!("CARGO_PKG_VERSION")))
        .build()?;
    let release = client
        .get(RELEASE_URL)
        .send()
        .await?
        .error_for_status()?
        .json::<GithubRelease>()
        .await?;
    snapshot_for_release(env!("CARGO_PKG_VERSION"), release)
}

/// Refresh a shared snapshot. Errors are represented in the snapshot rather
/// than propagated into the daemon's supervision loop: update availability is
/// informational and must never make the agent unavailable.
pub async fn refresh(state: &SharedUpdateState) {
    let (next, failure) = match check_latest().await {
        Ok(snapshot) => (snapshot, None),
        Err(error) => (
            UpdateSnapshot {
                current_version: env!("CARGO_PKG_VERSION").to_string(),
                checked_at_unix_secs: Some(now_unix_secs()),
                error: Some("release check unavailable".to_string()),
                ..UpdateSnapshot::default()
            },
            Some(error),
        ),
    };
    if let Some(error) = failure {
        tracing::warn!(event = "update_check_failed", error = %error);
    } else if next.available {
        tracing::info!(event = "update_available", current = %next.current_version, latest = ?next.latest_version);
    }
    *state.write().await = next;
}

pub fn spawn_checker(state: SharedUpdateState) {
    tokio::spawn(async move {
        loop {
            refresh(&state).await;
            tokio::time::sleep(CHECK_INTERVAL).await;
        }
    });
}

pub async fn snapshot_text(state: &SharedUpdateState) -> String {
    let snapshot = state.read().await.clone();
    match (&snapshot.latest_version, snapshot.available, &snapshot.error) {
        (_, _, Some(_)) => format!(
            "Bastion v{}; não foi possível consultar releases agora. Rode `bastion update` localmente para tentar novamente.",
            snapshot.current_version
        ),
        (Some(latest), true, _) => format!(
            "Atualização disponível: v{} → v{}. No host, rode `bastion update --apply --yes`.{}",
            snapshot.current_version,
            latest,
            snapshot
                .release_url
                .as_deref()
                .map(|url| format!(" Notas: {url}"))
                .unwrap_or_default(),
        ),
        _ => format!("Bastion v{} está atualizado.", snapshot.current_version),
    }
}

#[derive(Debug, Deserialize)]
struct UpdaterRequest {
    token: String,
    action: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct UpdaterResponse {
    accepted: bool,
    message: String,
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |diff, (x, y)| diff | (x ^ y)) == 0
}

/// Ask the optional host-only updater helper to start an update. This client
/// is used only from the Compose container through an explicitly mounted Unix
/// socket; it never receives Docker credentials or a shell capability.
#[cfg(unix)]
pub async fn request_apply() -> anyhow::Result<String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::UnixStream;

    let socket = std::env::var("BASTION_UPDATER_SOCKET")
        .unwrap_or_else(|_| "/bastion-updater/updater.sock".to_string());
    let token = std::env::var("BASTION_UPDATER_TOKEN")
        .map_err(|_| anyhow::anyhow!("host updater is not enabled for this installation"))?;
    let request = serde_json::to_vec(&serde_json::json!({ "token": token, "action": "apply" }))?;
    let mut stream = tokio::time::timeout(Duration::from_secs(3), UnixStream::connect(socket))
        .await
        .map_err(|_| anyhow::anyhow!("host updater did not respond"))??;
    stream.write_all(&request).await?;
    stream.shutdown().await?;
    let mut response = Vec::new();
    stream
        .take(MAX_UPDATER_REQUEST_BYTES as u64)
        .read_to_end(&mut response)
        .await?;
    let response: UpdaterResponse = serde_json::from_slice(&response)?;
    anyhow::ensure!(
        response.accepted,
        "host updater refused the request: {}",
        response.message
    );
    Ok(response.message)
}

#[cfg(not(unix))]
pub async fn request_apply() -> anyhow::Result<String> {
    anyhow::bail!("channel-triggered updates require a Unix host updater")
}

/// Run on the host, never inside the Compose container. One accepted request
/// spawns the existing audited CLI update flow and returns immediately so a
/// chat/channel request is not held open during a build.
#[cfg(unix)]
pub async fn serve_updater(socket: &Path, token: &str) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    use std::process::{Command, Stdio};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::UnixListener;

    if let Some(parent) = socket.parent() {
        std::fs::create_dir_all(parent)?;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    }
    if socket.exists() {
        std::fs::remove_file(socket)?;
    }
    let listener = UnixListener::bind(socket)?;
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))?;

    loop {
        let (mut stream, _) = listener.accept().await?;
        let token = token.to_string();
        tokio::spawn(async move {
            let mut bytes = Vec::new();
            let read = {
                let mut limited = (&mut stream).take(MAX_UPDATER_REQUEST_BYTES as u64);
                limited.read_to_end(&mut bytes).await
            };
            let response = match read
                .ok()
                .and_then(|_| serde_json::from_slice::<UpdaterRequest>(&bytes).ok())
            {
                Some(request)
                    if request.action == "apply"
                        && constant_time_eq(request.token.as_bytes(), token.as_bytes()) => {
                    match std::env::current_exe().and_then(|executable| {
                        Command::new(executable)
                            .args(["update", "--apply", "--yes"])
                            .stdin(Stdio::null())
                            .stdout(Stdio::null())
                            .stderr(Stdio::null())
                            .spawn()
                    }) {
                        Ok(_) => UpdaterResponse {
                            accepted: true,
                            message: "Atualização iniciada no host; Bastion vai reiniciar após o health check.".to_string(),
                        },
                        Err(error) => {
                            tracing::error!(event = "host_update_spawn_failed", error = %error);
                            UpdaterResponse {
                                accepted: false,
                                message: "não foi possível iniciar a atualização no host".to_string(),
                            }
                        }
                    }
                }
                _ => {
                    tracing::warn!(event = "host_update_request_refused");
                    UpdaterResponse {
                        accepted: false,
                        message: "solicitação de atualização recusada".to_string(),
                    }
                }
            };
            if let Ok(payload) = serde_json::to_vec(&response) {
                let _ = stream.write_all(&payload).await;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_release_is_available() {
        let snapshot = snapshot_for_release(
            "0.2.0",
            GithubRelease {
                tag_name: "v0.3.0".into(),
                html_url: "https://example.test/v0.3.0".into(),
            },
        )
        .unwrap();
        assert!(snapshot.available);
        assert_eq!(snapshot.latest_version.as_deref(), Some("0.3.0"));
    }

    #[test]
    fn older_release_is_not_an_update() {
        let snapshot = snapshot_for_release(
            "0.3.0",
            GithubRelease {
                tag_name: "v0.2.0".into(),
                html_url: "https://example.test/v0.2.0".into(),
            },
        )
        .unwrap();
        assert!(!snapshot.available);
    }

    #[test]
    fn malformed_tag_is_rejected() {
        assert!(version_from_tag("latest").is_err());
    }

    #[test]
    fn constant_time_comparison_requires_same_value() {
        assert!(constant_time_eq(b"same", b"same"));
        assert!(!constant_time_eq(b"same", b"other"));
    }
}
