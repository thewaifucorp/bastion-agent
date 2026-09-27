//! Where a device keeps the keys that protect what it holds for the owner.
//!
//! - **Replica key** (BMD-17): 32 random bytes in the system vault — the
//!   Windows Credential Manager (DPAPI-protected, per user) or the macOS
//!   Keychain. On Linux, where no vault is guaranteed to exist on a headless
//!   host, it is an owner-only file beside the state (`vault/`), which is no
//!   stronger than the file permissions; that trade-off is stated in the
//!   docs. Without the key the replica does not open.
//! - **Secrets key** (BMD-29): an age X25519 identity, wrapped with the
//!   owner's passphrase (scrypt) in `secrets.key`. It is never unwrapped
//!   while the device is a node: only [`unwrap_secrets_key`], called by the
//!   promotion command after the owner typed the passphrase on this device,
//!   can open it. On Windows the promotion also asks Windows Hello when it
//!   is set up.

use age::secrecy::{ExposeSecret, SecretString};
use zeroize::Zeroizing;

use super::state::{write_secret, DeviceState};

/// A small secret store.
pub trait Vault: Send + Sync {
    fn get(&self, name: &str) -> anyhow::Result<Option<Zeroizing<Vec<u8>>>>;
    fn put(&self, name: &str, value: &[u8]) -> anyhow::Result<()>;
    fn delete(&self, name: &str) -> anyhow::Result<()>;
}

/// The system vault (Windows Credential Manager, macOS Keychain).
#[cfg(any(windows, target_os = "macos"))]
pub struct SystemVault {
    service: String,
}

#[cfg(any(windows, target_os = "macos"))]
impl SystemVault {
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }

    fn entry(&self, name: &str) -> anyhow::Result<keyring::Entry> {
        Ok(keyring::Entry::new(&self.service, name)?)
    }
}

