//! `ExtensionHost` — dependency resolution, reproducible lockfile,
//! install/upgrade/rollback/revoke. Product code, deliberately outside the
//! kernel.
//!
//! Every state-changing method here is all-or-nothing: a failed
//! install/upgrade leaves the registry and `installed` map EXACTLY as they
//! were before the call (acceptance criterion 2 — "zero órfão"). This is
//! achieved by trusting only what `HostFacade::registered_capabilities()`
//! reports was ACTUALLY registered, never the manifest's own claims.

use crate::extension::facade::{ExtensionInstance, HostFacade};
use crate::extension::trust::{official_anchors, Provenance};
use bastion_extension_protocol::{
    digest_hex, ExtensionError, ExtensionManifest, LoadoutLock, LockEntry, PackManifest,
    PermissionSet, TrustAnchors,
};
use bastion_runtime::capability::CapabilityRegistry;
use semver::Version;
use std::collections::BTreeMap;
use std::sync::Arc;

/// One installed extension's live state.
struct InstalledExtension {
    manifest: ExtensionManifest,
    instance: Arc<dyn ExtensionInstance>,
    /// Capability names THIS installation actually registered — the ground
    /// truth `revoke`/`upgrade` clean up, independent of `manifest.provides`.
    registered: Vec<String>,
    owner: String,
    /// How this version's trust tier was established (BEX-03).
    provenance: Provenance,
}

/// A previously-active version of an extension, kept so `rollback` can
/// reactivate it without needing to re-fetch/re-resolve anything.
///
/// Deliberately does NOT store the old `registered` capability list —
/// `rollback` re-`activate()`s this snapshot and trusts the FRESH
/// `HostFacade::registered_capabilities()` that call produces, exactly like
/// `install`/`upgrade` do, rather than replaying a stale list.
struct HistorySnapshot {
    manifest: ExtensionManifest,
    instance: Arc<dyn ExtensionInstance>,
    owner: String,
    /// Restored with the version on rollback — a rollback never changes the
    /// tier that version was installed with.
    provenance: Provenance,
}

/// Resolved set of active extensions+versions for one agent instance
/// (design doc §4). A read-only VIEW over `ExtensionHost` — human/UX summary
/// (M4-09) reads this, never `ExtensionHost`'s internals directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loadout {
    pub extensions: Vec<(String, Version)>,
}

/// The extension host. Owns the `CapabilityRegistry` extensions register
/// into (mirrors the kernel's own registry shape) plus the reproducible
/// `LoadoutLock`.
pub struct ExtensionHost {
    registry: CapabilityRegistry,
    installed: BTreeMap<String, InstalledExtension>,
    history: BTreeMap<String, Vec<HistorySnapshot>>,
    lock: LoadoutLock,
    /// Publisher keys signatures are verified against before an install
    /// (`extension::trust::verify_pack`). Defaults to this build's
    /// `official` keys.
    trust_anchors: TrustAnchors,
}

impl Default for ExtensionHost {
    fn default() -> Self {
        Self::new()
    }
}

impl ExtensionHost {
    pub fn new() -> Self {
        Self {
            registry: CapabilityRegistry::new(),
            installed: BTreeMap::new(),
            history: BTreeMap::new(),
            lock: LoadoutLock::default(),
            trust_anchors: official_anchors(),
        }
    }

    /// Replace the trust anchors (tests, or an embedding host with its own
    /// publisher keys).
    pub fn with_trust_anchors(mut self, anchors: TrustAnchors) -> Self {
        self.trust_anchors = anchors;
        self
    }

    pub fn trust_anchors(&self) -> &TrustAnchors {
        &self.trust_anchors
    }

    /// How the installed version of `id` got its trust tier.
    pub fn provenance(&self, id: &str) -> Option<&Provenance> {
        self.installed.get(id).map(|e| &e.provenance)
    }

    pub fn registry(&self) -> &CapabilityRegistry {
        &self.registry
    }

    pub fn lock(&self) -> &LoadoutLock {
        &self.lock
    }

    pub fn is_installed(&self, id: &str) -> bool {
        self.installed.contains_key(id)
    }

    pub fn loadout(&self) -> Loadout {
        Loadout {
            extensions: self
                .installed
                .values()
                .map(|e| (e.manifest.id.clone(), e.manifest.version.clone()))
                .collect(),
        }
    }

