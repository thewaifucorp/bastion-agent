//! `HostFacade` — the ONE chokepoint an extension mechanism (declarative,
//! subprocess, wasm) goes through to touch anything outside itself. Mirrors
//! `CapabilityRegistry::invoke`'s "one policy boundary" precedent
//! (`crates/bastion-runtime/src/capability/registry.rs`) one layer earlier:
//! there, every FRONTEND funnels through one `invoke()`; here, every
//! MECHANISM funnels through one facade.
//!
//! An extension never gets a raw `&mut CapabilityRegistry`, a raw socket, or
//! raw memory access. Every method on this type is a check-then-act pair
//! where the check is a pure `PermissionSet` method
//! (`bastion_extension_protocol::permission`) — enforcement lives HERE, in
//! product code, never trusted to the mechanism/extension itself.

use bastion_extension_protocol::{ExtensionError, ExtensionManifest};
use bastion_runtime::capability::{Capability, CapabilityRegistry};
use std::sync::Arc;

/// A running (or about-to-run) extension mechanism. Implemented once per
/// kind: `declarative::DeclarativeExtension`, `subprocess::SubprocessExtension`
/// (and, if the WASM mechanism lands, `wasm::WasmExtension`).
///
/// `activate`/`deactivate` are the ONLY way a mechanism touches the host —
/// both take a `&mut HostFacade`, never a raw registry/socket/memory handle.
#[async_trait::async_trait]
pub trait ExtensionInstance: Send + Sync {
    fn manifest(&self) -> &ExtensionManifest;

    /// Register whatever this extension provides. Every `register_capability`
    /// call inside this method is checked against `manifest().permissions`
    /// before it reaches the registry — a mechanism that tries to register
    /// something undeclared gets `Err` back, not a silent no-op.
    async fn activate(&self, facade: &mut HostFacade<'_>) -> Result<(), ExtensionError>;

    /// Undo everything `activate` did. MUST be idempotent-safe to call even
    /// if `activate` partially failed (the host always calls this, or does
    /// the equivalent cleanup itself, on any failure path — see
    /// `ExtensionHost::install`/`upgrade`/`revoke`).
    async fn deactivate(&self, facade: &mut HostFacade<'_>) -> Result<(), ExtensionError>;
}

/// The enforcement boundary, scoped to exactly ONE extension's manifest and
/// owner for the duration of one `activate`/`deactivate` call.
///
/// Holds `&mut CapabilityRegistry` directly (no `Mutex`) — mirrors the
/// daemon's "one `&mut agent`" serialization model (AGENTS.md): exactly one
/// facade borrows the registry at a time, for exactly the duration of one
/// mechanism call, never shared/cloned across tasks.
pub struct HostFacade<'a> {
    manifest: &'a ExtensionManifest,
    owner: &'a str,
    registry: &'a mut CapabilityRegistry,
    /// Capability names registered THROUGH this facade during the current
    /// call — the host trusts THIS list (not the mechanism's own bookkeeping)
    /// to know what to roll back on failure or remove on deactivate/revoke.
    registered: Vec<String>,
}

impl<'a> HostFacade<'a> {
    pub fn new(
        manifest: &'a ExtensionManifest,
        owner: &'a str,
        registry: &'a mut CapabilityRegistry,
    ) -> Self {
        Self {
            manifest,
            owner,
            registry,
            registered: Vec::new(),
        }
    }

    pub fn manifest(&self) -> &ExtensionManifest {
        self.manifest
    }

    pub fn owner(&self) -> &str {
        self.owner
    }

    /// Every capability name this facade successfully registered so far.
    pub fn registered_capabilities(&self) -> &[String] {
        &self.registered
    }

    /// Adversarial vector (a): register a capability outside
    /// `manifest.permissions.capabilities`. Checked HERE, before the registry
    /// is ever touched — a malicious mechanism cannot register-then-hope, the
    /// call itself fails.
    pub fn register_capability(&mut self, cap: Arc<dyn Capability>) -> Result<(), ExtensionError> {
        let name = cap.name().to_string();
        if !self.manifest.permissions.allows_capability(&name) {
            return Err(ExtensionError::CapabilityNotDeclared {
                extension: self.manifest.id.clone(),
                capability: name,
            });
        }
        self.registry
            .register(cap)
            .map_err(|_| ExtensionError::CapabilityCollision {
                capability: name.clone(),
                owner: self.manifest.id.clone(),
            })?;
        self.registered.push(name);
        Ok(())
    }

    /// Remove a capability this facade (or a prior activation of the same
    /// extension) registered. Idempotent — mirrors
    /// `CapabilityRegistry::remove`.
    pub fn deregister_capability(&mut self, name: &str) -> bool {
        self.registry.remove(name)
    }

    /// Adversarial vector (b): reach a host outside `manifest.permissions.egress`.
    pub fn check_egress_host(&self, host: &str) -> Result<(), ExtensionError> {
        if !self.manifest.permissions.allows_egress_host(host) {
            return Err(ExtensionError::EgressHostNotGranted {
                extension: self.manifest.id.clone(),
                host: host.to_string(),
            });
        }
        Ok(())
    }

    /// Adversarial vector (c): read memory belonging to an owner other than
    /// the one this extension instance is running for. `target_owner` is
    /// whatever the mechanism is ASKING for — never trusted to equal
    /// `self.owner` on its own.
    pub fn check_memory_read(&self, target_owner: &str) -> Result<(), ExtensionError> {
        if !self
            .manifest
            .permissions
            .allows_memory_read(self.owner, target_owner)
        {
            return Err(ExtensionError::MemoryCrossOwnerDenied {
                extension: self.manifest.id.clone(),
                requester: self.owner.to_string(),
                target: target_owner.to_string(),
            });
        }
        Ok(())
    }

