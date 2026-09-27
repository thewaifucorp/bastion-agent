//! Keeping a node's dormant secrets in sync (§5.7, BMD-18, BMD-29..33).
//!
//! The owner grants a secret to a device; the primary seals that secret's
//! current value to the device's secrets key (age) and sends it. The node
//! stores only ciphertext ([`SealedSink`]); it cannot open it while it is a
//! node — the private half of its secrets key stays wrapped under the owner's
//! passphrase and is unwrapped only at promotion ([`install_on_promote`]).
//!
//! An event never carries a secret value (that invariant lives in the Core
//! event log); secrets travel on this separate, per-device sealed channel.

use std::sync::Arc;

use async_trait::async_trait;
use bastion_mesh::devices::secrets::{
    seal_for_device, SealedSecret, SealedSecretStore, SecretValue,
};
use bastion_mesh::devices::{DeviceId, DeviceRecord, SecretSink};
use bastion_types::SecretResolver;
use zeroize::Zeroizing;

use super::state::DeviceState;
use super::vault;

/// The node side: store the sealed secrets the primary sends, ciphertext only.
pub struct SealedSink {
    store: SealedSecretStore,
}

impl SealedSink {
    pub fn new(state: &DeviceState) -> Self {
        Self {
            store: SealedSecretStore::new(state.path("sealed-secrets.json")),
        }
    }
}

#[async_trait]
impl SecretSink for SealedSink {
    async fn replace(&self, secrets: Vec<SealedSecret>) -> anyhow::Result<()> {
        self.store.replace(&secrets)
    }
}

/// Seal exactly what `record` is allowed to keep, at each secret's current
/// value from the daemon's resolver. A device with no secrets key, revoked,
/// or without grants yields an empty vec — nothing is sent.
pub fn seal_for(
    record: &DeviceRecord,
    resolver: &Arc<dyn SecretResolver>,
) -> anyhow::Result<Vec<SealedSecret>> {
    seal_for_device(record, |name| {
        // Rotation (BMD-31): the value is read fresh here, so each reseal
        // (on grant and on reconnect) ships the current version. The version
        // is a content hash so an unchanged value re-seals identically.
        resolver.resolve(name).ok().map(|value| {
            let bytes = value.expose_secret().as_bytes().to_vec();
            SecretValue {
                version: content_version(&bytes),
                value: Zeroizing::new(bytes),
            }
        })
    })
}

fn content_version(bytes: &[u8]) -> u64 {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    let mut v = [0u8; 8];
    v.copy_from_slice(&digest[..8]);
    u64::from_be_bytes(v)
}

/// At promotion, unwrap this device's secrets key with the owner's passphrase
/// and write the secrets it held into `dir` (one file per secret, owner-only),
/// the shape `BASTION_SECRETS_DIR` reads. Returns the names installed.
///
/// This is the ONLY path that opens a sealed secret (BMD-29): it runs locally,
/// on the device being promoted, with the owner present to type the
/// passphrase — never while the device is a node (BMD-30).
pub fn install_on_promote(
    state: &DeviceState,
    passphrase: &str,
    dir: &std::path::Path,
) -> anyhow::Result<Vec<String>> {
    if !vault::has_secrets_key(state) {
        return Ok(Vec::new());
    }
    let identity = vault::unwrap_secrets_key(state, passphrase)?;
    let sealed = SealedSecretStore::new(state.path("sealed-secrets.json")).load()?;
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let mut installed = Vec::new();
    for secret in &sealed {
        let plain = bastion_mesh::devices::secrets::open(&identity, secret)?;
        super::state::write_secret(&dir.join(&secret.name), &plain)?;
        installed.push(secret.name.clone());
    }
    Ok(installed)
}

/// Delete a node's sealed secrets and its secrets key (revocation, BMD-33).
pub fn wipe(state: &DeviceState) -> anyhow::Result<()> {
    SealedSecretStore::new(state.path("sealed-secrets.json")).wipe()?;
    vault::wipe_secrets_key(state)?;
    Ok(())
}

/// The device ids a secret is currently granted to, for the primary to reseal
/// on rotation.
pub fn devices_with_secret<'a>(
    records: impl Iterator<Item = &'a DeviceRecord>,
    secret: &str,
) -> Vec<DeviceId> {
    records
        .filter(|r| !r.revoked)
        .filter(|r| r.secret_grants.iter().any(|g| g.secret == secret))
        .map(|r| r.enrollment.device.clone())
        .collect()
}