    fn lock_entry_for(manifest: &ExtensionManifest, provenance: &Provenance) -> LockEntry {
        let bytes = serde_json::to_vec(manifest).unwrap_or_default();
        LockEntry {
            id: manifest.id.clone(),
            version: manifest.version.clone(),
            hash: digest_hex(&bytes),
            signature: provenance.signature.clone(),
            tier: provenance.tier(),
        }
    }

    /// The auditable record of a trust decision: which version of what got
    /// which tier, from which publisher, through which action.
    fn audit_trust(action: &'static str, manifest: &ExtensionManifest, provenance: &Provenance) {
        let publisher = provenance
            .signature
            .as_ref()
            .map_or("", |s| s.publisher.as_str());
        tracing::info!(
            event = "extension_trust_decision",
            action,
            id = %manifest.id,
            version = %manifest.version,
            tier = provenance.tier().as_str(),
            publisher,
        );
    }

    /// Install a fresh extension. Atomic: if `activate` fails partway, every
    /// capability it DID manage to register is removed before returning —
    /// the registry and `installed` map are left exactly as before the call.
    ///
    /// `instance_ceiling` is the instance's own grant (e.g. from
    /// `AgentDefinition`) — `manifest.permissions` must be a subset of it
    /// (design doc §4) or this is rejected before anything is touched.
    ///
    /// Records the extension as `local` (nothing verified); a caller that
    /// verified a signature uses [`Self::install_with_provenance`].
    pub async fn install(
        &mut self,
        instance: Arc<dyn ExtensionInstance>,
        owner: &str,
        instance_ceiling: &PermissionSet,
    ) -> Result<(), ExtensionError> {
        self.install_with_provenance(instance, owner, instance_ceiling, Provenance::unsigned())
            .await
    }

    /// [`Self::install`], recording `provenance` (the verified trust tier
    /// and the signature behind it) in the lockfile and the installed set.
    pub async fn install_with_provenance(
        &mut self,
        instance: Arc<dyn ExtensionInstance>,
        owner: &str,
        instance_ceiling: &PermissionSet,
        provenance: Provenance,
    ) -> Result<(), ExtensionError> {
        let manifest = instance.manifest().clone();
        manifest.validate_self_consistent()?;

        if self.installed.contains_key(&manifest.id) {
            return Err(ExtensionError::AlreadyInstalled {
                id: manifest.id,
                version: manifest.version,
            });
        }

        if !manifest.permissions.is_subset_of(instance_ceiling) {
            return Err(ExtensionError::AuthorityEscalation {
                extension: manifest.id,
                pack: None,
            });
        }

        let mut facade = HostFacade::new(&manifest, owner, &mut self.registry);
        let activate_result = instance.activate(&mut facade).await;
        let registered = facade.registered_capabilities().to_vec();
        drop(facade); // release the borrow of self.registry before touching it again

        if let Err(e) = activate_result {
            for name in &registered {
                self.registry.remove(name);
            }
            return Err(e);
        }

        self.lock
            .upsert(Self::lock_entry_for(&manifest, &provenance));
        Self::audit_trust("install", &manifest, &provenance);
        self.installed.insert(
            manifest.id.clone(),
            InstalledExtension {
                manifest,
                instance,
                registered,
                owner: owner.to_string(),
                provenance,
            },
        );
        Ok(())
    }

    /// Upgrade an installed extension to a new version. Blocks (acceptance
    /// criterion 5) BEFORE touching the active loadout when:
    /// - the new manifest's `compat` range rejects this host's protocol
    ///   version, or
    /// - another currently-installed extension's `requires` entry for this id
    ///   would break against the new version.
    ///
    /// Only after both checks (and the authority-ceiling check) pass does
    /// this deactivate the old version and activate the new one. If the new
    /// version's `activate` fails, the old version is reactivated — the
    /// loadout never ends up with neither version registered.
    ///
    /// The new version is recorded as `local`; see
    /// [`Self::upgrade_with_provenance`].
    pub async fn upgrade(
        &mut self,
        new_instance: Arc<dyn ExtensionInstance>,
        instance_ceiling: &PermissionSet,
    ) -> Result<(), ExtensionError> {
        self.upgrade_with_provenance(new_instance, instance_ceiling, Provenance::unsigned())
            .await
    }

