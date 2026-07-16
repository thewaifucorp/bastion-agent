//! Integration tests for the `Subprocess` extension mechanism
//! (`docs/revamp/C3-extension-protocol-design.md` §2) against the real
//! `reference-extension-echo` child process (`src/bin/reference_extension_echo.rs`).
//!
//! Lives here (not as a `src/extension/subprocess.rs` unit test) because
//! `CARGO_BIN_EXE_reference-extension-echo` is only defined by cargo for
//! INTEGRATION test targets of a package that defines the `[[bin]]` — never
//! for that package's own lib unit tests.

use bastion::extension::facade::{ExtensionInstance, HostFacade};
use bastion::extension::subprocess::SubprocessExtension;
use bastion_extension_protocol::{
    EgressScope, Entrypoint, ExtensionKind, ExtensionManifest, MemoryScope, PermissionSet,
    Provided, SecretRef,
};
use bastion_runtime::capability::{CapabilityRegistry, InvokeCtx};
use std::collections::HashMap;
use std::sync::Arc;

/// Path to the `reference-extension-echo` bin target.
fn echo_bin() -> String {
    env!("CARGO_BIN_EXE_reference-extension-echo").to_string()
}

fn manifest(permissions: PermissionSet) -> ExtensionManifest {
    ExtensionManifest {
        id: "acme/echo".to_string(),
        version: semver::Version::new(1, 0, 0),
        kind: ExtensionKind::Subprocess,
        compat: semver::VersionReq::parse("*").unwrap(),
        provides: vec![Provided::Capability("acme/echo:call".to_string())],
        requires: vec![],
        permissions,
        secrets: vec![],
        entrypoint: Entrypoint::Subprocess {
            command: echo_bin(),
            args: vec![],
        },
        migrations: vec![],
        signature: None,
    }
}

fn ctx(owner: &str) -> InvokeCtx {
    InvokeCtx {
        owner: owner.to_string(),
        privacy_tier: Some(bastion_memory::PrivacyTier::CloudOk),
    }
}

async fn install_echo(permissions: PermissionSet) -> (CapabilityRegistry, ExtensionManifest) {
    let m = manifest(PermissionSet {
        capabilities: vec!["acme/echo:call".to_string()],
        ..permissions
    });
    let ext = SubprocessExtension::new(
        m.clone(),
        vec![(
            "acme/echo:call".to_string(),
            "echoes its input back".to_string(),
            serde_json::json!({}),
            echo_bin(),
            vec![],
        )],
    )
    .with_unsandboxed_runner();
    let mut registry = CapabilityRegistry::new();
    {
        let mut facade = HostFacade::new(&m, "alice", &mut registry);
        ext.activate(&mut facade).await.expect("activate succeeds");
    }
    (registry, m)
}

#[tokio::test]
async fn plain_call_echoes_input_over_the_wire() {
    let (registry, _m) = install_echo(PermissionSet::none()).await;
    let result = registry
        .invoke(
            "acme/echo:call",
            serde_json::json!({"hello": "world"}),
            &ctx("alice"),
        )
        .await
        .expect("subprocess round-trip should succeed");
    assert_eq!(result.data["echo"], serde_json::json!({"hello": "world"}));
    assert!(!result.trusted, "subprocess output defaults to untrusted");
}

#[tokio::test]
async fn host_mediated_egress_fetch_denied_without_grant() {
    let (registry, _m) = install_echo(PermissionSet::none()).await;
    let result = registry
        .invoke(
            "acme/echo:call",
            serde_json::json!({"fetch_host": "evil.com"}),
            &ctx("alice"),
        )
        .await
        .expect("the call itself succeeds — denial is IN the response");
    let host_response = &result.data["host_response"];
    assert_eq!(host_response["ok"], serde_json::json!(false));
    assert!(host_response["error"].as_str().unwrap().contains("egress"));
}

#[tokio::test]
async fn host_mediated_egress_fetch_allowed_with_grant() {
    let (registry, _m) = install_echo(PermissionSet {
        egress: EgressScope::Hosts(vec!["api.x.com".to_string()]),
        ..PermissionSet::none()
    })
    .await;
    let result = registry
        .invoke(
            "acme/echo:call",
            serde_json::json!({"fetch_host": "api.x.com"}),
            &ctx("alice"),
        )
        .await
        .expect("call succeeds");
    let host_response = &result.data["host_response"];
    assert_eq!(host_response["ok"], serde_json::json!(true));
    assert_eq!(
        host_response["data"]["authorized_host"],
        serde_json::json!("api.x.com")
    );
}