    pub fn check_memory_write(&self, target_owner: &str) -> Result<(), ExtensionError> {
        if !self
            .manifest
            .permissions
            .allows_memory_write(self.owner, target_owner)
        {
            return Err(ExtensionError::MemoryCrossOwnerDenied {
                extension: self.manifest.id.clone(),
                requester: self.owner.to_string(),
                target: target_owner.to_string(),
            });
        }
        Ok(())
    }

    /// Adversarial vector (d): open a listening socket without
    /// `network_bind`.
    pub fn check_network_bind(&self) -> Result<(), ExtensionError> {
        if !self.manifest.permissions.allows_network_bind() {
            return Err(ExtensionError::NetworkBindNotGranted {
                extension: self.manifest.id.clone(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bastion_extension_protocol::{Entrypoint, ExtensionKind, MemoryScope, PermissionSet};
    use bastion_runtime::capability::InvokeCtx;
    use serde_json::Value;

    fn manifest(permissions: PermissionSet) -> ExtensionManifest {
        ExtensionManifest {
            id: "acme/widget".to_string(),
            version: semver::Version::new(1, 0, 0),
            kind: ExtensionKind::Declarative,
            compat: semver::VersionReq::parse("*").unwrap(),
            provides: vec![],
            requires: vec![],
            permissions,
            secrets: vec![],
            entrypoint: Entrypoint::Declarative {
                artifact_path: "widget.json".into(),
            },
            migrations: vec![],
            signature: None,
        }
    }

    struct StubCap(&'static str);

    #[async_trait::async_trait]
    impl Capability for StubCap {
        fn name(&self) -> &str {
            self.0
        }
        fn description(&self) -> &str {
            "stub"
        }
        fn input_schema(&self) -> &Value {
            static SCHEMA: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
            SCHEMA.get_or_init(|| serde_json::json!({}))
        }
        async fn invoke(&self, _args: Value, _ctx: &InvokeCtx) -> anyhow::Result<Value> {
            Ok(Value::Null)
        }
    }

    #[test]
    fn register_capability_denied_when_not_declared() {
        let m = manifest(PermissionSet::none());
        let mut registry = CapabilityRegistry::new();
        let mut facade = HostFacade::new(&m, "alice", &mut registry);

        let result = facade.register_capability(Arc::new(StubCap("evil:exfiltrate")));
        assert!(matches!(
            result,
            Err(ExtensionError::CapabilityNotDeclared { .. })
        ));
        assert!(facade.registered_capabilities().is_empty());
        assert!(registry.list_names().is_empty());
    }

    #[test]
    fn register_capability_allowed_when_declared() {
        let m = manifest(PermissionSet {
            capabilities: vec!["widget:read".to_string()],
            ..PermissionSet::none()
        });
        let mut registry = CapabilityRegistry::new();
        let mut facade = HostFacade::new(&m, "alice", &mut registry);

        let result = facade.register_capability(Arc::new(StubCap("widget:read")));
        assert!(result.is_ok());
        assert_eq!(
            facade.registered_capabilities(),
            &["widget:read".to_string()]
        );
    }

    #[test]
    fn egress_denied_to_undeclared_host() {
        let m = manifest(PermissionSet::none());
        let mut registry = CapabilityRegistry::new();
        let facade = HostFacade::new(&m, "alice", &mut registry);

        let result = facade.check_egress_host("evil.com");
        assert!(matches!(
            result,
            Err(ExtensionError::EgressHostNotGranted { .. })
        ));
    }

    #[test]
    fn egress_allowed_to_declared_host() {
        let m = manifest(PermissionSet {
            egress: bastion_extension_protocol::EgressScope::Hosts(vec!["api.x.com".to_string()]),
            ..PermissionSet::none()
        });
        let mut registry = CapabilityRegistry::new();
        let facade = HostFacade::new(&m, "alice", &mut registry);

        assert!(facade.check_egress_host("api.x.com").is_ok());
        assert!(facade.check_egress_host("evil.com").is_err());
    }

    #[test]
    fn memory_read_denied_cross_owner() {
        let m = manifest(PermissionSet {
            memory_scope: MemoryScope::ReadWriteOwn,
            ..PermissionSet::none()
        });
        let mut registry = CapabilityRegistry::new();
        let facade = HostFacade::new(&m, "alice", &mut registry);

        assert!(facade.check_memory_read("alice").is_ok());
        let result = facade.check_memory_read("bob");
        assert!(matches!(
            result,
            Err(ExtensionError::MemoryCrossOwnerDenied { .. })
        ));
    }

    #[test]
    fn network_bind_denied_without_permission() {
        let m = manifest(PermissionSet::none());
        let mut registry = CapabilityRegistry::new();
        let facade = HostFacade::new(&m, "alice", &mut registry);

        let result = facade.check_network_bind();
        assert!(matches!(
            result,
            Err(ExtensionError::NetworkBindNotGranted { .. })
        ));
    }

    #[test]
    fn network_bind_allowed_with_permission() {
        let m = manifest(PermissionSet {
            network_bind: true,
            ..PermissionSet::none()
        });
        let mut registry = CapabilityRegistry::new();
        let facade = HostFacade::new(&m, "alice", &mut registry);
        assert!(facade.check_network_bind().is_ok());
    }
}