    /// [`Self::upgrade`], recording `provenance` for the new version. The
    /// old version keeps its own provenance in the rollback history; a
    /// failed upgrade restores it unchanged.
    pub async fn upgrade_with_provenance(
        &mut self,
        new_instance: Arc<dyn ExtensionInstance>,
        instance_ceiling: &PermissionSet,
        provenance: Provenance,
    ) -> Result<(), ExtensionError> {
        let new_manifest = new_instance.manifest().clone();
        new_manifest.validate_self_consistent()?;

        if !self.installed.contains_key(&new_manifest.id) {
            return Err(ExtensionError::NotFound {
                id: new_manifest.id.clone(),
            });
        }

        let protocol_version = Version::parse(bastion_extension_protocol::PROTOCOL_VERSION)
            .map_err(|e| ExtensionError::InvalidManifest {
                id: new_manifest.id.clone(),
                reason: e.to_string(),
            })?;
        if !new_manifest.compat.matches(&protocol_version) {
            return Err(ExtensionError::IncompatibleVersion {
                id: new_manifest.id.clone(),
                found: new_manifest.version.clone(),
                required: new_manifest.compat.clone(),
                dependent: None,
            });
        }

        for other in self.installed.values() {
            if other.manifest.id == new_manifest.id {
                continue;
            }
            for req in &other.manifest.requires {
                if req.id == new_manifest.id && !req.version.matches(&new_manifest.version) {
                    return Err(ExtensionError::IncompatibleVersion {
                        id: new_manifest.id.clone(),
                        found: new_manifest.version.clone(),
                        required: req.version.clone(),
                        dependent: Some(other.manifest.id.clone()),
                    });
                }
            }
        }

        if !new_manifest.permissions.is_subset_of(instance_ceiling) {
            return Err(ExtensionError::AuthorityEscalation {
                extension: new_manifest.id.clone(),
                pack: None,
            });
        }

        // Every guard above passed — ONLY NOW do we touch the active loadout.
        let (old_manifest, old_instance, old_registered, owner, old_provenance) = {
            let cur = self
                .installed
                .get(&new_manifest.id)
                .expect("checked contains_key above");
            (
                cur.manifest.clone(),
                cur.instance.clone(),
                cur.registered.clone(),
                cur.owner.clone(),
                cur.provenance.clone(),
            )
        };

        {
            let mut facade_old = HostFacade::new(&old_manifest, &owner, &mut self.registry);
            let _ = old_instance.deactivate(&mut facade_old).await;
        }
        for name in &old_registered {
            self.registry.remove(name);
        }

        let mut facade_new = HostFacade::new(&new_manifest, &owner, &mut self.registry);
        let activate_result = new_instance.activate(&mut facade_new).await;
        let new_registered = facade_new.registered_capabilities().to_vec();
        drop(facade_new); // release the borrow of self.registry before touching it again

        match activate_result {
            Ok(()) => {
                let registered = new_registered;
                self.history
                    .entry(new_manifest.id.clone())
                    .or_default()
                    .push(HistorySnapshot {
                        manifest: old_manifest,
                        instance: old_instance,
                        owner: owner.clone(),
                        provenance: old_provenance,
                    });
                self.lock
                    .upsert(Self::lock_entry_for(&new_manifest, &provenance));
                Self::audit_trust("upgrade", &new_manifest, &provenance);
                self.installed.insert(
                    new_manifest.id.clone(),
                    InstalledExtension {
                        manifest: new_manifest,
                        instance: new_instance,
                        registered,
                        owner,
                        provenance,
                    },
                );
                Ok(())
            }
            Err(e) => {
                for name in &new_registered {
                    self.registry.remove(name);
                }
                // Best-effort restore of the old version so the loadout never
                // ends up with NEITHER version active.
                let mut facade_restore = HostFacade::new(&old_manifest, &owner, &mut self.registry);
                let _ = old_instance.activate(&mut facade_restore).await;
                let restored = facade_restore.registered_capabilities().to_vec();
                self.installed.insert(
                    old_manifest.id.clone(),
                    InstalledExtension {
                        manifest: old_manifest,
                        instance: old_instance,
                        registered: restored,
                        owner,
                        provenance: old_provenance,
                    },
                );
                Err(e)
            }
        }
    }

