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
    /// `(subcommand, flag)` pairs explicitly permitted to look like a flag
    /// (start with `-`). SECURITY: everything else starting with `-` is
    /// rejected before a subprocess is ever spawned — an allowlisted
    /// subcommand is not itself permission to pass ANY flag to it. Without
    /// this, a caller could smuggle e.g. `git log --output=/etc/cron.d/evil`
    /// (git's `--output` writes to an arbitrary path, escaping the
    /// workspace confinement entirely) through a subcommand that's
    /// otherwise perfectly safe.
    allowed_flags: Vec<(String, String)>,
    needs_approval: bool,
    /// Confinement root — every invocation runs with this as `current_dir`,
    /// regardless of anything the caller passes in `args`.
    workspace: PathBuf,
    /// The child's ENTIRE environment, on top of `PATH` and a locale. The
    /// daemon's own environment (provider keys, cloud credentials, the
    /// operator's shell variables) never reaches a wrapped CLI.
    env: Vec<(String, String)>,
    /// Arguments placed before the subcommand, for options the caller must
    /// not be able to turn off (e.g. git's `-c core.hooksPath=/dev/null`).
    pre_args: Vec<String>,
    schema: Value,
}

/// Inherited from the daemon so the binary and its dynamic libraries resolve
/// and messages are readable. Nothing else is.
const INHERITED_ENV: &[&str] = &["PATH", "LANG", "LC_ALL"];

impl CliCapability {
    pub fn new(
        capability_name: impl Into<String>,
        description: impl Into<String>,
        binary: impl Into<String>,
        allowed_subcommands: Vec<String>,
        allowed_flags: Vec<(String, String)>,
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
                    "description": "Extra arguments appended after the subcommand — any \
                                     '-'-prefixed value must be in this capability's flag \
                                     allowlist for that subcommand, or invoke() rejects it"
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
            allowed_flags,
            needs_approval,
            workspace: workspace.into(),
            env: Vec::new(),
            pre_args: Vec::new(),
            schema,
        }
    }

    /// Set the child's environment (beyond [`INHERITED_ENV`]).
    pub fn with_env(mut self, env: Vec<(String, String)>) -> Self {
        self.env = env;
        self
    }

    /// Arguments forced before every subcommand.
    pub fn with_pre_args(mut self, pre_args: Vec<String>) -> Self {
        self.pre_args = pre_args;
        self
    }

    fn flag_is_allowed(&self, subcommand: &str, flag: &str) -> bool {
        self.allowed_flags
            .iter()
            .any(|(sc, f)| sc == subcommand && f == flag)
    }

    /// Preset for `bastion-extensions`' `software-sdlc` pack's
    /// `git-capability`, read side: `status`/`diff`/`log` in the workspace,
    /// no approval. Writes live in [`Self::git_write`], behind approval.
    /// Every flag is rejected (e.g. `log --output=<path>`, which writes
    /// anywhere).
    pub fn git(workspace: impl Into<PathBuf>) -> Self {
        let workspace = workspace.into();
        Self::new(
            "git",
            "Workspace-confined local Git, read-only: status/diff/log. Changes go through \
             git_write, which needs approval.",
            "git",
            vec!["status".to_string(), "diff".to_string(), "log".to_string()],
            Vec::new(),
            false,
            workspace.clone(),
        )
        .with_env(git_env(&workspace))
        .with_pre_args(git_pre_args())
    }

    /// The write side of the git preset: `init`/`add`/`commit`/`branch`,
    /// each one approved by the operator. `-m`/`--message` on `commit` is the
    /// only flag allowed. Never reaches a remote: `push`/`fetch`/`clone`/
    /// `remote` are absent from both presets, not merely undocumented.
    pub fn git_write(workspace: impl Into<PathBuf>) -> Self {
        let workspace = workspace.into();
        Self::new(
            "git_write",
            "Workspace-confined local Git changes: init/add/commit/branch. Each call needs \
             approval. No push/remote/fetch/clone.",
            "git",
            vec![
                "init".to_string(),
                "add".to_string(),
                "commit".to_string(),
                "branch".to_string(),
            ],
            vec![
                ("commit".to_string(), "-m".to_string()),
                ("commit".to_string(), "--message".to_string()),
            ],
            true,
            workspace.clone(),
        )
        .with_env(git_env(&workspace))
        .with_pre_args(git_pre_args())
    }
}

