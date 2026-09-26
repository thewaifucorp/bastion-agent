//! End to end, native: the supervisor starts a real sidecar (self-improving)
//! confined with no network, it listens on its Unix socket, and the daemon's
//! MCP client lists its tools through `unix:`.
//!
//! Needs a virtualenv with the sidecar's requirements:
//! `BASTION_SIDECAR_TEST_VENV=/path/to/venv` (e.g. `uv venv` +
//! `uv pip install -r skills/self-improving/requirements.txt`). Skipped
//! without it, and without an OS sandbox backend.

use std::path::Path;

use bastion::config::SidecarsConfig;
use bastion_mcp::McpClient;

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if name == "tests" || name == "__pycache__" || name == ".venv" {
            continue;
        }
        if path.is_dir() {
            copy_dir(&path, &to.join(&name));
        } else {
            std::fs::copy(&path, to.join(&name)).unwrap();
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_native_sidecar_runs_confined_on_a_unix_socket() {
    let Some(venv) = std::env::var_os("BASTION_SIDECAR_TEST_VENV") else {
        eprintln!("skipping: set BASTION_SIDECAR_TEST_VENV to a venv with the sidecar's deps");
        return;
    };
    if bastion::sandbox::init_with_helper(env!("CARGO_BIN_EXE_bastion").into()).is_none() {
        eprintln!("skipping: no sandbox backend on this host");
        return;
    }

    let data = tempfile::tempdir().unwrap();
    let root = data.path().join("sidecars");
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("skills/self-improving"),
        &root.join("src/skills/self-improving"),
    );
    std::fs::create_dir_all(root.join("venv")).unwrap();
    std::os::unix::fs::symlink(&venv, root.join("venv/self-improving")).unwrap();
    let skills = data.path().join("skills");
    std::fs::create_dir_all(&skills).unwrap();
    // This test binary owns its process: nothing else reads these.
    std::env::set_var("BASTION_DATA_DIR", data.path());
    std::env::set_var("SKILLS_DIR", &skills);

    let servers = bastion::sidecars::start(&SidecarsConfig {
        enabled: vec!["self-improving".to_string()],
        root: Some(root.clone()),
    })
    .await;
    let entry = servers.get("self-improving").expect("sidecar registered");
    assert!(entry.url.starts_with("unix:"), "{}", entry.url);
    assert!(
        data.path().join("run/self-improving.sock").exists(),
        "socket never appeared; log: {}",
        std::fs::read_to_string(root.join("logs/self-improving.log")).unwrap_or_default()
    );

    let client = McpClient::connect_from_config(&servers).await.unwrap();
    assert_eq!(
        client.registry().server_for("observe_usage"),
        Some("self-improving"),
        "tools: {:?}",
        client.registry().list_tool_names()
    );
}