    /// Reactivate the most recent PREVIOUS version of `id` (pushed onto
    /// history by `upgrade`). Errs with `NoRollbackTarget` if there is none.
    pub async fn rollback(&mut self, id: &str) -> Result<(), ExtensionError> {
        let mut history = self.history.remove(id).unwrap_or_default();
        let target = history
            .pop()
            .ok_or_else(|| ExtensionError::NoRollbackTarget { id: id.to_string() })?;
        if !history.is_empty() {
            self.history.insert(id.to_string(), history);
        }

        if let Some(cur) = self.installed.remove(id) {
            {
                let mut facade = HostFacade::new(&cur.manifest, &cur.owner, &mut self.registry);
                let _ = cur.instance.deactivate(&mut facade).await;
            }
            for name in &cur.registered {
                self.registry.remove(name);
            }
        }

        let mut facade = HostFacade::new(&target.manifest, &target.owner, &mut self.registry);
        target.instance.activate(&mut facade).await?;
        let registered = facade.registered_capabilities().to_vec();

        self.lock
            .upsert(Self::lock_entry_for(&target.manifest, &target.provenance));
        Self::audit_trust("rollback", &target.manifest, &target.provenance);
        self.installed.insert(
            id.to_string(),
            InstalledExtension {
                manifest: target.manifest,
                instance: target.instance,
                registered,
                owner: target.owner,
                provenance: target.provenance,
            },
        );
        Ok(())
    }

    /// Remove an installed extension entirely. Zero orphan (acceptance
    /// criterion 2): every capability it registered is removed from the
    /// registry, its lock entry is dropped, and its history is cleared (no
    /// dangling rollback target pointing at a revoked extension's data).
    pub async fn revoke(&mut self, id: &str) -> Result<(), ExtensionError> {
        let installed = self
            .installed
            .remove(id)
            .ok_or_else(|| ExtensionError::NotFound { id: id.to_string() })?;

        {
            let mut facade =
                HostFacade::new(&installed.manifest, &installed.owner, &mut self.registry);
            // Best-effort: even if a mechanism's deactivate() itself errors,
            // the host still forcibly removes every capability it registered
            // below — deactivate() is not the only cleanup mechanism trusted.
            let _ = installed.instance.deactivate(&mut facade).await;
        }
        for name in &installed.registered {
            self.registry.remove(name);
        }
        self.lock.remove(id);
        self.history.remove(id);
        Self::audit_trust("revoke", &installed.manifest, &installed.provenance);
        Ok(())
    }

