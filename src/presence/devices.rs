//! Paired presence devices: a desk screen (or anything implementing the
//! device API in `docs/en/presence.md`) that Bastion pushes the snapshot to
//! and that sends actions back.
//!
//! Two tokens per device, one per direction:
//! - the device's **push token**, which it issues at pairing; Bastion sends
//!   it as `Authorization: Bearer` on every `POST /api/state`;
//! - the **Bastion token**, which Bastion issues at pairing; the device sends
//!   it as `x-bastion-device-token` on `POST /presence/action`. Only its
//!   SHA-256 is stored here.
//!
//! The registry is a small private JSON file next to the daemon's other
//! state; a removed device can no longer act, and stops receiving pushes on
//! the next change.

use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::hub::now_secs;

const PUSH_TIMEOUT: Duration = Duration::from_secs(3);
const HEARTBEAT: Duration = Duration::from_secs(10);
const DEBOUNCE: Duration = Duration::from_millis(150);

fn default_w() -> u32 {
    240
}
fn default_max_bytes() -> usize {
    4096
}
fn default_max_sessions() -> usize {
    5
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeviceCaps {
    #[serde(default = "default_w")]
    pub w: u32,
    #[serde(default = "default_w")]
    pub h: u32,
    #[serde(default)]
    pub touch: bool,
    #[serde(default = "default_max_bytes")]
    pub max_bytes: usize,
    #[serde(default = "default_max_sessions")]
    pub max_sessions: usize,
}

impl Default for DeviceCaps {
    fn default() -> Self {
        Self {
            w: default_w(),
            h: default_w(),
            touch: false,
            max_bytes: default_max_bytes(),
            max_sessions: default_max_sessions(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeviceRecord {
    pub id: String,
    pub name: String,
    pub addr: String,
    pub owner: String,
    pub caps: DeviceCaps,
    pub paired_at: u64,
    push_token: String,
    bastion_token_sha256: String,
}

/// What `GET /presence/devices` shows: never a token.
#[derive(Clone, Debug, Serialize)]
pub struct DeviceView {
    pub id: String,
    pub name: String,
    pub addr: String,
    pub caps: DeviceCaps,
    pub paired_at: u64,
}

impl From<&DeviceRecord> for DeviceView {
    fn from(d: &DeviceRecord) -> Self {
        Self {
            id: d.id.clone(),
            name: d.name.clone(),
            addr: d.addr.clone(),
            caps: d.caps.clone(),
            paired_at: d.paired_at,
        }
    }
}

fn sha256_hex(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn random_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Constant-time comparison of two equal-length hex digests.
fn digest_eq(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

/// `host`, `host:port`, `a.b.c.d` or `a.b.c.d:port` — no scheme, no path, so
/// a pairing request can only ever reach `http://<addr>/api/...`.
pub fn validate_addr(addr: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !addr.is_empty()
            && addr.len() <= 253
            && addr
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':')),
        "device address must be a host or IP, optionally with :port"
    );
    Ok(())
}

pub struct DeviceRegistry {
    path: Option<PathBuf>,
    devices: RwLock<Vec<DeviceRecord>>,
}

impl DeviceRegistry {
    /// `path: None` keeps the registry in memory (tests).
    pub fn load(path: Option<PathBuf>) -> Self {
        let devices = path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
        Self {
            path,
            devices: RwLock::new(devices),
        }
    }

    fn save(&self, devices: &[DeviceRecord]) -> anyhow::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let body = serde_json::to_vec_pretty(devices)?;
        write_private(path, &body)
    }

    pub fn all(&self) -> Vec<DeviceRecord> {
        self.devices.read().map(|d| d.clone()).unwrap_or_default()
    }

    pub fn list(&self, owner: &str) -> Vec<DeviceView> {
        self.all()
            .iter()
            .filter(|d| d.owner == owner)
            .map(DeviceView::from)
            .collect()
    }

    /// Adds or replaces (same id) a device.
    pub fn upsert(&self, record: DeviceRecord) -> anyhow::Result<()> {
        let mut devices = self
            .devices
            .write()
            .map_err(|_| anyhow::anyhow!("device registry lock poisoned"))?;
        devices.retain(|d| d.id != record.id);
        devices.push(record);
        self.save(&devices)
    }

    pub fn remove(&self, owner: &str, id: &str) -> anyhow::Result<bool> {
        let mut devices = self
            .devices
            .write()
            .map_err(|_| anyhow::anyhow!("device registry lock poisoned"))?;
        let before = devices.len();
        devices.retain(|d| !(d.owner == owner && d.id == id));
        let removed = devices.len() != before;
        if removed {
            self.save(&devices)?;
        }
        Ok(removed)
    }

    /// The device a Bastion token belongs to.
    pub fn by_token(&self, token: &str) -> Option<DeviceRecord> {
        if token.is_empty() {
            return None;
        }
        let digest = sha256_hex(token);
        self.all()
            .into_iter()
            .find(|d| digest_eq(&d.bastion_token_sha256, &digest))
    }
}

#[cfg(unix)]
fn write_private(path: &std::path::Path, body: &[u8]) -> anyhow::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(body)?;
    Ok(())
}

#[cfg(not(unix))]
fn write_private(path: &std::path::Path, body: &[u8]) -> anyhow::Result<()> {
    std::fs::write(path, body).map_err(Into::into)
}

#[derive(Deserialize)]
struct PairAnswer {
    id: String,
    token: String,
    #[serde(default)]
    caps: DeviceCaps,
}

/// Pairs with the device at `addr` using the code on its screen. The device
/// keeps `bastion_url` and the token issued here to send actions back.
pub async fn pair(
    client: &reqwest::Client,
    addr: &str,
    code: &str,
    host: &str,
    bastion_url: &str,
    owner: &str,
    name: Option<&str>,
) -> anyhow::Result<DeviceRecord> {
    validate_addr(addr)?;
    anyhow::ensure!(
        code.len() == 4 && code.chars().all(|c| c.is_ascii_digit()),
        "the pairing code is the 4 digits shown on the device"
    );
    let bastion_token = random_token();
    let response = client
        .post(format!("http://{addr}/api/pair"))
        .timeout(Duration::from_secs(5))
        .json(&serde_json::json!({
            "code": code,
            "host": host,
            "bastion_url": bastion_url,
            "bastion_token": bastion_token,
        }))
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("could not reach the device at {addr}: {e}"))?;
    match response.status().as_u16() {
        200 | 201 => {}
        403 => anyhow::bail!("wrong code — check the device screen and try again"),
        429 => anyhow::bail!("too many wrong codes — wait a minute and try again"),
        status => anyhow::bail!("the device answered {status}"),
    }
    let answer: PairAnswer = response
        .json()
        .await
        .map_err(|e| anyhow::anyhow!("the device sent an unexpected pairing answer: {e}"))?;
    anyhow::ensure!(
        !answer.token.is_empty() && answer.id.len() <= 64,
        "the device sent an unexpected pairing answer"
    );
    Ok(DeviceRecord {
        name: name
            .filter(|n| !n.trim().is_empty())
            .map(|n| super::policy::cut(n.trim(), 24))
            .unwrap_or_else(|| answer.id.clone()),
        id: answer.id,
        addr: addr.to_string(),
        owner: owner.to_string(),
        caps: answer.caps,
        paired_at: now_secs(),
        push_token: answer.token,
        bastion_token_sha256: sha256_hex(&bastion_token),
    })
}

/// Pushes the snapshot to every paired device on each change (debounced)
/// and on a heartbeat, so a device can tell "nothing new" from "Bastion is
/// gone". `snapshot_for` builds one device's snapshot: its owner's, cut to
/// its caps.
pub fn spawn_pusher<F>(
    registry: Arc<DeviceRegistry>,
    mut changes: tokio::sync::watch::Receiver<u64>,
    snapshot_for: F,
) where
    F: Fn(&DeviceRecord) -> Value + Send + Sync + 'static,
{
    tokio::spawn(async move {
        let client = reqwest::Client::new();
        loop {
            tokio::select! {
                changed = changes.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    tokio::time::sleep(DEBOUNCE).await;
                    // Changes made during the debounce ride along with this push.
                    let _ = changes.borrow_and_update();
                }
                _ = tokio::time::sleep(HEARTBEAT) => {}
            }
            for device in registry.all() {
                let body = snapshot_for(&device);
                let result = client
                    .post(format!("http://{}/api/state", device.addr))
                    .bearer_auth(&device.push_token)
                    .timeout(PUSH_TIMEOUT)
                    .json(&body)
                    .send()
                    .await;
                match result {
                    Ok(r) if r.status().is_success() => {}
                    Ok(r) => tracing::debug!(
                        event = "presence_device_push_refused",
                        device = %device.id,
                        status = r.status().as_u16()
                    ),
                    Err(e) => tracing::debug!(
                        event = "presence_device_unreachable",
                        device = %device.id,
                        error = %e
                    ),
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: &str, owner: &str, token: &str) -> DeviceRecord {
        DeviceRecord {
            id: id.into(),
            name: id.into(),
            addr: "192.168.0.42".into(),
            owner: owner.into(),
            caps: DeviceCaps::default(),
            paired_at: 1,
            push_token: "push".into(),
            bastion_token_sha256: sha256_hex(token),
        }
    }

    #[test]
    fn tokens_resolve_to_their_device_and_survive_a_reload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("devices.json");
        let registry = DeviceRegistry::load(Some(path.clone()));
        registry
            .upsert(record("bastion-4f2a", "_local", "secret-a"))
            .unwrap();
        registry
            .upsert(record("bastion-9c1d", "other", "secret-b"))
            .unwrap();
        assert_eq!(registry.by_token("secret-a").unwrap().id, "bastion-4f2a");
        assert!(registry.by_token("wrong").is_none());
        assert!(registry.by_token("").is_none());

        let reloaded = DeviceRegistry::load(Some(path.clone()));
        assert_eq!(reloaded.list("_local").len(), 1);
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(
            !raw.contains("secret-a"),
            "only the token's digest is stored"
        );

        assert!(
            !reloaded.remove("_local", "bastion-9c1d").unwrap(),
            "other owner's device"
        );
        assert!(reloaded.remove("_local", "bastion-4f2a").unwrap());
        assert!(reloaded.by_token("secret-a").is_none());
    }

    #[test]
    fn device_views_never_carry_tokens() {
        let view = serde_json::to_value(DeviceView::from(&record("d", "o", "t"))).unwrap();
        assert!(view.get("push_token").is_none());
        assert!(view.get("bastion_token_sha256").is_none());
    }

    #[test]
    fn addresses_cannot_smuggle_a_scheme_or_path() {
        assert!(validate_addr("192.168.0.42").is_ok());
        assert!(validate_addr("bastion-4f2a.local:80").is_ok());
        assert!(validate_addr("http://x").is_err());
        assert!(validate_addr("x/y").is_err());
        assert!(validate_addr("").is_err());
    }
}
