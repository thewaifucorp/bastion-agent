//! Reference pack — real-usage, dogfood (`docs/revamp/C3-extension-protocol-design.md`
//! §6, M4-12). Composes THREE extensions of three DIFFERENT kinds
//! (Declarative + Subprocess + Wasm) into one pack matching the owner's
//! actual Life OS/Developer usage, and drives the full lifecycle end to end:
//!
//!   install → permission review → Loadout resolved → execution → upgrade →
//!   rollback → revoke, with zero orphan at every step.
//!
//! - `mario/life-os-daily-note` (Declarative): a canned daily reflection
//!   prompt — the kind of static, host-read artifact a Life OS skill pack
//!   ships.
//! - `mario/dev-repo-trigger` (Subprocess): a repo-check signal backed by the
//!   real `reference-extension-echo` child process — the kind of "shells out
//!   to a real tool" integration a Developer pack needs.
//! - `mario/budget-calc` (Wasm): a sandboxed daily-budget-remaining
//!   computation — pure arithmetic that never needs to leave the process.

use bastion::extension::declarative::DeclarativeExtension;
use bastion::extension::facade::ExtensionInstance;
use bastion::extension::host::ExtensionHost;
use bastion::extension::review::permission_summary;
use bastion::extension::subprocess::SubprocessExtension;
use bastion::extension::wasm::{WasmExtension, DEFAULT_FUEL};
use bastion_extension_protocol::{
    Entrypoint, ExtensionKind, ExtensionManifest, LoadoutDefaults, PackManifest, PermissionSet,
    Provided,
};
use bastion_runtime::capability::InvokeCtx;
use std::collections::BTreeMap;
use std::sync::Arc;

const DAILY_NOTE_ID: &str = "mario/life-os-daily-note";
const REPO_TRIGGER_ID: &str = "mario/dev-repo-trigger";
const BUDGET_CALC_ID: &str = "mario/budget-calc";

fn daily_note_capability(id: &str) -> String {
    format!("{id}:prompt")
}
fn repo_trigger_capability(id: &str) -> String {
    format!("{id}:check")
}
fn budget_calc_capability(id: &str) -> String {
    format!("{id}:remaining")
}

fn echo_bin() -> String {
    env!("CARGO_BIN_EXE_reference-extension-echo").to_string()
}

const REFERENCE_WASM: &[u8] =
    include_bytes!("../src/extension/wasm_fixtures/reference_extension.wasm");

fn daily_note_manifest(version: (u64, u64, u64)) -> ExtensionManifest {
    let cap = daily_note_capability(DAILY_NOTE_ID);
    ExtensionManifest {
        id: DAILY_NOTE_ID.to_string(),
        version: semver::Version::new(version.0, version.1, version.2),
        kind: ExtensionKind::Declarative,
        compat: semver::VersionReq::parse(&format!(
            "^{}",
            bastion_extension_protocol::PROTOCOL_VERSION
        ))
        .unwrap(),
        provides: vec![Provided::Capability(cap.clone())],
        requires: vec![],
        permissions: PermissionSet {
            capabilities: vec![cap],
            ..PermissionSet::none()
        },
        secrets: vec![],
        entrypoint: Entrypoint::Declarative {
            artifact_path: "daily_note.json".into(),
        },
        migrations: vec![],
        signature: None,
    }
}

fn daily_note_extension(version: (u64, u64, u64)) -> Arc<dyn ExtensionInstance> {
    let manifest = daily_note_manifest(version);
    let cap = daily_note_capability(&manifest.id);
    Arc::new(DeclarativeExtension::new(
        manifest,
        vec![(
            cap,
            "today's Life OS reflection prompt".to_string(),
            serde_json::json!({}),
            serde_json::json!({"prompt": "What's the one thing that would make today great?"}),
        )],
    ))
}

fn repo_trigger_manifest(version: (u64, u64, u64)) -> ExtensionManifest {
    let cap = repo_trigger_capability(REPO_TRIGGER_ID);
    ExtensionManifest {
        id: REPO_TRIGGER_ID.to_string(),
        version: semver::Version::new(version.0, version.1, version.2),
        kind: ExtensionKind::Subprocess,
        compat: semver::VersionReq::parse(&format!(
            "^{}",
            bastion_extension_protocol::PROTOCOL_VERSION
        ))
        .unwrap(),
        provides: vec![Provided::Capability(cap.clone())],
        requires: vec![],
        permissions: PermissionSet {
            capabilities: vec![cap],
            ..PermissionSet::none()
        },
        secrets: vec![],
        entrypoint: Entrypoint::Subprocess {
            command: echo_bin(),
            args: vec![],
        },
        migrations: vec![],
        signature: None,
    }
}

