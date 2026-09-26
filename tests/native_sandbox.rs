//! The daemon's own tools under `bastion-sandbox`, with the real `bastion`
//! binary as the helper (the way the daemon runs them). Its own test binary
//! because the detected sandbox is process-global.
//!
//! Skipped without an OS backend unless `BASTION_SANDBOX_TESTS_REQUIRED=1`.

use bastion::extension::cli_capability::CliCapability;
use bastion::extension::facade::{ExtensionInstance, HostFacade};
use bastion::extension::subprocess::SubprocessExtension;
use bastion_extension_protocol::{
    Entrypoint, ExtensionKind, ExtensionManifest, PermissionSet, Provided,
};
use bastion_runtime::capability::{Capability, CapabilityRegistry, InvokeCtx};
use serde_json::json;

fn sandbox_ready() -> bool {
    match bastion::sandbox::init_with_helper(env!("CARGO_BIN_EXE_bastion").into()) {
        Some(sandbox) => {
            eprintln!("backend: {:?}", sandbox.backend());
            true
        }
        None if std::env::var("BASTION_SANDBOX_TESTS_REQUIRED").as_deref() == Ok("1") => {
            panic!("sandbox backend required but unavailable")
        }
        None => {
            eprintln!("skipping: no sandbox backend on this host");
            false
        }
    }
}

fn ctx() -> InvokeCtx {
    InvokeCtx {
        owner: "alice".to_string(),
        privacy_tier: Some(bastion_memory::PrivacyTier::LocalOnly),
        allowed_tools: None,
    }
}

/// A wrapped CLI sees its workspace and nothing of the operator's files.
#[tokio::test]
async fn a_wrapped_cli_is_confined_to_its_workspace() {
    if !sandbox_ready() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("ws");
    let secret = root.path().join("home/.aws/credentials");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(secret.parent().unwrap()).unwrap();
    std::fs::write(&secret, "AWS-SECRET").unwrap();

    let sh = CliCapability::new(
        "sh-probe",
        "d",
        "sh",
        vec!["-c".to_string()],
        vec![],
        false,
        &workspace,
    );
    let script = format!(
        "echo hi > inside.txt && echo WROTE; cat '{}' && echo READ",
        secret.display()
    );
    let out = sh
        .invoke(json!({"subcommand": "-c", "args": [script]}), &ctx())
        .await
        .unwrap();
    let stdout = out["stdout"].as_str().unwrap();
    assert!(stdout.contains("WROTE"), "{out}");
    assert!(workspace.join("inside.txt").exists());
    assert!(
        !stdout.contains("AWS-SECRET") && !stdout.contains("READ"),
        "{out}"
    );
}

/// The git pack works confined end to end.
#[tokio::test]
async fn the_git_pack_commits_under_the_sandbox() {
    if !sandbox_ready() {
        return;
    }
    let workspace = tempfile::tempdir().unwrap();
    let write = CliCapability::git_write(workspace.path());
    for args in [
        json!({"subcommand": "init"}),
        json!({"subcommand": "add", "args": ["f.txt"]}),
        json!({"subcommand": "commit", "args": ["-m", "confined"]}),
    ] {
        if args["subcommand"] == "add" {
            std::fs::write(workspace.path().join("f.txt"), "x").unwrap();
        }
        let out = write.invoke(args, &ctx()).await.unwrap();
        assert_eq!(out["exit_code"], 0, "{out}");
    }
    let log = CliCapability::git(workspace.path())
        .invoke(json!({"subcommand": "log"}), &ctx())
        .await
        .unwrap();
    assert!(
        log["stdout"].as_str().unwrap().contains("confined"),
        "{log}"
    );
}

/// A subprocess extension runs under the sandbox without the unsafe opt-in —
/// on hosts where bubblewrap cannot create namespaces too.
#[tokio::test]
async fn a_subprocess_extension_runs_sandboxed() {
    if !sandbox_ready() {
        return;
    }
    let echo = env!("CARGO_BIN_EXE_reference-extension-echo").to_string();
    let manifest = ExtensionManifest {
        id: "acme/echo".to_string(),
        version: semver::Version::new(1, 0, 0),
        kind: ExtensionKind::Subprocess,
        compat: semver::VersionReq::parse("*").unwrap(),
        provides: vec![Provided::Capability("acme/echo:call".to_string())],
        requires: vec![],
        permissions: PermissionSet {
            capabilities: vec!["acme/echo:call".to_string()],
            ..PermissionSet::none()
        },
        secrets: vec![],
        entrypoint: Entrypoint::Subprocess {
            command: echo.clone(),
            args: vec![],
        },
        migrations: vec![],
        signature: None,
    };
    let ext = SubprocessExtension::new(
        manifest.clone(),
        vec![(
            "acme/echo:call".to_string(),
            "echoes".to_string(),
            json!({}),
            echo,
            vec![],
        )],
    );
    let mut registry = CapabilityRegistry::new();
    {
        let mut facade = HostFacade::new(&manifest, "alice", &mut registry);
        ext.activate(&mut facade).await.unwrap();
    }
    let result = registry
        .invoke(
            "acme/echo:call",
            json!({"hello": "sandbox"}),
            &InvokeCtx {
                privacy_tier: Some(bastion_memory::PrivacyTier::CloudOk),
                ..ctx()
            },
        )
        .await
        .expect("sandboxed subprocess round-trip");
    assert_eq!(result.data["echo"], json!({"hello": "sandbox"}));
}