#[cfg(any(windows, target_os = "macos"))]
impl Vault for SystemVault {
    fn get(&self, name: &str) -> anyhow::Result<Option<Zeroizing<Vec<u8>>>> {
        match self.entry(name)?.get_secret() {
            Ok(bytes) => Ok(Some(Zeroizing::new(bytes))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn put(&self, name: &str, value: &[u8]) -> anyhow::Result<()> {
        Ok(self.entry(name)?.set_secret(value)?)
    }

    fn delete(&self, name: &str) -> anyhow::Result<()> {
        match self.entry(name)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

/// Owner-only files in a directory. The Linux default and the test vault.
pub struct FileVault {
    dir: std::path::PathBuf,
}

impl FileVault {
    pub fn new(dir: impl Into<std::path::PathBuf>) -> anyhow::Result<Self> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self { dir })
    }

    fn path(&self, name: &str) -> anyhow::Result<std::path::PathBuf> {
        if name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        {
            anyhow::bail!("invalid vault entry name {name:?}");
        }
        Ok(self.dir.join(name))
    }
}

impl Vault for FileVault {
    fn get(&self, name: &str) -> anyhow::Result<Option<Zeroizing<Vec<u8>>>> {
        match std::fs::read(self.path(name)?) {
            Ok(bytes) => Ok(Some(Zeroizing::new(bytes))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn put(&self, name: &str, value: &[u8]) -> anyhow::Result<()> {
        write_secret(&self.path(name)?, value)
    }

    fn delete(&self, name: &str) -> anyhow::Result<()> {
        match std::fs::remove_file(self.path(name)?) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

/// The vault this platform uses by default.
pub fn default_vault(state: &DeviceState) -> anyhow::Result<Box<dyn Vault>> {
    #[cfg(any(windows, target_os = "macos"))]
    {
        let _ = state;
        Ok(Box::new(SystemVault::new("bastion-devices")))
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        Ok(Box::new(FileVault::new(state.path("vault"))?))
    }
}

fn replica_entry(device: &str) -> String {
    format!("replica-key.{device}")
}

/// The replica key for `device`, created on first use.
pub fn replica_key(vault: &dyn Vault, device: &str) -> anyhow::Result<Zeroizing<[u8; 32]>> {
    let name = replica_entry(device);
    if let Some(bytes) = vault.get(&name)? {
        let key: [u8; 32] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| anyhow::anyhow!("replica key in the vault is not 32 bytes"))?;
        return Ok(Zeroizing::new(key));
    }
    let mut key = Zeroizing::new([0u8; 32]);
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut *key);
    vault.put(&name, &*key)?;
    Ok(key)
}

/// Forget the replica key (revocation): the replica becomes unreadable.
pub fn forget_replica_key(vault: &dyn Vault, device: &str) -> anyhow::Result<()> {
    vault.delete(&replica_entry(device))
}

const SECRETS_KEY_FILE: &str = "secrets.key";

/// Create this device's secrets key, wrapped with `passphrase`. Returns the
/// public recipient that goes into its enrollment.
pub fn create_secrets_key(state: &DeviceState, passphrase: &str) -> anyhow::Result<String> {
    if passphrase.chars().count() < 8 {
        anyhow::bail!("the passphrase must have at least 8 characters");
    }
    let identity = age::x25519::Identity::generate();
    let recipient = age::scrypt::Recipient::new(SecretString::from(passphrase.to_owned()));
    let wrapped = age::encrypt(&recipient, identity.to_string().expose_secret().as_bytes())
        .map_err(|e| anyhow::anyhow!("wrapping the secrets key: {e}"))?;
    write_secret(&state.path(SECRETS_KEY_FILE), &wrapped)?;
    Ok(identity.to_public().to_string())
}

pub fn has_secrets_key(state: &DeviceState) -> bool {
    state.path(SECRETS_KEY_FILE).exists()
}

/// Unwrap the secrets key with the owner's passphrase. Only the promotion
/// command calls this (BMD-29, BMD-30).
pub fn unwrap_secrets_key(
    state: &DeviceState,
    passphrase: &str,
) -> anyhow::Result<age::x25519::Identity> {
    let wrapped = std::fs::read(state.path(SECRETS_KEY_FILE))?;
    let identity = age::scrypt::Identity::new(SecretString::from(passphrase.to_owned()));
    let plain = Zeroizing::new(
        age::decrypt(&identity, &wrapped).map_err(|_| anyhow::anyhow!("wrong passphrase"))?,
    );
    std::str::from_utf8(&plain)?
        .parse()
        .map_err(|e| anyhow::anyhow!("secrets key: {e}"))
}

/// Delete the secrets key (revocation): the sealed secrets become unreadable
/// for good (BMD-33).
pub fn wipe_secrets_key(state: &DeviceState) -> anyhow::Result<()> {
    match std::fs::remove_file(state.path(SECRETS_KEY_FILE)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_replica_key_is_created_once_and_forgotten_on_revocation() {
        let dir = tempfile::tempdir().unwrap();
        let vault = FileVault::new(dir.path().join("vault")).unwrap();
        let a = replica_key(&vault, "pc").unwrap();
        let b = replica_key(&vault, "pc").unwrap();
        assert_eq!(*a, *b);
        forget_replica_key(&vault, "pc").unwrap();
        assert_ne!(*replica_key(&vault, "pc").unwrap(), *a);
        assert!(vault.get("../escape").is_err());
    }

    #[test]
    fn the_secrets_key_opens_only_with_the_passphrase() {
        let dir = tempfile::tempdir().unwrap();
        let state = DeviceState::open(dir.path()).unwrap();
        assert!(create_secrets_key(&state, "short").is_err());
        let recipient = create_secrets_key(&state, "correct horse battery").unwrap();
        let sealed = age::encrypt(
            &recipient.parse::<age::x25519::Recipient>().unwrap(),
            b"sk-canary",
        )
        .unwrap();
        let file = std::fs::read(state.path(SECRETS_KEY_FILE)).unwrap();
        assert!(!String::from_utf8_lossy(&file).contains("AGE-SECRET-KEY"));

        assert!(unwrap_secrets_key(&state, "wrong passphrase!").is_err());
        let identity = unwrap_secrets_key(&state, "correct horse battery").unwrap();
        assert_eq!(age::decrypt(&identity, &sealed).unwrap(), b"sk-canary");

        wipe_secrets_key(&state).unwrap();
        assert!(unwrap_secrets_key(&state, "correct horse battery").is_err());
    }
}