fn repo_trigger_extension(version: (u64, u64, u64)) -> Arc<dyn ExtensionInstance> {
    let manifest = repo_trigger_manifest(version);
    let cap = repo_trigger_capability(&manifest.id);
    Arc::new(SubprocessExtension::new(
        manifest,
        vec![(
            cap,
            "checks the dev repo for a trigger signal".to_string(),
            serde_json::json!({}),
            echo_bin(),
            vec![],
        )],
    ))
}

fn budget_calc_manifest(version: (u64, u64, u64)) -> ExtensionManifest {
    let cap = budget_calc_capability(BUDGET_CALC_ID);
    ExtensionManifest {
        id: BUDGET_CALC_ID.to_string(),
        version: semver::Version::new(version.0, version.1, version.2),
        kind: ExtensionKind::Wasm,
        compat: semver::VersionReq::parse(&format!(
            "^{}",
            bastion_extension_protocol::PROTOCOL_VERSION
        ))
        .unwrap(),
        provides: vec![Provided::Capability(cap.clone())],
        requires: vec![],
        permissions: PermissionSet {
            capabilities: vec![cap],
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

fn budget_calc_extension(version: (u64, u64, u64)) -> Arc<dyn ExtensionInstance> {
    let manifest = budget_calc_manifest(version);
    let cap = budget_calc_capability(&manifest.id);
    Arc::new(
        WasmExtension::new(
            manifest,
            vec![(
                cap,
                "daily budget remaining = total + (-spent), inside a wasm sandbox".to_string(),
                serde_json::json!({}),
                REFERENCE_WASM.to_vec(),
                "add".to_string(),
                DEFAULT_FUEL,
            )],
        )
        .expect("WasmExtension::new should succeed"),
    )
}

/// The instance's own grant — a real personal-agent instance would derive
/// this from `AgentDefinition` (design doc §4). Exactly the three
/// capability names the pack needs, nothing more — proves the pack doesn't
/// need (and doesn't get) any broader authority than its members declare.
fn instance_ceiling() -> PermissionSet {
    PermissionSet {
        capabilities: vec![
            daily_note_capability(DAILY_NOTE_ID),
            repo_trigger_capability(REPO_TRIGGER_ID),
            budget_calc_capability(BUDGET_CALC_ID),
        ],
        ..PermissionSet::none()
    }
}

/// `CloudOk` — this pack mixes a genuinely `is_local()==false` mechanism
/// (`Subprocess`: the child COULD reach the network directly since there is
/// no OS-level network-namespace sandbox here, only the voluntary
/// host-mediated egress protocol — see `src/extension/subprocess.rs`'s
/// `is_local()` doc) alongside two `is_local()==true` ones. A single shared
/// `ctx` exercising all three capabilities together needs the tier that
/// clears the egress gate for BOTH classes; `declarative.rs`/`wasm.rs`'s own
/// unit tests separately prove the `is_local()==true` mechanisms also work
/// under `LocalOnly`.
fn ctx(owner: &str) -> InvokeCtx {
    InvokeCtx {
        owner: owner.to_string(),
        privacy_tier: Some(bastion_memory::PrivacyTier::CloudOk),
    }
}

#[tokio::test]
async fn life_os_developer_pack_full_lifecycle_with_zero_orphan() {
    let owner = "mario";
    let ceiling = instance_ceiling();

    // --- Step 1: compose the pack, resolve it against a catalog ------------
    let catalog: BTreeMap<String, Arc<dyn ExtensionInstance>> = BTreeMap::from([
        (DAILY_NOTE_ID.to_string(), daily_note_extension((1, 0, 0))),
        (
            REPO_TRIGGER_ID.to_string(),
            repo_trigger_extension((1, 0, 0)),
        ),
        (BUDGET_CALC_ID.to_string(), budget_calc_extension((1, 0, 0))),
    ]);
    let pack = PackManifest {
        id: "mario/life-os-developer-pack".to_string(),
        version: semver::Version::new(1, 0, 0),
        extensions: vec![
            (
                DAILY_NOTE_ID.to_string(),
                semver::VersionReq::parse("^1").unwrap(),
            ),
            (
                REPO_TRIGGER_ID.to_string(),
                semver::VersionReq::parse("^1").unwrap(),
            ),
            (
                BUDGET_CALC_ID.to_string(),
                semver::VersionReq::parse("^1").unwrap(),
            ),
        ],
        skills: vec![],
        personas: vec![],
        defaults: LoadoutDefaults {
            enabled_extensions: vec![
                DAILY_NOTE_ID.to_string(),
                REPO_TRIGGER_ID.to_string(),
                BUDGET_CALC_ID.to_string(),
            ],
        },
    };

    let resolved = ExtensionHost::resolve_pack(&pack, &catalog, &ceiling)
        .expect("pack resolves — every member's permissions fit the instance ceiling");
    assert_eq!(resolved.len(), 3, "3 extensions, 3 different kinds");

    // --- Step 2: permission review (M4-09) — BEFORE installing anything ----
    for instance in &resolved {
        let summary = permission_summary(instance.manifest());
        assert!(summary.contains(&instance.manifest().id));
        assert!(summary.contains("Trust tier:"));
        assert!(summary.contains("capabilities:"));
    }

    // --- Step 3: install (atomic per extension) -----------------------------
    let mut host = ExtensionHost::new();
    for instance in resolved {
        host.install(instance, owner, &ceiling)
            .await
            .unwrap_or_else(|e| panic!("install of a resolved pack member must not fail: {e}"));
    }

    // --- Step 4: Loadout resolved --------------------------------------------
    let loadout = host.loadout();
    let mut ids: Vec<&str> = loadout
        .extensions
        .iter()
        .map(|(id, _)| id.as_str())
        .collect();
    ids.sort_unstable();
    let mut expected_ids = vec![BUDGET_CALC_ID, DAILY_NOTE_ID, REPO_TRIGGER_ID];
    expected_ids.sort_unstable();
    assert_eq!(
        ids, expected_ids,
        "the resolved Loadout contains exactly the pack's 3 members"
    );

    // --- Step 5: execution — invoke all 3, one per kind ---------------------
    let daily_note = host
        .registry()
        .invoke(
            &daily_note_capability(DAILY_NOTE_ID),
            serde_json::json!({}),
            &ctx(owner),
        )
        .await
        .expect("declarative capability invokes");
    assert_eq!(
        daily_note.data["prompt"],
        serde_json::json!("What's the one thing that would make today great?")
    );
    assert!(
        daily_note.trusted,
        "declarative data is host-wrapped, trusted"
    );

    let repo_check = host
        .registry()
        .invoke(
            &repo_trigger_capability(REPO_TRIGGER_ID),
            serde_json::json!({"repo": "bastion"}),
            &ctx(owner),
        )
        .await
        .expect("subprocess capability invokes via the real child process");
    assert_eq!(
        repo_check.data["echo"],
        serde_json::json!({"repo": "bastion"})
    );
    assert!(
        !repo_check.trusted,
        "subprocess output defaults to untrusted"
    );

    let budget = host
        .registry()
        .invoke(
            &budget_calc_capability(BUDGET_CALC_ID),
            serde_json::json!({"a": 100, "b": -35}),
            &ctx(owner),
        )
        .await
        .expect("wasm capability invokes via the sandbox");
    assert_eq!(budget.data, serde_json::json!({"result": 65}));
    assert!(
        budget.trusted,
        "wasm execution never leaves the process, trusted"
    );

    // --- Step 6: upgrade one member (budget-calc 1.0.0 -> 1.1.0) ------------
    host.upgrade(budget_calc_extension((1, 1, 0)), &ceiling)
        .await
        .expect("compatible upgrade should succeed");
    assert_eq!(
        host.lock().find(BUDGET_CALC_ID).unwrap().version,
        semver::Version::new(1, 1, 0)
    );
    let budget_after_upgrade = host
        .registry()
        .invoke(
            &budget_calc_capability(BUDGET_CALC_ID),
            serde_json::json!({"a": 100, "b": -35}),
            &ctx(owner),
        )
        .await
        .expect("upgraded wasm capability still invokes correctly");
    assert_eq!(budget_after_upgrade.data, serde_json::json!({"result": 65}));

    // --- Step 7: rollback the upgrade ----------------------------------------
    host.rollback(BUDGET_CALC_ID)
        .await
        .expect("rollback to the previous version should succeed");
    assert_eq!(
        host.lock().find(BUDGET_CALC_ID).unwrap().version,
        semver::Version::new(1, 0, 0),
        "rollback restores the version that was active before the upgrade"
    );
    assert!(host
        .registry()
        .list_names()
        .contains(&budget_calc_capability(BUDGET_CALC_ID).as_str()));

    // --- Step 8: revoke everything — zero orphan -----------------------------
    for id in [DAILY_NOTE_ID, REPO_TRIGGER_ID, BUDGET_CALC_ID] {
        host.revoke(id).await.expect("revoke should succeed");
    }
    assert!(
        host.registry().list_names().is_empty(),
        "zero orphan capability after revoking every pack member"
    );
    assert!(host.lock().find(DAILY_NOTE_ID).is_none());
    assert!(host.lock().find(REPO_TRIGGER_ID).is_none());
    assert!(host.lock().find(BUDGET_CALC_ID).is_none());
    assert!(!host.is_installed(DAILY_NOTE_ID));
    assert!(!host.is_installed(REPO_TRIGGER_ID));
    assert!(!host.is_installed(BUDGET_CALC_ID));
}

/// Pack composition never amplifies authority (§4/§6): a hypothetical 4th
/// member asking for `network_bind` — which NONE of the real pack members
/// need and the instance ceiling above does not grant — is rejected by
/// resolution before any install happens, and does not affect the other 3
/// members' ability to resolve.
#[tokio::test]
async fn pack_cannot_smuggle_in_a_member_exceeding_the_instance_ceiling() {
    let greedy_id = "mario/would-be-listener";
    let greedy_cap = format!("{greedy_id}:listen");
    let greedy_manifest = ExtensionManifest {
        id: greedy_id.to_string(),
        version: semver::Version::new(1, 0, 0),
        kind: ExtensionKind::Declarative,
        compat: semver::VersionReq::parse("*").unwrap(),
        provides: vec![Provided::Capability(greedy_cap.clone())],
        requires: vec![],
        permissions: PermissionSet {
            capabilities: vec![greedy_cap.clone()],
            network_bind: true, // NOT granted by instance_ceiling()
            ..PermissionSet::none()
        },
        secrets: vec![],
        entrypoint: Entrypoint::Declarative {
            artifact_path: "listener.json".into(),
        },
        migrations: vec![],
        signature: None,
    };

    let mut catalog: BTreeMap<String, Arc<dyn ExtensionInstance>> =
        BTreeMap::from([(DAILY_NOTE_ID.to_string(), daily_note_extension((1, 0, 0)))]);
    catalog.insert(
        greedy_id.to_string(),
        Arc::new(DeclarativeExtension::new(
            greedy_manifest,
            vec![(
                greedy_cap,
                "d".to_string(),
                serde_json::json!({}),
                serde_json::json!({}),
            )],
        )),
    );

    let pack = PackManifest {
        id: "mario/life-os-developer-pack-plus-greedy".to_string(),
        version: semver::Version::new(1, 0, 0),
        extensions: vec![
            (
                DAILY_NOTE_ID.to_string(),
                semver::VersionReq::parse("^1").unwrap(),
            ),
            (
                greedy_id.to_string(),
                semver::VersionReq::parse("^1").unwrap(),
            ),
        ],
        skills: vec![],
        personas: vec![],
        defaults: Default::default(),
    };

    let result = ExtensionHost::resolve_pack(&pack, &catalog, &instance_ceiling());
    match result {
        Err(bastion_extension_protocol::ExtensionError::AuthorityEscalation {
            extension, ..
        }) => assert_eq!(extension, greedy_id),
        Err(other) => {
            panic!("expected AuthorityEscalation naming the greedy member, got {other:?}")
        }
        Ok(_) => panic!("expected AuthorityEscalation, pack resolution unexpectedly succeeded"),
    }
}
