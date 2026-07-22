//! The `Wasm` mechanism (design doc §2): a sandboxed, zero-import,
//! fuel-bounded module. The wasm runtime dependency (`wasmi`) lives entirely
//! in the isolated `bastion-extension-wasm` crate (§8.7) — this module only
//! wraps [`bastion_extension_wasm::WasmSandbox`] into an
//! [`ExtensionInstance`]/[`Capability`], the same shape `declarative.rs` and
//! `subprocess.rs` use.
//!
//! Unlike the `Subprocess` mechanism, there is no host-request protocol here
//! — the reference `Wasm` extension is pure computation, and the sandbox
//! itself proves "no ambient authority" STRUCTURALLY (an empty `Linker`
//! means the guest has nothing to call outside itself, not merely something
//! policy-denies). `is_local()` is `true` for the same reason
//! `declarative.rs`'s `StaticDataCapability` is: execution never leaves the
//! daemon's own process, let alone the host.

use crate::extension::facade::{ExtensionInstance, HostFacade};
use async_trait::async_trait;
use bastion_extension_protocol::{ExtensionError, ExtensionManifest};
use bastion_extension_wasm::WasmSandbox;
use bastion_runtime::capability::{Capability, InvokeCtx};
use serde_json::Value;
use std::sync::Arc;

/// Default fuel budget for a wasm call — generous enough for real
/// computation, small enough that a `busy_loop`-shaped guest traps in well
/// under a second instead of spinning a CPU core.
pub const DEFAULT_FUEL: u64 = 10_000_000;

pub struct WasmCapability {
    name: String,
    description: String,
    schema: Value,
    extension_id: String,
    wasm_bytes: Arc<Vec<u8>>,
    func_name: String,
    fuel: u64,
    sandbox: Arc<WasmSandbox>,
}

fn i64_arg(args: &Value, key: &str) -> anyhow::Result<i64> {
    args.get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| anyhow::anyhow!("missing or non-integer '{key}' argument"))
}

#[async_trait]
impl Capability for WasmCapability {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn input_schema(&self) -> &Value {
        &self.schema
    }

    /// Execution never leaves the daemon's own process — the sandbox has no
    /// imports to reach a network/filesystem/registry through even in
    /// principle.
    fn is_local(&self) -> bool {
        true
    }

    async fn invoke(&self, args: Value, _ctx: &InvokeCtx) -> anyhow::Result<Value> {
        let a = i64_arg(&args, "a")?;
        let b = i64_arg(&args, "b")?;

        let sandbox = self.sandbox.clone();
        let wasm_bytes = self.wasm_bytes.clone();
        let func_name = self.func_name.clone();
        let fuel = self.fuel;
        let extension_id = self.extension_id.clone();

        // wasmi execution is synchronous/CPU-bound (it's an interpreter, not
        // a JIT) — run it off the async executor so a slow (or
        // fuel-bounded-but-still-nontrivial) guest never blocks the daemon's
        // tokio worker threads.
        let result = tokio::task::spawn_blocking(move || {
            sandbox.call_i64_i64_to_i64(&wasm_bytes, &func_name, a, b, fuel)
        })
        .await
        .map_err(|e| anyhow::anyhow!("wasm extension '{extension_id}' task panicked: {e}"))?
        .map_err(|e| anyhow::anyhow!("wasm extension '{extension_id}' error: {e}"))?;

        Ok(serde_json::json!({"result": result}))
    }
}

/// One wasm-backed capability entry: (name, description, schema, wasm
/// bytes, exported function name, fuel budget).
pub type WasmEntry = (String, String, Value, Vec<u8>, String, u64);

/// A `Wasm`-kind extension: a manifest plus the capabilities it wants backed
/// by a sandboxed module.
pub struct WasmExtension {
    manifest: ExtensionManifest,
    entries: Vec<WasmEntry>,
    sandbox: Arc<WasmSandbox>,
}

