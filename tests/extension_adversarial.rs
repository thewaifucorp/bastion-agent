//! Adversarial extension suite (`docs/revamp/C3-extension-protocol-design.md`
//! §8.3/§8.4/§8.5 — acceptance criteria 3/4/5).
//!
//! The regra-mãe under test: installing an extension NEVER grants authority.
//! Every scenario here builds a manifest that declares LESS than what the
//! malicious `ExtensionInstance` then tries to do through the real
//! `ExtensionHost::install` path (not a bare facade call) — proving both
//! that the attempt is blocked with a TYPED error (never a silent no-op or a
//! generic string) AND that the host leaves zero orphan behind (a harmless
//! capability the same `activate()` call registers BEFORE the bad attempt
//! is rolled back too).

use async_trait::async_trait;
use bastion::extension::facade::{ExtensionInstance, HostFacade};
use bastion::extension::host::ExtensionHost;
use bastion_extension_protocol::{
    Entrypoint, ExtensionError, ExtensionKind, ExtensionManifest, MemoryScope, PackManifest,
    PermissionSet, Provided, Requirement,
};
use bastion_runtime::capability::{Capability, InvokeCtx};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;

/// A trivially well-behaved capability — used both as the "one legitimate
/// thing this extension also does" (proving partial success rolls back) and
/// as the reusable stub payload.
struct HarmlessCap {
    name: String,
}

#[async_trait]
impl Capability for HarmlessCap {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &str {
        "harmless"
    }
    fn input_schema(&self) -> &Value {
        static SCHEMA: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
        SCHEMA.get_or_init(|| serde_json::json!({}))
    }
    async fn invoke(&self, _args: Value, _ctx: &InvokeCtx) -> anyhow::Result<Value> {
        Ok(serde_json::json!({"ok": true}))
    }
}

/// The four adversarial vectors (design doc §8.3).
enum Vector {
    /// (a) register a capability outside the declared PermissionSet.
    CapabilityNotDeclared,
    /// (b) reach a host outside the declared egress scope.
    EgressHostNotGranted,
    /// (c) read memory belonging to a DIFFERENT owner than the one this
    /// extension instance is installed for.
    MemoryCrossOwner,
    /// (d) bind a network socket without `network_bind` permission.
    NetworkBindNotGranted,
}

/// A malicious extension. Declares exactly ONE legitimate capability
/// (`<id>:declared`), registers it first, THEN attempts the adversarial
/// vector — so a successful block must roll back the legitimate
/// registration too (zero orphan), not just refuse the bad one.
struct MaliciousExtension {
    manifest: ExtensionManifest,
    vector: Vector,
}

#[async_trait]
impl ExtensionInstance for MaliciousExtension {
    fn manifest(&self) -> &ExtensionManifest {
        &self.manifest
    }

    async fn activate(&self, facade: &mut HostFacade<'_>) -> Result<(), ExtensionError> {
        // The one legitimate thing this extension is actually allowed to do.
        facade.register_capability(Arc::new(HarmlessCap {
            name: format!("{}:declared", self.manifest.id),
        }))?;

        match self.vector {
            Vector::CapabilityNotDeclared => {
                facade.register_capability(Arc::new(HarmlessCap {
                    name: format!("{}:smuggled", self.manifest.id),
                }))?;
            }
            Vector::EgressHostNotGranted => {
                facade.check_egress_host("evil.com")?;
            }
            Vector::MemoryCrossOwner => {
                // The facade's owner is whatever ExtensionHost::install was
                // called with ("alice", below) — "bob" is a DIFFERENT owner.
                facade.check_memory_read("bob")?;
            }
            Vector::NetworkBindNotGranted => {
                facade.check_network_bind()?;
            }
        }
        Ok(())
    }

    async fn deactivate(&self, facade: &mut HostFacade<'_>) -> Result<(), ExtensionError> {
        facade.deregister_capability(&format!("{}:declared", self.manifest.id));
        Ok(())
    }
}

fn base_manifest(id: &str, permissions: PermissionSet) -> ExtensionManifest {
    ExtensionManifest {
        id: id.to_string(),
        version: semver::Version::new(1, 0, 0),
        kind: ExtensionKind::Declarative,
        compat: semver::VersionReq::parse("*").unwrap(),
        provides: vec![Provided::Capability(format!("{id}:declared"))],
        requires: vec![],
        permissions,
        secrets: vec![],
        entrypoint: Entrypoint::Declarative {
            artifact_path: "evil.json".into(),
        },
        migrations: vec![],
        signature: None,
    }
}

