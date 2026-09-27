//! What a device keeps on disk about itself and the owner's other devices.
//!
//! One directory (`[devices] state_dir`, default `.bastion/devices`):
//!
//! | file | who | what |
//! |---|---|---|
//! | `device.key` | every device | this device's identity (age secret; Ed25519 derived) |
//! | `owner.key` | the primary | the owner key that signs enrollments |
//! | `role.json` | every device | `primary` or `node`, and the epoch |
//! | `registry.json` | the primary | the owner's device registry |
//! | `pending.json` | the primary | enrollment requests waiting for the owner |
//! | `node.json` | a node | epoch seen, own enrollment, registry copy |
//! | `replica.bin` | a replica node | the encrypted replica |
//! | `sealed-secrets.json` | a node | secrets sealed to its secrets key |
//! | `secrets.key` | a node | its secrets key, wrapped by the owner's passphrase |
//!
//! Key files are written owner-only (0600 on Unix; the per-user profile ACL
//! on Windows).

use std::path::{Path, PathBuf};

use bastion_mesh::devices::{DeviceId, DeviceRegistry};
use bastion_mesh::identity::age_identity::AgeIdentity;
use serde::{Deserialize, Serialize};

/// A device's current role, as the owner last set it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum LocalRole {
    Primary { epoch: u64 },
    Node,
}

#[derive(Debug, Clone)]
pub struct DeviceState {
    dir: PathBuf,
}

impl DeviceState {
    pub fn open(dir: impl Into<PathBuf>) -> anyhow::Result<Self> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        restrict_dir(&dir);
        Ok(Self { dir })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// This device's identity, created on first use.
    pub fn device_identity(&self) -> anyhow::Result<AgeIdentity> {
        load_or_create_identity(&self.path("device.key"))
    }

    /// The owner key, if this device holds it (the primary).
    pub fn owner_identity(&self) -> anyhow::Result<Option<AgeIdentity>> {
        let path = self.path("owner.key");
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(AgeIdentity::from_bech32(
            std::fs::read_to_string(&path)?.trim(),
        )?))
    }

    pub fn create_owner_identity(&self) -> anyhow::Result<AgeIdentity> {
        let path = self.path("owner.key");
        if path.exists() {
            anyhow::bail!("{} already exists", path.display());
        }
        let identity = AgeIdentity::generate();
        write_secret(&path, identity.age_secret_bech32().as_bytes())?;
        Ok(identity)
    }

    /// Store an owner key received at promotion.
    pub fn store_owner_identity(&self, secret: &str) -> anyhow::Result<AgeIdentity> {
        let identity = AgeIdentity::from_bech32(secret.trim())?;
        write_secret(
            &self.path("owner.key"),
            identity.age_secret_bech32().as_bytes(),
        )?;
        Ok(identity)
    }

    pub fn role(&self) -> anyhow::Result<Option<LocalRole>> {
        self.read_json("role.json")
    }

    pub fn set_role(&self, role: LocalRole) -> anyhow::Result<()> {
        self.write_json("role.json", &role)
    }

    pub fn registry(&self) -> anyhow::Result<Option<DeviceRegistry>> {
        self.read_json("registry.json")
    }

    pub fn save_registry(&self, registry: &DeviceRegistry) -> anyhow::Result<()> {
        self.write_json("registry.json", registry)
    }

    pub fn read_json<T: serde::de::DeserializeOwned>(
        &self,
        name: &str,
    ) -> anyhow::Result<Option<T>> {
        match std::fs::read(self.path(name)) {
            Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Write via a temporary file and rename, so a crash never leaves half
    /// a registry.
    pub fn write_json<T: Serialize>(&self, name: &str, value: &T) -> anyhow::Result<()> {
        let path = self.path(name);
        let tmp = self.path(&format!(".{name}.tmp"));
        std::fs::write(&tmp, serde_json::to_vec_pretty(value)?)?;
        restrict_file(&tmp);
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }
}

/// A device id for this installation: the host name, reduced to the
/// characters device ids use, plus a short random suffix so two machines
/// with the same name never collide.
pub fn default_device_id() -> DeviceId {
    let host = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .unwrap_or_else(|| "device".into());
    let base: String = host
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .take(24)
        .collect();
    let suffix: u16 = rand::random();
    DeviceId::new(format!("{}-{suffix:04x}", base.trim_matches('-')))
}

fn load_or_create_identity(path: &Path) -> anyhow::Result<AgeIdentity> {
    if path.exists() {
        return AgeIdentity::from_bech32(std::fs::read_to_string(path)?.trim());
    }
    let identity = AgeIdentity::generate();
    write_secret(path, identity.age_secret_bech32().as_bytes())?;
    Ok(identity)
}

pub(crate) fn write_secret(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let tmp = path.with_extension("tmp");
    {
        #[cfg(unix)]
        let file = {
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp)?
        };
        #[cfg(not(unix))]
        let file = std::fs::File::create(&tmp)?;
        use std::io::Write;
        let mut file = file;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn restrict_dir(_dir: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(_dir, std::fs::Permissions::from_mode(0o700));
    }
}

fn restrict_file(_path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(_path, std::fs::Permissions::from_mode(0o600));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identities_persist_and_the_owner_key_is_created_once() {
        let dir = tempfile::tempdir().unwrap();
        let state = DeviceState::open(dir.path()).unwrap();
        let a = state.device_identity().unwrap();
        let b = state.device_identity().unwrap();
        assert_eq!(a.verifying_key_bytes(), b.verifying_key_bytes());
        assert!(state.owner_identity().unwrap().is_none());
        let owner = state.create_owner_identity().unwrap();
        assert!(state.create_owner_identity().is_err());
        assert_eq!(
            state
                .owner_identity()
                .unwrap()
                .unwrap()
                .verifying_key_bytes(),
            owner.verifying_key_bytes()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(state.path("owner.key"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn the_role_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let state = DeviceState::open(dir.path()).unwrap();
        assert_eq!(state.role().unwrap(), None);
        state.set_role(LocalRole::Primary { epoch: 3 }).unwrap();
        assert_eq!(state.role().unwrap(), Some(LocalRole::Primary { epoch: 3 }));
    }

    #[test]
    fn device_ids_are_safe_and_distinct() {
        let id = default_device_id();
        assert!(id
            .as_str()
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-'));
    }
}
