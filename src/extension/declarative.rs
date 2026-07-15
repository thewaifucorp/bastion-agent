//! The `Declarative` mechanism (design doc §2): data only — skills, personas,
//! triggers, config. **No code runs.** The only "execution" a declarative
//! extension performs is the host mechanically wrapping its own static data
//! behind a read-only [`Capability`] — the wrapper is HOST-authored, generic,
//! trusted code; the extension supplies nothing but a JSON value.
//!
//! Still goes through [`HostFacade::register_capability`] exactly like every
//! other mechanism — "no code runs" does not mean "no enforcement": a
//! declarative manifest that tries to register data under an undeclared
//! capability name is rejected the same way a subprocess/wasm extension
//! would be.

use crate::extension::facade::{ExtensionInstance, HostFacade};
use bastion_extension_protocol::{ExtensionError, ExtensionManifest};
use bastion_runtime::capability::{Capability, InvokeCtx};
use serde_json::Value;
use std::sync::Arc;

/// A read-only capability whose `invoke()` returns exactly the static data
/// it was constructed with — never interprets, never executes, never accepts
/// input that changes its output. `is_local()`/`is_trusted()` both default to
/// `true`: the data never leaves the host and the wrapper is host code, not
/// third-party logic (mirrors `crates/bastion-runtime/src/capability/registry.rs`'s
/// `is_local`/`is_trusted` typed-property convention).
pub struct StaticDataCapability {
    name: String,
    description: String,
    schema: Value,
    data: Value,
}

impl StaticDataCapability {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        schema: Value,
        data: Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            schema,
            data,
        }
    }
}

#[async_trait::async_trait]
impl Capability for StaticDataCapability {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn input_schema(&self) -> &Value {
        &self.schema
    }

    async fn invoke(&self, _args: Value, _ctx: &InvokeCtx) -> anyhow::Result<Value> {
        Ok(self.data.clone())
    }

    fn is_local(&self) -> bool {
        true
    }

    fn is_trusted(&self) -> bool {
        true
    }
}

/// One declarative capability entry: (name, description, input schema, data).
pub type DeclarativeEntry = (String, String, Value, Value);

/// A `Declarative`-kind extension: a manifest plus the static data entries it
/// provides. Loading the artifact file (parsing whatever on-disk format a
/// pack ships, e.g. TOML/JSON skill definitions) into these entries is a
/// loader concern outside this type — this is the validated, in-memory shape
/// `ExtensionHost::install` activates.
pub struct DeclarativeExtension {
    manifest: ExtensionManifest,
    entries: Vec<DeclarativeEntry>,
}

impl DeclarativeExtension {
    pub fn new(manifest: ExtensionManifest, entries: Vec<DeclarativeEntry>) -> Self {
        Self { manifest, entries }
    }
}

#[async_trait::async_trait]
impl ExtensionInstance for DeclarativeExtension {
    fn manifest(&self) -> &ExtensionManifest {
        &self.manifest
    }

    async fn activate(&self, facade: &mut HostFacade<'_>) -> Result<(), ExtensionError> {
        for (name, description, schema, data) in &self.entries {
            facade.register_capability(Arc::new(StaticDataCapability::new(
                name.clone(),
                description.clone(),
                schema.clone(),
                data.clone(),
            )))?;
        }
        Ok(())
    }

    async fn deactivate(&self, facade: &mut HostFacade<'_>) -> Result<(), ExtensionError> {
        for (name, _, _, _) in &self.entries {
            facade.deregister_capability(name);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bastion_extension_protocol::{Entrypoint, ExtensionKind, PermissionSet, Provided};
    use bastion_runtime::capability::CapabilityRegistry;

    fn manifest() -> ExtensionManifest {
        ExtensionManifest {
            id: "acme/skill-pack".to_string(),
            version: semver::Version::new(1, 0, 0),
            kind: ExtensionKind::Declarative,
            compat: semver::VersionReq::parse("*").unwrap(),
            provides: vec![Provided::Capability("acme/skill-pack:greeting".to_string())],
            requires: vec![],
            permissions: PermissionSet {
                capabilities: vec!["acme/skill-pack:greeting".to_string()],
                ..PermissionSet::none()
            },
            secrets: vec![],
            entrypoint: Entrypoint::Declarative {
                artifact_path: "greeting.json".into(),
            },
            migrations: vec![],
            signature: None,
        }
    }

    #[tokio::test]
    async fn activate_registers_static_capability_returning_its_own_data() {
        let ext = DeclarativeExtension::new(
            manifest(),
            vec![(
                "acme/skill-pack:greeting".to_string(),
                "returns a canned greeting".to_string(),
                serde_json::json!({}),
                serde_json::json!({"greeting": "hello, owner"}),
            )],
        );
        let m = ext.manifest().clone();
        let mut registry = CapabilityRegistry::new();
        {
            let mut facade = HostFacade::new(&m, "alice", &mut registry);
            ext.activate(&mut facade).await.expect("activate succeeds");
        }

        let cap_names = registry.list_names();
        assert!(cap_names.contains(&"acme/skill-pack:greeting"));

        let result = registry
            .invoke(
                "acme/skill-pack:greeting",
                serde_json::json!({}),
                &bastion_runtime::capability::InvokeCtx {
                    owner: "alice".to_string(),
                    privacy_tier: Some(bastion_memory::PrivacyTier::LocalOnly),
                },
            )
            .await
            .expect("invoke succeeds — is_local()==true clears the egress gate");
        assert_eq!(result.data, serde_json::json!({"greeting": "hello, owner"}));
        assert!(
            result.trusted,
            "declarative data is host-wrapped and trusted"
        );
    }

    #[tokio::test]
    async fn deactivate_removes_every_registered_capability() {
        let ext = DeclarativeExtension::new(
            manifest(),
            vec![(
                "acme/skill-pack:greeting".to_string(),
                "d".to_string(),
                serde_json::json!({}),
                serde_json::json!({}),
            )],
        );
        let m = ext.manifest().clone();
        let mut registry = CapabilityRegistry::new();
        {
            let mut facade = HostFacade::new(&m, "alice", &mut registry);
            ext.activate(&mut facade).await.unwrap();
        }
        {
            let mut facade = HostFacade::new(&m, "alice", &mut registry);
            ext.deactivate(&mut facade).await.unwrap();
        }
        assert!(registry.list_names().is_empty());
    }
}