#[tokio::test]
async fn host_mediated_memory_read_denied_cross_owner() {
    let (registry, _m) = install_echo(PermissionSet {
        memory_scope: MemoryScope::ReadOwn,
        ..PermissionSet::none()
    })
    .await;
    let result = registry
        .invoke(
            "acme/echo:call",
            serde_json::json!({"read_memory_owner": "bob"}),
            &ctx("alice"),
        )
        .await
        .expect("call succeeds");
    let host_response = &result.data["host_response"];
    assert_eq!(host_response["ok"], serde_json::json!(false));
}

#[tokio::test]
async fn host_mediated_memory_read_allowed_for_own_owner() {
    let (registry, _m) = install_echo(PermissionSet {
        memory_scope: MemoryScope::ReadOwn,
        ..PermissionSet::none()
    })
    .await;
    let result = registry
        .invoke(
            "acme/echo:call",
            serde_json::json!({"read_memory_owner": "alice"}),
            &ctx("alice"),
        )
        .await
        .expect("call succeeds");
    let host_response = &result.data["host_response"];
    assert_eq!(host_response["ok"], serde_json::json!(true));
}

#[tokio::test]
async fn host_mediated_network_bind_denied_without_grant() {
    let (registry, _m) = install_echo(PermissionSet::none()).await;
    let result = registry
        .invoke(
            "acme/echo:call",
            serde_json::json!({"bind_port": 8080}),
            &ctx("alice"),
        )
        .await
        .expect("call succeeds");
    let host_response = &result.data["host_response"];
    assert_eq!(host_response["ok"], serde_json::json!(false));
}

/// Adversarial vector (a) over the subprocess wire: even a child that ASKS
/// the host to register an undeclared capability mid-`invoke()` is denied —
/// structurally (no `CapabilityRegistry` handle reaches `invoke()` at all)
/// and by policy (the capability was never declared).
#[tokio::test]
async fn host_mediated_register_capability_always_denied() {
    let (registry, _m) = install_echo(PermissionSet::none()).await;
    let result = registry
        .invoke(
            "acme/echo:call",
            serde_json::json!({"attempt_register_capability": true}),
            &ctx("alice"),
        )
        .await
        .expect("call succeeds — denial is IN the response, not a hard invoke() failure");
    let host_response = &result.data["host_response"];
    assert_eq!(host_response["ok"], serde_json::json!(false));

    // The smuggled capability name never actually reaches the registry.
    assert!(!registry.list_names().contains(&"acme/echo:smuggled"));
}

// ─── C3-cloud-ready: SecretRef-by-name resolution into subprocess env ──────

/// Fixed-map test resolver — the same trait a real
/// `LayeredSecretResolver`/hosted operator secret manager implements.
struct MapSecretResolver(HashMap<String, String>);

impl bastion_types::SecretResolver for MapSecretResolver {
    fn resolve(
        &self,
        name: &str,
    ) -> Result<bastion_types::SecretValue, bastion_types::BastionError> {
        self.0
            .get(name)
            .map(|v| bastion_types::SecretValue::new(v.clone()))
            .ok_or_else(|| bastion_types::BastionError::SecretNotFound {
                name: name.to_string(),
            })
    }
}

fn manifest_with_secrets(secret_names: &[&str]) -> ExtensionManifest {
    ExtensionManifest {
        secrets: secret_names
            .iter()
            .map(|n| SecretRef {
                name: n.to_string(),
            })
            .collect(),
        ..manifest(PermissionSet {
            capabilities: vec!["acme/echo:call".to_string()],
            ..PermissionSet::none()
        })
    }
}

async fn install_echo_with_secrets(
    m: ExtensionManifest,
    resolver: Option<Arc<dyn bastion_types::SecretResolver>>,
) -> CapabilityRegistry {
    let mut ext = SubprocessExtension::new(
        m.clone(),
        vec![(
            "acme/echo:call".to_string(),
            "echoes its input back".to_string(),
            serde_json::json!({}),
            echo_bin(),
            vec![],
        )],
    )
    .with_unsandboxed_runner();
    if let Some(r) = resolver {
        ext = ext.with_secret_resolver(r);
    }
    let mut registry = CapabilityRegistry::new();
    {
        let mut facade = HostFacade::new(&m, "alice", &mut registry);
        ext.activate(&mut facade).await.expect("activate succeeds");
    }
    registry
}

