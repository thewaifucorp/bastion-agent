//! Where the shell finds its device and the primary it should show.
//!
//! One file, `shell.json`, in the platform config dir
//! (`%APPDATA%\bastion\shell.json` on Windows), written at install/first run:
//!
//! ```json
//! {
//!   "primary_url": "https://linux-box.tailnet.ts.net:8443",
//!   "owner_token": "…",
//!   "stop_command": ["bastion", "node", "stop"]
//! }
//! ```
//!
//! `owner_token` is the paired-device token the web app authenticates with;
//! the shell only injects it into the WebView's localStorage so the embedded
//! app starts signed in. `stop_command`, if set, is what the tray Stop item
//! runs to cut the local node (BMD-15); by default the shell signals the
//! running `bastion node run` process it started itself.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellConfig {
    /// The primary's base URL; the WebView loads `<primary_url>/app`.
    pub primary_url: String,
    /// The paired-device token, injected into the web app's localStorage.
    #[serde(default)]
    pub owner_token: Option<String>,
    /// What the tray Stop item runs. Empty: stop the node process the shell
    /// launched itself.
    #[serde(default)]
    pub stop_command: Vec<String>,
    /// Command that starts the local node the shell supervises. Empty: the
    /// shell only shows the web app and does not run a node.
    #[serde(default)]
    pub node_command: Vec<String>,
}

impl ShellConfig {
    pub fn path() -> anyhow::Result<PathBuf> {
        let dirs = directories::ProjectDirs::from("ai", "bastion", "bastion")
            .ok_or_else(|| anyhow::anyhow!("cannot locate a config directory"))?;
        Ok(dirs.config_dir().join("shell.json"))
    }

    pub fn load() -> anyhow::Result<Self> {
        let path = Self::path()?;
        let bytes = std::fs::read(&path)
            .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", path.display()))?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub fn app_url(&self) -> String {
        format!("{}/app", self.primary_url.trim_end_matches('/'))
    }

    pub fn primary_status_url(&self) -> String {
        format!("{}/devices/primary", self.primary_url.trim_end_matches('/'))
    }
}