impl WasmExtension {
    pub fn new(
        manifest: ExtensionManifest,
        entries: Vec<WasmEntry>,
    ) -> Result<Self, ExtensionError> {
        let sandbox = WasmSandbox::new().map_err(|e| ExtensionError::Mechanism {
            id: manifest.id.clone(),
            detail: e.to_string(),
        })?;
        Ok(Self {
            manifest,
            entries,
            sandbox: Arc::new(sandbox),
        })
    }
}

#[async_trait]
impl ExtensionInstance for WasmExtension {
    fn manifest(&self) -> &ExtensionManifest {
        &self.manifest
    }

    async fn activate(&self, facade: &mut HostFacade<'_>) -> Result<(), ExtensionError> {
        for (name, description, schema, wasm_bytes, func_name, fuel) in &self.entries {
            facade.register_capability(Arc::new(WasmCapability {
                name: name.clone(),
                description: description.clone(),
                schema: schema.clone(),
                extension_id: self.manifest.id.clone(),
                wasm_bytes: Arc::new(wasm_bytes.clone()),
                func_name: func_name.clone(),
                fuel: *fuel,
                sandbox: self.sandbox.clone(),
            }))?;
        }
        Ok(())
    }

    async fn deactivate(&self, facade: &mut HostFacade<'_>) -> Result<(), ExtensionError> {
        for (name, _, _, _, _, _) in &self.entries {
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

    /// The reference `Wasm` extension module — see
    /// `src/extension/wasm_fixtures/reference_extension.rs` for source +
    /// regeneration instructions. `add`/`busy_loop` exports.
    const REFERENCE_WASM: &[u8] = include_bytes!("wasm_fixtures/reference_extension.wasm");

    fn manifest() -> ExtensionManifest {
        ExtensionManifest {
            id: "acme/wasm-calc".to_string(),
            version: semver::Version::new(1, 0, 0),
            kind: ExtensionKind::Wasm,
            compat: semver::VersionReq::parse("*").unwrap(),
            provides: vec![Provided::Capability("acme/wasm-calc:add".to_string())],
            requires: vec![],
            permissions: PermissionSet {
                capabilities: vec!["acme/wasm-calc:add".to_string()],
                ..PermissionSet::none()
            },
            secrets: vec![],
            entrypoint: Entrypoint::Wasm {
                module_path: "reference_extension.wasm".into(),
            },
            migrations: vec![],
            signature: None,
        }
    }

    fn ctx(owner: &str) -> InvokeCtx {
        InvokeCtx {
            owner: owner.to_string(),
            privacy_tier: Some(bastion_memory::PrivacyTier::LocalOnly),
            allowed_tools: None,
        }
    }

    #[tokio::test]
    async fn activate_registers_capability_and_invoke_computes_via_wasmi() {
        let m = manifest();
        let ext = WasmExtension::new(
            m.clone(),
            vec![(
                "acme/wasm-calc:add".to_string(),
                "adds two integers inside a wasm sandbox".to_string(),
                serde_json::json!({}),
                REFERENCE_WASM.to_vec(),
                "add".to_string(),
                DEFAULT_FUEL,
            )],
        )
        .expect("WasmExtension::new should succeed");

        let mut registry = CapabilityRegistry::new();
        {
            let mut facade = HostFacade::new(&m, "alice", &mut registry);
            ext.activate(&mut facade).await.expect("activate succeeds");
        }

        assert!(registry.list_names().contains(&"acme/wasm-calc:add"));

        let result = registry
            .invoke(
                "acme/wasm-calc:add",
                serde_json::json!({"a": 7, "b": 35}),
                &ctx("alice"),
            )
            .await
            .expect("invoke succeeds — is_local()==true clears the egress gate under LocalOnly");
        assert_eq!(result.data, serde_json::json!({"result": 42}));
    }

    #[tokio::test]
    async fn deactivate_removes_every_registered_capability() {
        let m = manifest();
        let ext = WasmExtension::new(
            m.clone(),
            vec![(
                "acme/wasm-calc:add".to_string(),
                "d".to_string(),
                serde_json::json!({}),
                REFERENCE_WASM.to_vec(),
                "add".to_string(),
                DEFAULT_FUEL,
            )],
        )
        .unwrap();
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
