//! Human-readable permission summary shown at install time (design doc
//! §1.3/§3, M4-09: "resumo humano de permissões na instalação"). A pure
//! formatting function — no UI framework opinion, so any product surface
//! (cockpit command today, a future GUI) can render it without this module
//! knowing about either.
//!
//! Trust tier is INFORMATIONAL ONLY (design doc §1.3) — this module never
//! feeds it back into enforcement, and `trust_tier_of` is deliberately
//! conservative: it returns [`TrustTier::Local`] for every manifest this
//! cycle. Verifying a REAL publisher signature needs a trust-anchor/
//! key-distribution story that's out of scope this loop (§7: no registry/
//! catalog yet) — claiming a higher tier off an unverified `signature` field
//! would be worse than not showing one. Wiring real verification (the
//! workspace already depends on `ed25519-dalek` for Agent Card signing) is a
//! documented follow-up, not a silent gap.

use bastion_extension_protocol::{ExtensionManifest, TrustTier};

/// Always `Local` this cycle — see module docs for why.
pub fn trust_tier_of(manifest: &ExtensionManifest) -> TrustTier {
    let _ = manifest; // signature presence is not, by itself, verification
    TrustTier::without_signature()
}

/// Renders the manifest's declared authority as owner-facing text. Every
/// line here corresponds 1:1 to a field `PermissionSet`/`HostFacade` actually
/// enforces — this is a REVIEW of real authority, not marketing copy.
pub fn permission_summary(manifest: &ExtensionManifest) -> String {
    let p = &manifest.permissions;
    let tier = trust_tier_of(manifest);
    let tier_label = match tier {
        TrustTier::Local => "local (unsigned — reviewed only by you)",
        TrustTier::Community => "community",
        TrustTier::Verified => "verified",
        TrustTier::Official => "official",
    };

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
    use bastion_extension_protocol::{Entrypoint, ExtensionKind, PermissionSet};

    #[test]
    fn summary_reflects_declared_capabilities_and_local_tier() {
        let manifest = ExtensionManifest {
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
            signature: None,
        };

        let summary = permission_summary(&manifest);
        assert!(summary.contains("acme/widget v1.2.3"));
        assert!(summary.contains("local (unsigned"));
        assert!(summary.contains("acme/widget:read"));
    }

    #[test]
    fn trust_tier_is_always_local_without_real_verification() {
        let manifest = ExtensionManifest {
            id: "acme/widget".to_string(),
            version: semver::Version::new(1, 0, 0),
            kind: ExtensionKind::Declarative,
            compat: semver::VersionReq::parse("*").unwrap(),
            provides: vec![],
            requires: vec![],
            permissions: PermissionSet::none(),
            secrets: vec![],
            entrypoint: Entrypoint::Declarative {
                artifact_path: "widget.json".into(),
            },
            migrations: vec![],
            signature: Some(bastion_extension_protocol::Signature {
                publisher: "acme".to_string(),
                algorithm: "ed25519".to_string(),
                value: "not-actually-verified".to_string(),
            }),
        };
        assert_eq!(trust_tier_of(&manifest), TrustTier::Local);
    }
}