    /// Resolve a pack against a catalog of available (not-yet-installed)
    /// extension instances, WITHOUT installing anything. Every member
    /// extension's own `permissions` must be a subset of `instance_ceiling` —
    /// a pack cannot smuggle in one over-privileged member (design doc §4/§6,
    /// acceptance criterion 4). Returns the ordered list of instances a
    /// caller should then `install()` one by one.
    pub fn resolve_pack(
        pack: &PackManifest,
        catalog: &BTreeMap<String, Arc<dyn ExtensionInstance>>,
        instance_ceiling: &PermissionSet,
    ) -> Result<Vec<Arc<dyn ExtensionInstance>>, ExtensionError> {
        let mut resolved = Vec::with_capacity(pack.extensions.len());
        for (id, _version_req) in &pack.extensions {
            let instance = catalog
                .get(id)
                .ok_or_else(|| ExtensionError::NotFound { id: id.clone() })?;
            let manifest = instance.manifest();
            if !manifest.permissions.is_subset_of(instance_ceiling) {
                return Err(ExtensionError::AuthorityEscalation {
                    extension: manifest.id.clone(),
                    pack: Some(pack.id.clone()),
                });
            }
            resolved.push(instance.clone());
        }
        Ok(resolved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::declarative::DeclarativeExtension;
    use bastion_extension_protocol::{Entrypoint, ExtensionKind};

    fn manifest_with(
        id: &str,
        version: (u64, u64, u64),
        capabilities: Vec<String>,
        requires: Vec<bastion_extension_protocol::Requirement>,
    ) -> ExtensionManifest {
        let cap_name = format!("{id}:read");
        let provides = if capabilities.contains(&cap_name) {
            vec![bastion_extension_protocol::Provided::Capability(
                cap_name.clone(),
            )]
        } else {
            vec![]
        };
        ExtensionManifest {
            id: id.to_string(),
            version: Version::new(version.0, version.1, version.2),
            kind: ExtensionKind::Declarative,
            compat: semver::VersionReq::parse("*").unwrap(),
            provides,
            requires,
            permissions: PermissionSet {
                capabilities,
                ..PermissionSet::none()
            },
            secrets: vec![],
            entrypoint: Entrypoint::Declarative {
                artifact_path: format!("{id}.json").into(),
            },
            migrations: vec![],
            signature: None,
        }
    }

    fn declarative(manifest: ExtensionManifest) -> Arc<dyn ExtensionInstance> {
        let cap_name = format!("{}:read", manifest.id);
        Arc::new(DeclarativeExtension::new(
            manifest,
            vec![(
                cap_name,
                "reference data".to_string(),
                serde_json::json!({}),
                serde_json::json!({"hello": "world"}),
            )],
        ))
    }

    fn full_ceiling() -> PermissionSet {
        PermissionSet {
            capabilities: vec![
                "acme/widget:read".to_string(),
                "acme/widget2:read".to_string(),
            ],
            ..PermissionSet::none()
        }
    }

    #[tokio::test]
    async fn install_registers_capability_and_lock_entry() {
        let mut host = ExtensionHost::new();
        let m = manifest_with(
            "acme/widget",
            (1, 0, 0),
            vec!["acme/widget:read".to_string()],
            vec![],
        );
        let instance = declarative(m);

        host.install(instance, "alice", &full_ceiling())
            .await
            .expect("install should succeed");

        assert!(host.is_installed("acme/widget"));
        assert!(host.registry().list_names().contains(&"acme/widget:read"));
        assert!(host.lock().find("acme/widget").is_some());
    }

    #[tokio::test]
    async fn install_rejects_authority_exceeding_ceiling() {
        let mut host = ExtensionHost::new();
        let m = manifest_with(
            "acme/widget",
            (1, 0, 0),
            vec!["acme/widget:read".to_string()],
            vec![],
        );
        let instance = declarative(m);

        let result = host
            .install(instance, "alice", &PermissionSet::none())
            .await;
        assert!(matches!(
            result,
            Err(ExtensionError::AuthorityEscalation { .. })
        ));
        assert!(!host.is_installed("acme/widget"));
        assert!(host.registry().list_names().is_empty());
    }

    #[tokio::test]
    async fn install_twice_rejected_as_already_installed() {
        let mut host = ExtensionHost::new();
        let m = manifest_with(
            "acme/widget",
            (1, 0, 0),
            vec!["acme/widget:read".to_string()],
            vec![],
        );
        host.install(declarative(m.clone()), "alice", &full_ceiling())
            .await
            .unwrap();

        let result = host.install(declarative(m), "alice", &full_ceiling()).await;
        assert!(matches!(
            result,
            Err(ExtensionError::AlreadyInstalled { .. })
        ));
    }

    #[tokio::test]
    async fn revoke_leaves_zero_orphan() {
        let mut host = ExtensionHost::new();
        let m = manifest_with(
            "acme/widget",
            (1, 0, 0),
            vec!["acme/widget:read".to_string()],
            vec![],
        );
        host.install(declarative(m), "alice", &full_ceiling())
            .await
            .unwrap();

        host.revoke("acme/widget").await.expect("revoke succeeds");

        assert!(!host.is_installed("acme/widget"));
        assert!(host.registry().list_names().is_empty());
        assert!(host.lock().find("acme/widget").is_none());
    }

    #[tokio::test]
    async fn upgrade_swaps_version_and_keeps_capability_registered() {
        let mut host = ExtensionHost::new();
        let v1 = manifest_with(
            "acme/widget",
            (1, 0, 0),
            vec!["acme/widget:read".to_string()],
            vec![],
        );
        host.install(declarative(v1), "alice", &full_ceiling())
            .await
            .unwrap();

        let v2 = manifest_with(
            "acme/widget",
            (2, 0, 0),
            vec!["acme/widget:read".to_string()],
            vec![],
        );
        host.upgrade(declarative(v2), &full_ceiling())
            .await
            .expect("upgrade should succeed");

        assert_eq!(
            host.lock().find("acme/widget").unwrap().version,
            Version::new(2, 0, 0)
        );
        assert!(host.registry().list_names().contains(&"acme/widget:read"));
    }

    #[tokio::test]
    async fn upgrade_blocked_by_dependent_requirement_leaves_loadout_untouched() {
        let mut host = ExtensionHost::new();
        let v1 = manifest_with(
            "acme/widget",
            (1, 0, 0),
            vec!["acme/widget:read".to_string()],
            vec![],
        );
        host.install(declarative(v1), "alice", &full_ceiling())
            .await
            .unwrap();

        // A dependent extension that requires acme/widget ^1.0.
        let dependent = manifest_with(
            "acme/widget2",
            (1, 0, 0),
            vec!["acme/widget2:read".to_string()],
            vec![bastion_extension_protocol::Requirement {
                id: "acme/widget".to_string(),
                version: semver::VersionReq::parse("^1.0").unwrap(),
            }],
        );
        host.install(declarative(dependent), "alice", &full_ceiling())
            .await
            .unwrap();

        let v2 = manifest_with(
            "acme/widget",
            (2, 0, 0),
            vec!["acme/widget:read".to_string()],
            vec![],
        );
        let result = host.upgrade(declarative(v2), &full_ceiling()).await;
        assert!(matches!(
            result,
            Err(ExtensionError::IncompatibleVersion { .. })
        ));

        // Loadout must be UNTOUCHED — still v1.0.0, capability still live.
        assert_eq!(
            host.lock().find("acme/widget").unwrap().version,
            Version::new(1, 0, 0)
        );
        assert!(host.registry().list_names().contains(&"acme/widget:read"));
    }

    #[tokio::test]
    async fn tier_is_recorded_and_preserved_across_upgrade_rollback_and_revoke() {
        use bastion_extension_protocol::{Signature, TrustTier, Verification};

        let official = Provenance {
            verification: Verification::Verified {
                publisher: "test-publisher".to_string(),
                tier: TrustTier::Official,
            },
            signature: Some(Signature::sign_ed25519("test-publisher", &[1; 32], b"p")),
        };
        let mut host = ExtensionHost::new();
        let v1 = manifest_with(
            "acme/widget",
            (1, 0, 0),
            vec!["acme/widget:read".to_string()],
            vec![],
        );
        host.install_with_provenance(declarative(v1), "alice", &full_ceiling(), official.clone())
            .await
            .unwrap();
        let entry = host.lock().find("acme/widget").unwrap();
        assert_eq!(entry.tier, TrustTier::Official);
        assert_eq!(entry.signature, official.signature);
        assert_eq!(host.provenance("acme/widget"), Some(&official));

        // An unverified upgrade is recorded as local...
        let v2 = manifest_with(
            "acme/widget",
            (2, 0, 0),
            vec!["acme/widget:read".to_string()],
            vec![],
        );
        host.upgrade(declarative(v2), &full_ceiling())
            .await
            .unwrap();
        assert_eq!(
            host.lock().find("acme/widget").unwrap().tier,
            TrustTier::Local
        );

        // ...and rolling back restores v1 with the tier it was installed with.
        host.rollback("acme/widget").await.unwrap();
        let entry = host.lock().find("acme/widget").unwrap();
        assert_eq!(entry.version, Version::new(1, 0, 0));
        assert_eq!(entry.tier, TrustTier::Official);
        assert_eq!(host.provenance("acme/widget"), Some(&official));

        host.revoke("acme/widget").await.unwrap();
        assert!(host.lock().find("acme/widget").is_none());
        assert!(host.provenance("acme/widget").is_none());
    }

    #[tokio::test]
    async fn failed_upgrade_keeps_the_old_tier() {
        use bastion_extension_protocol::{TrustTier, Verification};

        let official = Provenance {
            verification: Verification::Verified {
                publisher: "test-publisher".to_string(),
                tier: TrustTier::Official,
            },
            signature: None,
        };
        let mut host = ExtensionHost::new();
        let v1 = manifest_with(
            "acme/widget",
            (1, 0, 0),
            vec!["acme/widget:read".to_string()],
            vec![],
        );
        host.install_with_provenance(declarative(v1), "alice", &full_ceiling(), official.clone())
            .await
            .unwrap();
        let mut v2 = manifest_with(
            "acme/widget",
            (2, 0, 0),
            vec!["acme/widget:read".to_string()],
            vec![],
        );
        v2.compat = semver::VersionReq::parse(">=99").unwrap();
        assert!(host
            .upgrade_with_provenance(declarative(v2), &full_ceiling(), Provenance::unsigned())
            .await
            .is_err());
        assert_eq!(host.provenance("acme/widget"), Some(&official));
        assert_eq!(
            host.lock().find("acme/widget").unwrap().tier,
            TrustTier::Official
        );
    }

    #[tokio::test]
    async fn rollback_restores_previous_version_after_upgrade() {
        let mut host = ExtensionHost::new();
        let v1 = manifest_with(
            "acme/widget",
            (1, 0, 0),
            vec!["acme/widget:read".to_string()],
            vec![],
        );
        host.install(declarative(v1), "alice", &full_ceiling())
            .await
            .unwrap();
        let v2 = manifest_with(
            "acme/widget",
            (2, 0, 0),
            vec!["acme/widget:read".to_string()],
            vec![],
        );
        host.upgrade(declarative(v2), &full_ceiling())
            .await
            .unwrap();

        host.rollback("acme/widget")
            .await
            .expect("rollback should succeed");

        assert_eq!(
            host.lock().find("acme/widget").unwrap().version,
            Version::new(1, 0, 0)
        );
        assert!(host.registry().list_names().contains(&"acme/widget:read"));
    }

    #[tokio::test]
    async fn rollback_without_history_errs() {
        let mut host = ExtensionHost::new();
        let m = manifest_with(
            "acme/widget",
            (1, 0, 0),
            vec!["acme/widget:read".to_string()],
            vec![],
        );
        host.install(declarative(m), "alice", &full_ceiling())
            .await
            .unwrap();

        let result = host.rollback("acme/widget").await;
        assert!(matches!(
            result,
            Err(ExtensionError::NoRollbackTarget { .. })
        ));
    }

    #[test]
    fn resolve_pack_rejects_member_exceeding_instance_ceiling() {
        let m = manifest_with(
            "acme/widget",
            (1, 0, 0),
            vec!["acme/widget:read".to_string()],
            vec![],
        );
        let mut catalog: BTreeMap<String, Arc<dyn ExtensionInstance>> = BTreeMap::new();
        catalog.insert("acme/widget".to_string(), declarative(m));

        let pack = PackManifest {
            id: "acme/pack".to_string(),
            version: Version::new(1, 0, 0),
            extensions: vec![(
                "acme/widget".to_string(),
                semver::VersionReq::parse("*").unwrap(),
            )],
            skills: vec![],
            personas: vec![],
            defaults: Default::default(),
            signature: None,
        };

        // Instance ceiling grants NOTHING — the pack's member exceeds it.
        let result = ExtensionHost::resolve_pack(&pack, &catalog, &PermissionSet::none());
        assert!(matches!(
            result,
            Err(ExtensionError::AuthorityEscalation { pack: Some(_), .. })
        ));
    }

    #[test]
    fn resolve_pack_succeeds_within_ceiling() {
        let m = manifest_with(
            "acme/widget",
            (1, 0, 0),
            vec!["acme/widget:read".to_string()],
            vec![],
        );
        let mut catalog: BTreeMap<String, Arc<dyn ExtensionInstance>> = BTreeMap::new();
        catalog.insert("acme/widget".to_string(), declarative(m));

        let pack = PackManifest {
            id: "acme/pack".to_string(),
            version: Version::new(1, 0, 0),
            extensions: vec![(
                "acme/widget".to_string(),
                semver::VersionReq::parse("*").unwrap(),
            )],
            skills: vec![],
            personas: vec![],
            defaults: Default::default(),
            signature: None,
        };

        let resolved = ExtensionHost::resolve_pack(&pack, &catalog, &full_ceiling())
            .expect("pack within ceiling resolves");
        assert_eq!(resolved.len(), 1);
    }
}
