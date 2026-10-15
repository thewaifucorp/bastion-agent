//! Trust tier of an installed pack (BEX-03): the `official` publisher keys
//! embedded in this build, detached-signature loading, and the
//! [`Provenance`] every install/upgrade/rollback records.
//!
//! The keys live in `keys/official.pub` at the repository root and are
//! compiled in (`include_str!`); `bastion-core` embeds none. A pack's
//! detached signature (`pack.toml.sig`) covers
//! every file in the pack directory, member extensions included, so each
//! member inherits the pack's verified tier. Outcomes, decided by
//! `bastion_extension_protocol::TrustAnchors::verify`:
//!
//! - no signature → `local`;
//! - a publisher this build has no key for → `local`, shown as unverified;
//! - a signature that verifies → that key's tier (`official` here);
//! - a signature that is malformed or does not verify → the install is
//!   refused before anything is copied or activated.
//!
//! The tier is informational: enforcement (`PermissionSet`, `HostFacade`)
//! never reads it.

use std::path::Path;

use bastion_extension_protocol::{
    ContentDigests, ExtensionError, PackManifest, Signature, TrustAnchors, TrustTier, Verification,
    SIGNATURE_FILE_SUFFIX,
};
use serde::{Deserialize, Serialize};

/// `official` publisher keys, one `<publisher> ed25519 <base64>` line each.
const OFFICIAL_KEYS: &str = include_str!("../../keys/official.pub");

/// The trust anchors this build ships: every key in `keys/official.pub`,
/// granting [`TrustTier::Official`]. A malformed key file is a release bug;
/// it is logged and treated as "no official key" (everything resolves to
/// `local`), never as a reason to trust more.
pub fn official_anchors() -> TrustAnchors {
    let mut anchors = TrustAnchors::new();
    if let Err(e) = anchors.add_key_file(TrustTier::Official, OFFICIAL_KEYS) {
        tracing::error!(event = "official_trust_anchors_invalid", error = %e);
        return TrustAnchors::new();
    }
    anchors
}

/// How an installed extension's tier was established — recorded in the
/// lockfile and the persisted loadout so upgrade/rollback/restart keep it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    /// Outcome of verifying the signature that covered this extension.
    pub verification: Verification,
    /// That signature (the pack's, for a pack member). `None` when unsigned.
    pub signature: Option<Signature>,
}

impl Provenance {
    /// Nothing was verified: `local`.
    pub fn unsigned() -> Self {
        Self {
            verification: Verification::Unsigned,
            signature: None,
        }
    }

    /// Tier granted by the verification.
    pub fn tier(&self) -> TrustTier {
        self.verification.tier()
    }

    /// Owner-facing one-liner for the permission review and `/extension list`.
    pub fn label(&self) -> String {
        match &self.verification {
            Verification::Unsigned => "local (unsigned — reviewed only by you)".to_string(),
            Verification::UnknownPublisher { publisher } => format!(
                "local (signed by unknown publisher '{publisher}' — not verified, reviewed only \
                 by you)"
            ),
            Verification::Verified { publisher, tier } => {
                format!(
                    "{} (signature verified — publisher '{publisher}')",
                    tier.as_str()
                )
            }
        }
    }
}

/// Reads `<manifest_path>.sig` (TOML: `publisher`, `algorithm`, `value`).
/// `Ok(None)` when the file does not exist; a file that exists but does not
/// parse is an invalid signature, never "unsigned".
fn read_detached_signature(
    manifest_path: &Path,
    subject_id: &str,
) -> Result<Option<Signature>, ExtensionError> {
    let mut sig_path = manifest_path.as_os_str().to_owned();
    sig_path.push(SIGNATURE_FILE_SUFFIX);
    let sig_path = Path::new(&sig_path);
    let raw = match std::fs::read_to_string(sig_path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(ExtensionError::ContentUnreadable {
                path: sig_path.display().to_string(),
                detail: e.to_string(),
            })
        }
    };
    toml::from_str(&raw)
        .map(Some)
        .map_err(|e| ExtensionError::SignatureInvalid {
            id: subject_id.to_string(),
            reason: format!("{} does not parse: {e}", sig_path.display()),
        })
}

/// Verify the pack at `pack_dir` (whose `pack.toml` parsed into `pack`)
/// against `anchors`. The signature is the detached `pack.toml.sig`: an
/// inline `[signature]` table is refused, since it would have to cover the
/// very file it lives in. Every file in `pack_dir` is digested (off the
/// async runtime) and checked against it.
pub async fn verify_pack(
    pack_dir: &Path,
    pack: &PackManifest,
    anchors: &TrustAnchors,
) -> Result<Provenance, ExtensionError> {
    if pack.signature.is_some() {
        return Err(ExtensionError::SignatureInvalid {
            id: pack.id.clone(),
            reason: "an inline [signature] in pack.toml cannot cover pack.toml itself; \
                     ship it as pack.toml.sig"
                .to_string(),
        });
    }
    let Some(signature) = read_detached_signature(&pack_dir.join("pack.toml"), &pack.id)? else {
        return Ok(Provenance::unsigned());
    };

    let dir = pack_dir.to_path_buf();
    let content = tokio::task::spawn_blocking(move || ContentDigests::from_dir(&dir))
        .await
        .map_err(|e| ExtensionError::ContentUnreadable {
            path: pack_dir.display().to_string(),
            detail: e.to_string(),
        })??;

    let mut signed = pack.clone();
    signed.signature = Some(signature.clone());
    let verification = signed.verify_signature(&content, anchors)?;
    Ok(Provenance {
        signature: Some(signature),
        verification,
    })
}