/// Git reads config from the system, from `$HOME`/`$XDG_CONFIG_HOME`, and
/// from the repository. The first two are the operator's, not the
/// workspace's (a credential helper, an `sshCommand`, an alias) and are
/// switched off; `HOME` points at the workspace so nothing resolves to the
/// real home. The author is fixed because the global config that would have
/// supplied it is gone.
fn git_env(workspace: &std::path::Path) -> Vec<(String, String)> {
    let home = workspace.to_string_lossy().into_owned();
    [
        ("HOME", home.as_str()),
        ("GIT_CONFIG_NOSYSTEM", "1"),
        ("GIT_CONFIG_GLOBAL", "/dev/null"),
        ("GIT_TERMINAL_PROMPT", "0"),
        ("GIT_PAGER", "cat"),
        ("GIT_AUTHOR_NAME", "Bastion"),
        ("GIT_AUTHOR_EMAIL", "bastion@localhost"),
        ("GIT_COMMITTER_NAME", "Bastion"),
        ("GIT_COMMITTER_EMAIL", "bastion@localhost"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect()
}

/// Repository-controlled code execution git would otherwise perform on the
/// subcommands above: hooks (on `commit`), the fsmonitor daemon (on
/// `status`/`add`), external diff and textconv drivers (on `diff`/`log`).
/// `-c` on the command line outranks the repository's own `.git/config`.
/// Clean/smudge filters configured in `.git/config` still run on `add`; the
/// OS sandbox, not argv, is what contains those.
fn git_pre_args() -> Vec<String> {
    [
        "-c",
        "core.hooksPath=/dev/null",
        "-c",
        "core.fsmonitor=false",
        "-c",
        "diff.external=",
        "-c",
        "core.pager=cat",
        "--no-pager",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

impl CliCapability {
    /// The CLI invocation: confined by [`crate::sandbox`] when this host has
    /// a backend (the workspace is the only writable path and there is no
    /// network — git never needs one for these subcommands), plain
    /// otherwise. `env` is the child's entire environment.
    fn command(&self, argv: &[String], env: Vec<(String, String)>) -> anyhow::Result<Command> {
        if let Some(sandbox) = crate::sandbox::current() {
            let binary = crate::sandbox::resolve_on_path(&self.binary).ok_or_else(|| {
                anyhow::anyhow!("'{}' is not installed on this host", self.binary)
            })?;
            let spec = bastion_sandbox::SandboxSpec::new(binary)
                .args(argv)
                .envs(env)
                .cwd(&self.workspace)
                .read_write(&self.workspace)
                .network(bastion_sandbox::Network::Blocked);
            return Ok(Command::from(sandbox.command(&spec)?));
        }
        let mut command = Command::new(&self.binary);
        command
            .env_clear()
            .envs(env)
            .current_dir(&self.workspace)
            .args(argv);
        Ok(command)
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

        // SECURITY: an allowlisted subcommand is not permission to pass ANY
        // flag to it — reject anything '-'-prefixed unless this instance's
        // allowlist names it for exactly this subcommand. Closes argv
        // flag-smuggling (e.g. `git log --output=/etc/cron.d/evil` writing
        // outside the workspace via a subcommand that's otherwise safe).
        for arg in &extra_args {
            if arg.starts_with('-') && !self.flag_is_allowed(subcommand, arg) {
                anyhow::bail!(
                    "{} arg '{arg}' looks like a flag and is not allowed for subcommand \
                     '{subcommand}'",
                    self.binary
                );
            }
        }

        let mut argv = self.pre_args.clone();
        argv.push(subcommand.to_string());
        argv.extend(extra_args);

        let env: Vec<(String, String)> = INHERITED_ENV
            .iter()
            .filter_map(|key| std::env::var(key).ok().map(|v| (key.to_string(), v)))
            .chain(self.env.iter().cloned())
            .collect();
        let mut command = self.command(&argv, env)?;
        let output = command.stdin(Stdio::null()).output().await.map_err(|e| {
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
    async fn git_write_init_then_git_status_round_trip_in_workspace() {
        let workspace = TempDir::new().unwrap();
        let init = CliCapability::git_write(workspace.path())
            .invoke(json!({"subcommand": "init"}), &ctx())
            .await
            .unwrap();
        assert_eq!(init["exit_code"], 0, "{init}");
        assert!(workspace.path().join(".git").is_dir());

        let status = CliCapability::git(workspace.path())
            .invoke(json!({"subcommand": "status"}), &ctx())
            .await
            .unwrap();
        assert_eq!(status["exit_code"], 0, "{status}");
    }

    #[tokio::test]
    async fn rejects_subcommand_outside_the_allowlist() {
        let workspace = TempDir::new().unwrap();
        for cap in [
            CliCapability::git(workspace.path()),
            CliCapability::git_write(workspace.path()),
        ] {
            let err = cap
                .invoke(json!({"subcommand": "push"}), &ctx())
                .await
                .expect_err("push must be rejected")
                .to_string();
            assert!(err.contains("not allowed"), "{err}");
        }
        // The read side cannot write.
        let err = CliCapability::git(workspace.path())
            .invoke(json!({"subcommand": "commit"}), &ctx())
            .await
            .expect_err("commit is not a read")
            .to_string();
        assert!(err.contains("not allowed"), "{err}");
    }

    #[tokio::test]
    async fn rejects_unlisted_flag_even_on_an_allowed_subcommand() {
        let workspace = TempDir::new().unwrap();
        CliCapability::git_write(workspace.path())
            .invoke(json!({"subcommand": "init"}), &ctx())
            .await
            .unwrap();

        // `git log --output=<path>` writes to an arbitrary filesystem path —
        // exactly the argv flag-smuggling vector the allowlist exists to close.
        let target = workspace.path().join("should-not-be-written");
        let result = CliCapability::git(workspace.path())
            .invoke(
                json!({"subcommand": "log", "args": [format!("--output={}", target.display())]}),
                &ctx(),
            )
            .await;
        let err = result
            .expect_err("unlisted flag must be rejected")
            .to_string();
        assert!(err.contains("not allowed"), "{err}");
        assert!(!target.exists());
    }

    /// Commits without any identity configured anywhere: the preset supplies
    /// it, since the operator's global config is deliberately out of reach.
    #[tokio::test]
    async fn git_write_commits_with_the_one_allowlisted_message_flag() {
        let workspace = TempDir::new().unwrap();
        let write = CliCapability::git_write(workspace.path());
        write
            .invoke(json!({"subcommand": "init"}), &ctx())
            .await
            .unwrap();
        std::fs::write(workspace.path().join("f.txt"), "x").unwrap();
        write
            .invoke(json!({"subcommand": "add", "args": ["f.txt"]}), &ctx())
            .await
            .unwrap();
        let commit = write
            .invoke(
                json!({"subcommand": "commit", "args": ["-m", "test commit"]}),
                &ctx(),
            )
            .await
            .unwrap();
        assert_eq!(commit["exit_code"], 0, "{commit}");

        let log = CliCapability::git(workspace.path())
            .invoke(json!({"subcommand": "log"}), &ctx())
            .await
            .unwrap();
        assert!(
            log["stdout"]
                .as_str()
                .unwrap()
                .contains("Bastion <bastion@localhost>"),
            "{log}"
        );
    }

    /// A hook committed into the repository must not run: repository content
    /// is data the agent handles, not code it executes.
    #[cfg(unix)]
    #[tokio::test]
    async fn repository_hooks_do_not_run() {
        use std::os::unix::fs::PermissionsExt;
        let workspace = TempDir::new().unwrap();
        let write = CliCapability::git_write(workspace.path());
        write
            .invoke(json!({"subcommand": "init"}), &ctx())
            .await
            .unwrap();
        let marker = workspace.path().join("hook-ran");
        let hook = workspace.path().join(".git/hooks/pre-commit");
        std::fs::write(&hook, format!("#!/bin/sh\ntouch '{}'\n", marker.display())).unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(workspace.path().join("f.txt"), "x").unwrap();
        write
            .invoke(json!({"subcommand": "add", "args": ["f.txt"]}), &ctx())
            .await
            .unwrap();
        let commit = write
            .invoke(json!({"subcommand": "commit", "args": ["-m", "m"]}), &ctx())
            .await
            .unwrap();
        assert_eq!(commit["exit_code"], 0, "{commit}");
        assert!(!marker.exists(), "the repository's pre-commit hook ran");
    }

    /// The daemon's environment does not reach the wrapped CLI.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_child_environment_is_the_declared_one_only() {
        let workspace = TempDir::new().unwrap();
        std::env::set_var("BASTION_TEST_CANARY_SECRET", "must-not-leak");
        let env_dump = CliCapability::new(
            "env-dump",
            "d",
            "env",
            vec!["-0".to_string()],
            vec![],
            false,
            workspace.path(),
        )
        .with_env(vec![("DECLARED".to_string(), "yes".to_string())])
        .invoke(json!({"subcommand": "-0"}), &ctx())
        .await
        .unwrap();
        std::env::remove_var("BASTION_TEST_CANARY_SECRET");
        let dumped = env_dump["stdout"].as_str().unwrap();
        assert!(dumped.contains("DECLARED=yes"), "{dumped}");
        assert!(!dumped.contains("must-not-leak"), "{dumped}");
    }

    #[tokio::test]
    async fn rejects_missing_subcommand() {
        let workspace = TempDir::new().unwrap();
        let cap = CliCapability::git(workspace.path());
        assert!(cap.invoke(json!({}), &ctx()).await.is_err());
    }

    #[test]
    fn git_reads_need_no_approval_and_writes_do() {
        let read = CliCapability::git(".");
        assert!(read.is_local());
        assert!(!read.needs_approval());
        assert!(read.is_trusted());
        assert_eq!(read.name(), "git");

        let write = CliCapability::git_write(".");
        assert!(write.is_local());
        assert!(write.needs_approval());
        assert_eq!(write.name(), "git_write");
    }
}