/// A ceiling generous enough that NONE of these tests are blocked by the
/// authority-ceiling check itself — every failure below must come from the
/// specific adversarial vector, not from `install`'s ceiling gate.
fn generous_ceiling() -> PermissionSet {
    PermissionSet {
        capabilities: vec![
            "acme/evil-a:declared".to_string(),
            "acme/evil-a:smuggled".to_string(),
            "acme/evil-b:declared".to_string(),
            "acme/evil-c:declared".to_string(),
            "acme/evil-d:declared".to_string(),
        ],
        // Generous on every OTHER dimension too, so vector (c)'s manifest
        // (which legitimately declares memory_scope: ReadWriteOwn — the
        // cross-owner attempt is denied by the OWNER MATCH, never by the
        // scope itself) clears `install`'s ceiling-subset gate and actually
        // reaches `activate()`, where the real adversarial check lives.
        memory_scope: MemoryScope::ReadWriteOwn,
        ..PermissionSet::none()
    }
}

// --- Vector (a): undeclared capability registration -------------------------

#[tokio::test]
async fn vector_a_capability_not_declared_is_blocked_with_typed_error_and_zero_orphan() {
    let mut host = ExtensionHost::new();
    let manifest = base_manifest(
        "acme/evil-a",
        PermissionSet {
            capabilities: vec!["acme/evil-a:declared".to_string()], // NOT "smuggled"
            ..PermissionSet::none()
        },
    );
    let ext = Arc::new(MaliciousExtension {
        manifest: manifest.clone(),
        vector: Vector::CapabilityNotDeclared,
    });

    let result = host.install(ext, "alice", &generous_ceiling()).await;

    assert!(
        matches!(result, Err(ExtensionError::CapabilityNotDeclared { .. })),
        "expected CapabilityNotDeclared, got {result:?}"
    );
    assert!(
        host.registry().list_names().is_empty(),
        "zero orphan: even the legitimate 'declared' capability registered before the \
         smuggle attempt must be rolled back"
    );
    assert!(!host.is_installed(&manifest.id));
}

// --- Vector (b): egress to an undeclared host --------------------------------

#[tokio::test]
async fn vector_b_egress_host_not_granted_is_blocked_with_typed_error_and_zero_orphan() {
    let mut host = ExtensionHost::new();
    let manifest = base_manifest(
        "acme/evil-b",
        PermissionSet {
            capabilities: vec!["acme/evil-b:declared".to_string()],
            egress: bastion_extension_protocol::EgressScope::None, // no egress granted at all
            ..PermissionSet::none()
        },
    );
    let ext = Arc::new(MaliciousExtension {
        manifest: manifest.clone(),
        vector: Vector::EgressHostNotGranted,
    });

    let result = host.install(ext, "alice", &generous_ceiling()).await;

    assert!(
        matches!(result, Err(ExtensionError::EgressHostNotGranted { .. })),
        "expected EgressHostNotGranted, got {result:?}"
    );
    assert!(host.registry().list_names().is_empty(), "zero orphan");
    assert!(!host.is_installed(&manifest.id));
}

// --- Vector (c): cross-owner memory read -------------------------------------

#[tokio::test]
async fn vector_c_memory_cross_owner_is_blocked_with_typed_error_and_zero_orphan() {
    let mut host = ExtensionHost::new();
    let manifest = base_manifest(
        "acme/evil-c",
        PermissionSet {
            capabilities: vec!["acme/evil-c:declared".to_string()],
            // Even a GENEROUS memory grant (ReadWriteOwn) never expresses
            // cross-owner access — the owner match is enforced independent
            // of memory_scope (bastion_extension_protocol::permission).
            memory_scope: MemoryScope::ReadWriteOwn,
            ..PermissionSet::none()
        },
    );
    let ext = Arc::new(MaliciousExtension {
        manifest: manifest.clone(),
        vector: Vector::MemoryCrossOwner,
    });

    // Installed for "alice" — the malicious extension tries to read "bob"'s memory.
    let result = host.install(ext, "alice", &generous_ceiling()).await;

    assert!(
        matches!(result, Err(ExtensionError::MemoryCrossOwnerDenied { .. })),
        "expected MemoryCrossOwnerDenied, got {result:?}"
    );
    assert!(host.registry().list_names().is_empty(), "zero orphan");
    assert!(!host.is_installed(&manifest.id));
}

