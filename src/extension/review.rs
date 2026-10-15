//! Human-readable permission summary shown at install time (design doc
//! §1.3/§3, M4-09: "resumo humano de permissões na instalação"). A pure
//! formatting function — no UI framework opinion, so any product surface
//! (cockpit command today, a future GUI) can render it without this module
//! knowing about either.
//!
//! The trust line comes from the [`Provenance`] the install flow verified
//! (`extension::trust::verify_pack`, BEX-03) — never from the manifest's own
//! `signature` field, which proves nothing until checked. Trust tier is
//! INFORMATIONAL ONLY (design doc §1.3): this module never feeds it back into
//! enforcement.

use crate::extension::trust::Provenance;
use bastion_extension_protocol::ExtensionManifest;

/// Renders the manifest's declared authority as owner-facing text. Every
/// line here corresponds 1:1 to a field `PermissionSet`/`HostFacade` actually
/// enforces — this is a REVIEW of real authority, not marketing copy.
pub fn permission_summary(manifest: &ExtensionManifest, provenance: &Provenance) -> String {
    let p = &manifest.permissions;
    let tier_label = provenance.label();

    let capabilities = if p.capabilities.is_empty() {
        "(none)".to_string()
    } else {
        p.capabilities.join(", ")
    };

    format!(
        "Extension: {id} v{version}\n\
         Trust tier: {tier_label}\n\
         Permissions requested:\n\
         \u{20}\u{20}capabilities: {capabilities}\n\
         \u{20}\u{20}egress: {egress:?}\n\
         \u{20}\u{20}filesystem: {filesystem:?}\n\
         \u{20}\u{20}devices: {devices:?}\n\
         \u{20}\u{20}network_bind: {network_bind}\n\
         \u{20}\u{20}memory: {memory:?}",
        id = manifest.id,
        version = manifest.version,
        egress = p.egress,
        filesystem = p.filesystem,
        devices = p.devices,
        network_bind = p.network_bind,
        memory = p.memory_scope,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use bastion_extension_protocol::{
        Entrypoint, ExtensionKind, PermissionSet, Signature, TrustTier, Verification,
    };

    fn manifest(signature: Option<Signature>) -> ExtensionManifest {
        ExtensionManifest {
            id: "acme/widget".to_string(),
            version: semver::Version::new(1, 2, 3),
            kind: ExtensionKind::Declarative,
            compat: semver::VersionReq::parse("*").unwrap(),
            provides: vec![],
            requires: vec![],
            permissions: PermissionSet {
                capabilities: vec!["acme/widget:read".to_string()],
                ..PermissionSet::none()
            },
            secrets: vec![],
            entrypoint: Entrypoint::Declarative {
                artifact_path: "widget.json".into(),
            },
            migrations: vec![],
            signature,
        }
    }

    #[test]
    fn summary_reflects_declared_capabilities_and_local_tier() {
        let summary = permission_summary(&manifest(None), &Provenance::unsigned());
        assert!(summary.contains("acme/widget v1.2.3"));
        assert!(summary.contains("local (unsigned"));
        assert!(summary.contains("acme/widget:read"));
    }

    #[test]
    fn summary_shows_the_verified_tier_and_publisher() {
        let provenance = Provenance {
            verification: Verification::Verified {
                publisher: "test-publisher".to_string(),
                tier: TrustTier::Official,
            },
            signature: None,
        };
        let summary = permission_summary(&manifest(None), &provenance);
        assert!(summary
            .contains("Trust tier: official (signature verified — publisher 'test-publisher')"));
    }

    #[test]
    fn an_unchecked_signature_field_never_raises_the_tier() {
        let claimed = Signature {
            publisher: "acme".to_string(),
            algorithm: "ed25519".to_string(),
            value: "not-actually-verified".to_string(),
        };
        let summary = permission_summary(&manifest(Some(claimed)), &Provenance::unsigned());
        assert!(summary.contains("Trust tier: local (unsigned"));
    }
}