/// The child sees EXACTLY the declared, resolved secrets — nothing ambient
/// from the test process's own environment leaks through `env_clear()`.
#[tokio::test]
async fn subprocess_child_receives_only_declared_resolved_secrets() {
    // Poison the test process's own env with something NOT declared by the
    // manifest — proves env_clear() actually holds, not just that the
    // allowlist mechanism works in isolation.
    std::env::set_var("ACME_ECHO_TEST_AMBIENT_LEAK", "should-never-reach-child");

    let manifest = manifest_with_secrets(&["ACME_API_TOKEN"]);
    let mut secrets = HashMap::new();
    secrets.insert("ACME_API_TOKEN".to_string(), "tok-abc-123".to_string());
    let resolver: Arc<dyn bastion_types::SecretResolver> = Arc::new(MapSecretResolver(secrets));

    let registry = install_echo_with_secrets(manifest, Some(resolver)).await;
    let result = registry
        .invoke(
            "acme/echo:call",
            serde_json::json!({"dump_env": true}),
            &ctx("alice"),
        )
        .await
        .expect("call succeeds");

    std::env::remove_var("ACME_ECHO_TEST_AMBIENT_LEAK");

    let env_pairs = result.data["env"]
        .as_array()
        .expect("env dump is an array")
        .clone();
    let as_map: HashMap<String, String> = env_pairs
        .into_iter()
        .map(|pair| {
            let pair = pair.as_array().unwrap();
            (
                pair[0].as_str().unwrap().to_string(),
                pair[1].as_str().unwrap().to_string(),
            )
        })
        .collect();

    assert_eq!(
        as_map.get("ACME_API_TOKEN").map(String::as_str),
        Some("tok-abc-123"),
        "declared, resolved secret must reach the child by name"
    );
    assert!(
        !as_map.contains_key("ACME_ECHO_TEST_AMBIENT_LEAK"),
        "ambient host env must never leak into a subprocess extension: got {as_map:?}"
    );
    assert_eq!(
        as_map.len(),
        1,
        "child must see NOTHING beyond the declared secret(s): got {as_map:?}"
    );
}

/// A manifest that declares secrets but is activated with no resolver at
/// all fails closed at invoke time — never silently spawns the child
/// without the credential it expects.
#[tokio::test]
async fn subprocess_call_fails_closed_when_declared_secret_has_no_resolver() {
    let manifest = manifest_with_secrets(&["ACME_API_TOKEN"]);
    let registry = install_echo_with_secrets(manifest, None).await;

    let err = registry
        .invoke(
            "acme/echo:call",
            serde_json::json!({"dump_env": true}),
            &ctx("alice"),
        )
        .await
        .expect_err("call must fail closed — no resolver configured for a declared secret");
    assert!(err.to_string().contains("SecretResolver"));
}

/// A manifest that declares a secret name the resolver does not have also
/// fails closed, rather than spawning the child with a partial/empty
/// allowlist.
#[tokio::test]
async fn subprocess_call_fails_closed_when_declared_secret_unresolvable() {
    let manifest = manifest_with_secrets(&["ACME_MISSING_TOKEN"]);
    let resolver: Arc<dyn bastion_types::SecretResolver> =
        Arc::new(MapSecretResolver(HashMap::new()));
    let registry = install_echo_with_secrets(manifest, Some(resolver)).await;

    let err = registry
        .invoke(
            "acme/echo:call",
            serde_json::json!({"dump_env": true}),
            &ctx("alice"),
        )
        .await
        .expect_err("call must fail closed — declared secret could not be resolved");
    assert!(err.to_string().contains("ACME_MISSING_TOKEN"));
}

/// A manifest that declares NO secrets never even asks the resolver — the
/// well-worn zero-secrets path (every existing reference/test extension
/// before this loop) stays exactly as fast and side-effect-free as before.
#[tokio::test]
async fn subprocess_call_with_no_declared_secrets_ignores_missing_resolver() {
    let registry = install_echo_with_secrets(manifest_with_secrets(&[]), None).await;
    let result = registry
        .invoke(
            "acme/echo:call",
            serde_json::json!({"dump_env": true}),
            &ctx("alice"),
        )
        .await
        .expect("call succeeds — no secrets declared, no resolver needed");
    let env_pairs = result.data["env"].as_array().expect("env dump is an array");
    assert!(
        env_pairs.is_empty(),
        "no secrets declared means no env vars at all"
    );
}