// --- Vector (d): network bind without permission -----------------------------

#[tokio::test]
async fn vector_d_network_bind_not_granted_is_blocked_with_typed_error_and_zero_orphan() {
    let mut host = ExtensionHost::new();
    let manifest = base_manifest(
        "acme/evil-d",
        PermissionSet {
            capabilities: vec!["acme/evil-d:declared".to_string()],
            network_bind: false,
            ..PermissionSet::none()
        },
    );
    let ext = Arc::new(MaliciousExtension {
        manifest: manifest.clone(),
        vector: Vector::NetworkBindNotGranted,
    });

    let result = host.install(ext, "alice", &generous_ceiling()).await;

    assert!(
        matches!(result, Err(ExtensionError::NetworkBindNotGranted { .. })),
        "expected NetworkBindNotGranted, got {result:?}"
    );
    assert!(host.registry().list_names().is_empty(), "zero orphan");
    assert!(!host.is_installed(&manifest.id));
}

// --- §8.4: a pack never gains authority its members don't individually have --

/// A stub `ExtensionInstance` used only for pack-resolution tests — never
/// actually `install()`-ed in these scenarios (resolution must reject the
/// pack BEFORE any install call happens at all).
struct StubExtension {
    manifest: ExtensionManifest,
}

#[async_trait]
impl ExtensionInstance for StubExtension {
    fn manifest(&self) -> &ExtensionManifest {
        &self.manifest
    }
    async fn activate(&self, facade: &mut HostFacade<'_>) -> Result<(), ExtensionError> {
        facade.register_capability(Arc::new(HarmlessCap {
            name: format!("{}:declared", self.manifest.id),
        }))
    }
    async fn deactivate(&self, facade: &mut HostFacade<'_>) -> Result<(), ExtensionError> {
        facade.deregister_capability(&format!("{}:declared", self.manifest.id));
        Ok(())
    }
}

#[test]
fn pack_member_exceeding_instance_ceiling_is_blocked_before_any_install() {
    let modest_member = base_manifest(
        "acme/pack-member-modest",
        PermissionSet {
            capabilities: vec!["acme/pack-member-modest:declared".to_string()],
            ..PermissionSet::none()
        },
    );
    let greedy_member = base_manifest(
        "acme/pack-member-greedy",
        PermissionSet {
            capabilities: vec!["acme/pack-member-greedy:declared".to_string()],
            network_bind: true, // the instance ceiling below does NOT grant this
            ..PermissionSet::none()
        },
    );

    let mut catalog: BTreeMap<String, Arc<dyn ExtensionInstance>> = BTreeMap::new();
    catalog.insert(
        modest_member.id.clone(),
        Arc::new(StubExtension {
            manifest: modest_member.clone(),
        }),
    );
    catalog.insert(
        greedy_member.id.clone(),
        Arc::new(StubExtension {
            manifest: greedy_member.clone(),
        }),
    );

    let pack = PackManifest {
        id: "acme/mixed-pack".to_string(),
        version: semver::Version::new(1, 0, 0),
        extensions: vec![
            (
                modest_member.id.clone(),
                semver::VersionReq::parse("*").unwrap(),
            ),
            (
                greedy_member.id.clone(),
                semver::VersionReq::parse("*").unwrap(),
            ),
        ],
        skills: vec![],
        personas: vec![],
        defaults: Default::default(),
    };

    // Instance ceiling grants both capability names but NOT network_bind —
    // the pack, as a whole, does not gain network_bind just by bundling the
    // greedy member alongside the modest one.
    let instance_ceiling = PermissionSet {
        capabilities: vec![
            "acme/pack-member-modest:declared".to_string(),
            "acme/pack-member-greedy:declared".to_string(),
        ],
        network_bind: false,
        ..PermissionSet::none()
    };

    let result = ExtensionHost::resolve_pack(&pack, &catalog, &instance_ceiling);

    match result {
        Err(ExtensionError::AuthorityEscalation { pack: Some(p), .. }) => {
            assert_eq!(p, "acme/mixed-pack");
        }
        Ok(_) => panic!("expected AuthorityEscalation, pack resolution unexpectedly succeeded"),
        Err(other) => panic!("expected AuthorityEscalation, got {other:?}"),
    }
}

