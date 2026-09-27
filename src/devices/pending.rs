//! What the primary is waiting on: devices that asked to join, the
//! one-time pairing codes that let them ask, and which old epochs were
//! already reconciled. Kept in `pending.json`.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use bastion_mesh::devices::{DeviceId, Platform};
use rand::Rng;
use serde::{Deserialize, Serialize};

use super::state::DeviceState;

const FILE: &str = "pending.json";
/// How long a pairing code stays usable.
pub const PAIRING_TTL_SECS: i64 = 600;

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestStatus {
    Pending,
    Approved,
    Refused,
}

/// A device asking to join, and the owner's answer once given.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnrollmentRequest {
    /// Random, unguessable: the only handle the asking device polls with.
    pub id: String,
    pub device: DeviceId,
    /// Ed25519 public key, base64url (as in `AgentCard`).
    pub device_key: String,
    pub platform: Platform,
    pub holds_replica: bool,
    pub requested_at: i64,
    pub status: RequestStatus,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct File {
    requests: Vec<EnrollmentRequest>,
    /// Pairing code → expiry (unix seconds). Each works once.
    codes: HashMap<String, i64>,
    reconciled_epochs: Vec<u64>,
}

pub struct Pending {
    state: DeviceState,
    inner: Mutex<File>,
}

fn random_token(len: usize) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut rng = rand::rngs::OsRng;
    (0..len)
        .map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char)
        .collect()
}

impl Pending {
    pub fn open(state: &DeviceState) -> anyhow::Result<Self> {
        let file = state.read_json::<File>(FILE)?.unwrap_or_default();
        Ok(Self {
            state: state.clone(),
            inner: Mutex::new(file),
        })
    }

    fn with<T>(&self, change: impl FnOnce(&mut File) -> T) -> anyhow::Result<T> {
        let mut file = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let out = change(&mut file);
        self.state.write_json(FILE, &*file)?;
        Ok(out)
    }

    fn read<T>(&self, look: impl FnOnce(&File) -> T) -> T {
        look(&self.inner.lock().unwrap_or_else(|p| p.into_inner()))
    }

    /// A new one-time code (`BAST-XXXX-XXXX`) and when it expires.
    pub fn new_code(&self) -> anyhow::Result<(String, i64)> {
        let code = format!("BAST-{}-{}", random_token(4), random_token(4));
        let expires = now() + PAIRING_TTL_SECS;
        self.with(|f| {
            f.codes.retain(|_, exp| *exp > now());
            f.codes.insert(code.clone(), expires);
        })?;
        Ok((code, expires))
    }

    /// Spend `code`: true once, if it exists and has not expired.
    pub fn take_code(&self, code: &str) -> anyhow::Result<bool> {
        self.with(|f| match f.codes.remove(code) {
            Some(expires) => expires > now(),
            None => false,
        })
    }

    pub fn add_request(
        &self,
        device: DeviceId,
        device_key: String,
        platform: Platform,
        holds_replica: bool,
    ) -> anyhow::Result<EnrollmentRequest> {
        let request = EnrollmentRequest {
            id: random_token(26),
            device,
            device_key,
            platform,
            holds_replica,
            requested_at: now(),
            status: RequestStatus::Pending,
        };
        let copy = request.clone();
        self.with(|f| f.requests.push(copy))?;
        Ok(request)
    }

    pub fn request(&self, id: &str) -> Option<EnrollmentRequest> {
        self.read(|f| f.requests.iter().find(|r| r.id == id).cloned())
    }

    pub fn requests(&self) -> Vec<EnrollmentRequest> {
        self.read(|f| f.requests.clone())
    }

    pub fn set_status(&self, id: &str, status: RequestStatus) -> anyhow::Result<bool> {
        self.with(|f| match f.requests.iter_mut().find(|r| r.id == id) {
            Some(r) if r.status == RequestStatus::Pending => {
                r.status = status;
                true
            }
            _ => false,
        })
    }

    pub fn reconciled_epochs(&self) -> Vec<u64> {
        self.read(|f| f.reconciled_epochs.clone())
    }

    pub fn mark_reconciled(&self, epoch: u64) -> anyhow::Result<()> {
        self.with(|f| {
            if !f.reconciled_epochs.contains(&epoch) {
                f.reconciled_epochs.push(epoch);
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pairing_code_works_once() {
        let dir = tempfile::tempdir().unwrap();
        let pending = Pending::open(&DeviceState::open(dir.path()).unwrap()).unwrap();
        let (code, expires) = pending.new_code().unwrap();
        assert!(code.starts_with("BAST-") && expires > now());
        assert!(pending.take_code(&code).unwrap());
        assert!(!pending.take_code(&code).unwrap());
        assert!(!pending.take_code("BAST-NOPE-NOPE").unwrap());
    }

    #[test]
    fn requests_persist_and_are_decided_once() {
        let dir = tempfile::tempdir().unwrap();
        let state = DeviceState::open(dir.path()).unwrap();
        let pending = Pending::open(&state).unwrap();
        let r = pending
            .add_request(DeviceId::new("pc"), "key".into(), Platform::Windows, true)
            .unwrap();
        assert_eq!(r.id.len(), 26);
        let reopened = Pending::open(&state).unwrap();
        assert_eq!(
            reopened.request(&r.id).unwrap().status,
            RequestStatus::Pending
        );
        assert!(reopened.set_status(&r.id, RequestStatus::Approved).unwrap());
        assert!(!reopened.set_status(&r.id, RequestStatus::Refused).unwrap());
    }
}