#[cfg(test)]
pub(crate) mod test_keys {
    //! A throwaway TEST key pair (fixed seed). Never a real publisher key.
    use super::*;

    pub const TEST_PUBLISHER: &str = "test-publisher";
    pub const TEST_SEED: [u8; 32] = [0x42; 32];

    pub fn test_anchors() -> TrustAnchors {
        let mut anchors = TrustAnchors::new();
        anchors
            .add_key_file(
                TrustTier::Official,
                &format!(
                    "{TEST_PUBLISHER} ed25519 {}",
                    bastion_extension_protocol::public_key_base64(&TEST_SEED)
                ),
            )
            .unwrap();
        anchors
    }

    /// Writes `pack.toml.sig` for the pack in `dir`, signed with `seed`.
    pub fn sign_pack_dir(dir: &Path, publisher: &str, seed: &[u8; 32]) {
        let raw = std::fs::read_to_string(dir.join("pack.toml")).unwrap();
        let pack: PackManifest = toml::from_str(&raw).unwrap();
        let content = ContentDigests::from_dir(dir).unwrap();
        let sig =
            Signature::sign_ed25519(publisher, seed, &pack.signing_payload(&content).unwrap());
        std::fs::write(dir.join("pack.toml.sig"), toml::to_string(&sig).unwrap()).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::test_keys::*;
    use super::*;

    const PACK_TOML: &str = r#"
id = "acme/pack"
version = "1.0.0"
extensions = []
skills = []
personas = []

[defaults]
enabled_extensions = []
"#;

    fn pack_dir() -> (tempfile::TempDir, PackManifest) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("pack.toml"), PACK_TOML).unwrap();
        std::fs::create_dir_all(dir.path().join("skills/a")).unwrap();
        std::fs::write(dir.path().join("skills/a/SKILL.md"), "# a").unwrap();
        (dir, toml::from_str(PACK_TOML).unwrap())
    }

    #[test]
    fn embedded_official_key_file_parses() {
        TrustAnchors::new()
            .add_key_file(TrustTier::Official, OFFICIAL_KEYS)
            .expect("keys/official.pub must always parse");
    }

    #[tokio::test]
    async fn unsigned_pack_is_local() {
        let (dir, pack) = pack_dir();
        let p = verify_pack(dir.path(), &pack, &test_anchors())
            .await
            .unwrap();
        assert_eq!(p, Provenance::unsigned());
        assert_eq!(p.tier(), TrustTier::Local);
    }

    #[tokio::test]
    async fn signed_pack_is_official_and_records_the_signature() {
        let (dir, pack) = pack_dir();
        sign_pack_dir(dir.path(), TEST_PUBLISHER, &TEST_SEED);
        let p = verify_pack(dir.path(), &pack, &test_anchors())
            .await
            .unwrap();
        assert_eq!(p.tier(), TrustTier::Official);
        assert!(p.signature.is_some());
        assert!(p.label().starts_with("official (signature verified"));
    }

    #[tokio::test]
    async fn editing_any_pack_file_after_signing_is_refused() {
        let (dir, pack) = pack_dir();
        sign_pack_dir(dir.path(), TEST_PUBLISHER, &TEST_SEED);
        std::fs::write(dir.path().join("skills/a/SKILL.md"), "# a, edited").unwrap();
        let err = verify_pack(dir.path(), &pack, &test_anchors())
            .await
            .unwrap_err();
        assert!(matches!(err, ExtensionError::SignatureInvalid { .. }));
    }

    #[tokio::test]
    async fn adding_a_file_after_signing_is_refused() {
        let (dir, pack) = pack_dir();
        sign_pack_dir(dir.path(), TEST_PUBLISHER, &TEST_SEED);
        std::fs::write(dir.path().join("extra.md"), "smuggled").unwrap();
        assert!(verify_pack(dir.path(), &pack, &test_anchors())
            .await
            .is_err());
    }

    #[tokio::test]
    async fn signature_by_another_key_of_the_official_publisher_is_refused() {
        let (dir, pack) = pack_dir();
        sign_pack_dir(dir.path(), TEST_PUBLISHER, &[0x01; 32]);
        assert!(verify_pack(dir.path(), &pack, &test_anchors())
            .await
            .is_err());
    }

    #[tokio::test]
    async fn unknown_publisher_is_local_and_labelled_unverified() {
        let (dir, pack) = pack_dir();
        sign_pack_dir(dir.path(), "someone-else", &[0x01; 32]);
        let p = verify_pack(dir.path(), &pack, &test_anchors())
            .await
            .unwrap();
        assert_eq!(p.tier(), TrustTier::Local);
        assert!(p.label().contains("unknown publisher 'someone-else'"));
    }

    #[tokio::test]
    async fn malformed_signature_file_is_refused_not_ignored() {
        let (dir, pack) = pack_dir();
        std::fs::write(dir.path().join("pack.toml.sig"), "not = [toml").unwrap();
        assert!(matches!(
            verify_pack(dir.path(), &pack, &test_anchors()).await,
            Err(ExtensionError::SignatureInvalid { .. })
        ));
    }

    #[tokio::test]
    async fn inline_pack_signature_is_refused() {
        let (dir, mut pack) = pack_dir();
        pack.signature = Some(Signature::sign_ed25519(TEST_PUBLISHER, &TEST_SEED, b"x"));
        assert!(verify_pack(dir.path(), &pack, &test_anchors())
            .await
            .is_err());
    }
}