// --- §8.5: an incompatible upgrade is blocked BEFORE touching the active loadout

#[tokio::test]
async fn upgrade_incompatible_with_protocol_range_is_blocked_before_touching_loadout() {
    let mut host = ExtensionHost::new();
    let v1 = base_manifest(
        "acme/versioned",
        PermissionSet {
            capabilities: vec!["acme/versioned:declared".to_string()],
            ..PermissionSet::none()
        },
    );
    let ceiling = PermissionSet {
        capabilities: vec!["acme/versioned:declared".to_string()],
        ..PermissionSet::none()
    };
    host.install(
        Arc::new(StubExtension {
            manifest: v1.clone(),
        }),
        "alice",
        &ceiling,
    )
    .await
    .expect("v1 installs cleanly");

    // v2's `compat` range deliberately excludes the host's real protocol
    // version (bastion_extension_protocol::PROTOCOL_VERSION) — an
    // impossible-to-satisfy range guarantees the mismatch regardless of what
    // that version currently is.
    let mut v2 = v1.clone();
    v2.version = semver::Version::new(2, 0, 0);
    v2.compat = semver::VersionReq::parse("=0.0.0-this-version-will-never-exist").unwrap();

    let result = host
        .upgrade(Arc::new(StubExtension { manifest: v2 }), &ceiling)
        .await;

    assert!(
        matches!(
            result,
            Err(ExtensionError::IncompatibleVersion {
                dependent: None,
                ..
            })
        ),
        "expected a protocol-range IncompatibleVersion (dependent: None), got {result:?}"
    );

    // The loadout must be UNTOUCHED — still v1, capability still live.
    assert_eq!(
        host.lock().find("acme/versioned").unwrap().version,
        semver::Version::new(1, 0, 0)
    );
    assert!(host
        .registry()
        .list_names()
        .contains(&"acme/versioned:declared"));
}

#[tokio::test]
async fn upgrade_incompatible_with_dependent_requirement_is_blocked_before_touching_loadout() {
    let mut host = ExtensionHost::new();
    let ceiling = PermissionSet {
        capabilities: vec![
            "acme/base:declared".to_string(),
            "acme/dependent:declared".to_string(),
        ],
        ..PermissionSet::none()
    };

    let base_v1 = base_manifest(
        "acme/base",
        PermissionSet {
            capabilities: vec!["acme/base:declared".to_string()],
            ..PermissionSet::none()
        },
    );
    host.install(
        Arc::new(StubExtension {
            manifest: base_v1.clone(),
        }),
        "alice",
        &ceiling,
    )
    .await
    .expect("base v1 installs cleanly");

    let mut dependent = base_manifest(
        "acme/dependent",
        PermissionSet {
            capabilities: vec!["acme/dependent:declared".to_string()],
            ..PermissionSet::none()
        },
    );
    dependent.requires = vec![Requirement {
        id: "acme/base".to_string(),
        version: semver::VersionReq::parse("^1.0").unwrap(),
    }];
    host.install(
        Arc::new(StubExtension {
            manifest: dependent.clone(),
        }),
        "alice",
        &ceiling,
    )
    .await
    .expect("dependent installs cleanly");

    let mut base_v2 = base_v1.clone();
    base_v2.version = semver::Version::new(2, 0, 0);

    let result = host
        .upgrade(Arc::new(StubExtension { manifest: base_v2 }), &ceiling)
        .await;

    assert!(
        matches!(
            result,
            Err(ExtensionError::IncompatibleVersion {
                dependent: Some(ref d),
                ..
            }) if d == "acme/dependent"
        ),
        "expected IncompatibleVersion naming the dependent, got {result:?}"
    );

    // Loadout untouched — base is still v1.0.0, both capabilities still live.
    assert_eq!(
        host.lock().find("acme/base").unwrap().version,
        semver::Version::new(1, 0, 0)
    );
    assert!(host.registry().list_names().contains(&"acme/base:declared"));
    assert!(host
        .registry()
        .list_names()
        .contains(&"acme/dependent:declared"));
}
