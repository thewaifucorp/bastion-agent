//! `CliCapability` — a GENERIC mechanism that wraps an existing, already-
//! authenticated host CLI binary (git, gh, ...) as a workspace-confined
//! Bastion capability, instead of writing a bespoke REST client or routing
//! through MCP. Built once, reused per tool: a tool gets an entry here
//! (binary + subcommand allowlist + capability name), never a new Rust type.
//!
//! Why this exists instead of MCP for tools like local Git: `bastion-mcp`'s
//! client only speaks remote HTTP (`McpServerEntry.url: String`, no local
//! process transport) — local filesystem operations on the OWNER's own
//! workspace have no remote MCP server that could act on them. A CLI already
//! installed and authenticated on the host (the SAME assumption `gh`/`git`
//! make) is the cheaper, more honest mechanism for exactly this class of
//! tool: no OAuth flow to build, no REST client to maintain — just an
//! allowlisted subprocess call.
//!
//! `Command::args` never goes through a shell (argv passed directly, no
//! string concatenation) — the allowlist rejects an unlisted subcommand
//! before a subprocess is ever spawned, not by sanitizing a shell string.

use async_trait::async_trait;
use bastion_runtime::capability::{Capability, InvokeCtx};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::Command;

/// One CLI binary wrapped as a single Bastion capability. Every allowed
/// subcommand shares this capability's `needs_approval()` — if a tool needs
/// a per-subcommand split (read vs. write, like `git-capability`'s sibling
/// `github-capability` would have), register two `CliCapability` instances
/// under two different `capability_name`s with disjoint subcommand lists,
/// rather than adding per-call approval logic here.
pub struct CliCapability {
    capability_name: String,
    description: String,
    binary: String,
    allowed_subcommands: Vec<String>,
    needs_approval: bool,
    /// Confinement root — every invocation runs with this as `current_dir`,
    /// regardless of anything the caller passes in `args`.
    workspace: PathBuf,
    schema: Value,
}

impl CliCapability {
    pub fn new(
        capability_name: impl Into<String>,
        description: impl Into<String>,
        binary: impl Into<String>,
        allowed_subcommands: Vec<String>,
        needs_approval: bool,
        workspace: impl Into<PathBuf>,
    ) -> Self {
        let schema = json!({
            "type": "object",
            "properties": {
                "subcommand": {
                    "type": "string",
                    "enum": allowed_subcommands,
                },
                "args": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Extra arguments appended after the subcommand"
                }
            },
            "required": ["subcommand"],
            "additionalProperties": false
        });
        Self {
            capability_name: capability_name.into(),
            description: description.into(),
            binary: binary.into(),
            allowed_subcommands,
            needs_approval,
            workspace: workspace.into(),
            schema,
        }
    }

    /// Preset for `bastion-extensions`' `software-sdlc` pack's
    /// `git-capability`: workspace-confined local Git, read/write but never
    /// reaching a remote (no push/remote/fetch/clone — deliberately absent
    /// from the allowlist, not merely undocumented).
    pub fn git(workspace: impl Into<PathBuf>) -> Self {
        Self::new(
            "git",
            "Workspace-confined local Git: init/status/diff/add/commit/branch/log only. \
             No push/remote/fetch/clone — reaching a remote is out of scope for this capability.",
            "git",
            vec![
                "init".to_string(),
                "status".to_string(),
                "diff".to_string(),
                "add".to_string(),
                "commit".to_string(),
                "branch".to_string(),
                "log".to_string(),
            ],
            false,
            workspace,
        )
    }
}

#[async_trait]
impl Capability for CliCapability {
    fn name(&self) -> &str {
        &self.capability_name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn input_schema(&self) -> &Value {
        &self.schema
    }

    /// A wrapped host CLI runs entirely on-host — never leaves the machine
    /// through this capability itself (the CLI's own network calls, e.g.
    /// `git fetch`, are a separate question; this mechanism's allowlist is
    /// what actually keeps a given instance local-only, e.g. `git()` above).
    fn is_local(&self) -> bool {
        true
    }

    fn needs_approval(&self) -> bool {
        self.needs_approval
    }

    async fn invoke(&self, args: Value, _ctx: &InvokeCtx) -> anyhow::Result<Value> {
        let subcommand = args
            .get("subcommand")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("missing 'subcommand'"))?;
        if !self
            .allowed_subcommands
            .iter()
            .any(|allowed| allowed == subcommand)
        {
            anyhow::bail!(
                "{} subcommand '{subcommand}' is not allowed here (allowed: {})",
                self.binary,
                self.allowed_subcommands.join(", ")
            );
        }
        let extra_args: Vec<String> = args
            .get("args")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();

        let mut argv = vec![subcommand.to_string()];
        argv.extend(extra_args);

        let output = Command::new(&self.binary)
            .current_dir(&self.workspace)
            .args(&argv)
            .stdin(Stdio::null())
            .output()
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "failed to spawn '{}' (is it installed on this host?): {e}",
                    self.binary
                )
            })?;

        Ok(json!({
            "subcommand": subcommand,
            "exit_code": output.status.code(),
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn ctx() -> InvokeCtx {
        InvokeCtx {
            owner: "alice".to_string(),
            privacy_tier: Some(bastion_memory::PrivacyTier::LocalOnly),
            allowed_tools: None,
        }
    }

    #[tokio::test]
    async fn git_preset_init_and_status_round_trip_in_workspace() {
        let workspace = TempDir::new().unwrap();
        let cap = CliCapability::git(workspace.path());

        let init = cap
            .invoke(json!({"subcommand": "init"}), &ctx())
            .await
            .unwrap();
        assert_eq!(init["exit_code"], 0, "{init}");
        assert!(workspace.path().join(".git").is_dir());

        let status = cap
            .invoke(json!({"subcommand": "status"}), &ctx())
            .await
            .unwrap();
        assert_eq!(status["exit_code"], 0, "{status}");
    }

    #[tokio::test]
    async fn rejects_subcommand_outside_the_allowlist() {
        let workspace = TempDir::new().unwrap();
        let cap = CliCapability::git(workspace.path());
        let result = cap.invoke(json!({"subcommand": "push"}), &ctx()).await;
        let err = result.expect_err("push must be rejected").to_string();
        assert!(err.contains("not allowed"), "{err}");
    }

    #[tokio::test]
    async fn rejects_missing_subcommand() {
        let workspace = TempDir::new().unwrap();
        let cap = CliCapability::git(workspace.path());
        assert!(cap.invoke(json!({}), &ctx()).await.is_err());
    }

    #[test]
    fn git_preset_is_local_and_needs_no_approval() {
        let cap = CliCapability::git(".");
        assert!(cap.is_local());
        assert!(!cap.needs_approval());
        assert!(cap.is_trusted());
        assert_eq!(cap.name(), "git");
    }

    #[test]
    fn a_hypothetical_write_preset_can_require_approval() {
        // Demonstrates the mechanism is genuinely generic — a future CLI
        // instance (not this pack's) can flip `needs_approval` independent
        // of `git()`'s own false.
        let cap = CliCapability::new(
            "some-write-tool",
            "d",
            "some-tool",
            vec!["mutate".to_string()],
            true,
            ".",
        );
        assert!(cap.needs_approval());
    }
}
