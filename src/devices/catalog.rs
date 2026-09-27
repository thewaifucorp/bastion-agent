//! The primitives a node offers (spec §9 decision 3): run a program and
//! read and write files in granted folders. Operating a specific app is a
//! capability pack built on these, never core.
//!
//! The same catalog serves both sides: the primary registers a remote tool
//! for every granted name it knows ([`descriptors`]), the node runs them
//! ([`node_capabilities`]). Every primitive enforces its grant's scope on
//! the node, before doing anything.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bastion_mesh::devices::{
    ApprovalRef, CapabilityDescriptor, CapabilityGrant, Evidence, GrantScope, InvokeError,
    NodeCapability,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

pub const SYSTEM_RUN: &str = "system.run";
pub const FILE_READ: &str = "file.read";
pub const FILE_WRITE: &str = "file.write";

/// Longest output returned to the primary, per stream.
const MAX_OUTPUT: usize = 64 * 1024;
const MAX_READ: u64 = 256 * 1024;
const RUN_TIMEOUT: Duration = Duration::from_secs(120);

/// Every primitive's descriptor, known to the primary without asking a node.
/// Platform-independent on purpose: the primary (say, Linux) registers the
/// tools of a Windows node from this list, so the UI Automation primitives
/// are always here. Only [`node_capabilities`] is limited to what this host
/// can run; a node asked for something it lacks answers `UnknownCapability`.
pub fn descriptors() -> Vec<CapabilityDescriptor> {
    vec![
        SystemRun.descriptor(),
        FileRead.descriptor(),
        FileWrite.descriptor(),
        super::ui::snapshot_descriptor(),
        super::ui::act_descriptor(),
    ]
}

/// The primitives this node can actually run.
pub fn node_capabilities() -> Vec<Arc<dyn NodeCapability>> {
    #[allow(unused_mut)]
    let mut capabilities: Vec<Arc<dyn NodeCapability>> =
        vec![Arc::new(SystemRun), Arc::new(FileRead), Arc::new(FileWrite)];
    #[cfg(windows)]
    {
        // BMD-13/BMD-14: the UI Automation primitives share one snapshot store
        // (a snapshot taken by `ui.snapshot` is what `ui.act` references) and
        // one backend (the out-of-workspace `desktop/uia` helper).
        let backend: Arc<dyn super::ui::UiaBackend> =
            Arc::new(super::ui::SubprocessBackend::locate());
        let store = Arc::new(super::ui::SnapshotStore::new());
        capabilities.push(Arc::new(super::ui::UiSnapshot::new(
            backend.clone(),
            store.clone(),
        )));
        capabilities.push(Arc::new(super::ui::UiAct::new(backend, store)));
    }
    capabilities
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn failed(message: impl Into<String>) -> InvokeError {
    InvokeError::Failed(message.into())
}

fn arg_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, InvokeError> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| failed(format!("missing string argument {key:?}")))
}

/// The granted root that contains `path` (canonical), or `OutOfScope`.
/// For a path that does not exist yet (a file to create), its parent must.
pub(crate) fn scoped_path(
    scope: &GrantScope,
    path: &str,
) -> Result<(PathBuf, PathBuf), InvokeError> {
    let requested = Path::new(path);
    if !requested.is_absolute() {
        return Err(InvokeError::OutOfScope(format!("{path} is not absolute")));
    }
    let canonical = match std::fs::canonicalize(requested) {
        Ok(canonical) => canonical,
        Err(_) => {
            let parent = requested
                .parent()
                .and_then(|p| std::fs::canonicalize(p).ok())
                .ok_or_else(|| InvokeError::OutOfScope(format!("{path}: no such directory")))?;
            let name = requested
                .file_name()
                .ok_or_else(|| InvokeError::OutOfScope(format!("{path}: no file name")))?;
            parent.join(name)
        }
    };
    let roots: Vec<PathBuf> = match scope {
        GrantScope::Paths(roots) => roots
            .iter()
            .filter_map(|r| std::fs::canonicalize(r).ok())
            .collect(),
        GrantScope::Any => return Ok((canonical.clone(), canonical)),
        GrantScope::Apps(_) => Vec::new(),
    };
    roots
        .into_iter()
        .find(|root| canonical.starts_with(root))
        .map(|root| (canonical.clone(), root))
        .ok_or_else(|| InvokeError::OutOfScope(format!("{path} is outside the granted folders")))
}

fn truncate(bytes: &[u8]) -> String {
    let cut = bytes.len().min(MAX_OUTPUT);
    let mut text = String::from_utf8_lossy(&bytes[..cut]).into_owned();
    if bytes.len() > cut {
        text.push_str("\n[… truncated]");
    }
    text
}

/// Run a program, confined by the OS sandbox, in a granted folder.
pub struct SystemRun;

#[async_trait]
impl NodeCapability for SystemRun {
    fn descriptor(&self) -> CapabilityDescriptor {
        CapabilityDescriptor {
            name: SYSTEM_RUN.into(),
            description: "Run a program on this device, confined to a granted folder with no \
                          network. Returns exit code, stdout and stderr."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "argv": {"type": "array", "items": {"type": "string"}, "minItems": 1,
                             "description": "Absolute program path, then its arguments"},
                    "cwd": {"type": "string", "description": "Absolute folder inside a granted path"}
                },
                "required": ["argv", "cwd"]
            }),
        }
    }

    async fn run(
        &self,
        args: Value,
        grant: &CapabilityGrant,
        _approval: Option<&ApprovalRef>,
        cancel: CancellationToken,
    ) -> Result<(Value, Vec<Evidence>), InvokeError> {
        let argv: Vec<String> = args
            .get("argv")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .filter(|a: &Vec<String>| !a.is_empty())
            .ok_or_else(|| failed("argv must be a non-empty array of strings"))?;
        let (cwd, root) = scoped_path(&grant.scope, arg_str(&args, "cwd")?)?;
        // BMD-01: a node without an OS sandbox runs nothing.
        let sandbox = crate::sandbox::current()
            .ok_or_else(|| failed("this device has no OS sandbox; it runs no programs"))?;
        let mut spec = bastion_sandbox::SandboxSpec::new(&argv[0])
            .args(&argv[1..])
            .cwd(&cwd)
            .read_write(&root);
        if let Some(path) = std::env::var_os("PATH") {
            spec = spec.env("PATH", path.to_string_lossy());
        }
        #[cfg(windows)]
        if let Ok(root) = std::env::var("SystemRoot") {
            spec = spec.env("SystemRoot", root);
        }
        let command = sandbox
            .command(&spec)
            .map_err(|e| failed(format!("cannot confine {}: {e}", argv[0])))?;
        let mut command = tokio::process::Command::from(command);
        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let child = command
            .spawn()
            .map_err(|e| failed(format!("cannot start {}: {e}", argv[0])))?;
        let output = tokio::select! {
            output = child.wait_with_output() => output.map_err(|e| failed(e.to_string()))?,
            () = cancel.cancelled() => return Err(InvokeError::Cancelled),
            () = tokio::time::sleep(RUN_TIMEOUT) => return Err(failed("timed out")),
        };
        let result = json!({
            "exit_code": output.status.code(),
            "stdout": truncate(&output.stdout),
            "stderr": truncate(&output.stderr),
        });
        let evidence = vec![Evidence::Text {
            text: format!(
                "ran {} in {} (exit {:?})",
                argv[0],
                cwd.display(),
                output.status.code()
            ),
        }];
        Ok((result, evidence))
    }
}

pub struct FileRead;

#[async_trait]
impl NodeCapability for FileRead {
    fn descriptor(&self) -> CapabilityDescriptor {
        CapabilityDescriptor {
            name: FILE_READ.into(),
            description: "Read a text file inside a granted folder on this device.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"]
            }),
        }
    }

    async fn run(
        &self,
        args: Value,
        grant: &CapabilityGrant,
        _approval: Option<&ApprovalRef>,
        _cancel: CancellationToken,
    ) -> Result<(Value, Vec<Evidence>), InvokeError> {
        let (path, _) = scoped_path(&grant.scope, arg_str(&args, "path")?)?;
        let len = std::fs::metadata(&path)
            .map_err(|e| failed(format!("{}: {e}", path.display())))?
            .len();
        if len > MAX_READ {
            return Err(failed(format!(
                "{} is larger than {MAX_READ} bytes",
                path.display()
            )));
        }
        let bytes = std::fs::read(&path).map_err(|e| failed(e.to_string()))?;
        let sha = sha256_hex(&bytes);
        Ok((
            json!({"path": path, "content": String::from_utf8_lossy(&bytes)}),
            vec![Evidence::File {
                path: path.to_string_lossy().into_owned(),
                sha256: sha,
            }],
        ))
    }
}

pub struct FileWrite;

#[async_trait]
impl NodeCapability for FileWrite {
    fn descriptor(&self) -> CapabilityDescriptor {
        CapabilityDescriptor {
            name: FILE_WRITE.into(),
            description: "Write a text file inside a granted folder on this device.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {"path": {"type": "string"}, "content": {"type": "string"}},
                "required": ["path", "content"]
            }),
        }
    }

    async fn run(
        &self,
        args: Value,
        grant: &CapabilityGrant,
        _approval: Option<&ApprovalRef>,
        _cancel: CancellationToken,
    ) -> Result<(Value, Vec<Evidence>), InvokeError> {
        let (path, _) = scoped_path(&grant.scope, arg_str(&args, "path")?)?;
        let content = arg_str(&args, "content")?;
        std::fs::write(&path, content).map_err(|e| failed(format!("{}: {e}", path.display())))?;
        Ok((
            json!({"path": path, "bytes": content.len()}),
            vec![Evidence::File {
                path: path.to_string_lossy().into_owned(),
                sha256: sha256_hex(content.as_bytes()),
            }],
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grant(capability: &str, scope: GrantScope) -> CapabilityGrant {
        CapabilityGrant {
            capability: capability.into(),
            scope,
            needs_approval: false,
        }
    }

    #[tokio::test]
    async fn files_are_read_and_written_only_inside_granted_folders() {
        let granted = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        std::fs::write(other.path().join("secret.txt"), "no").unwrap();
        let scope = GrantScope::Paths(vec![granted.path().to_string_lossy().into_owned()]);
        let target = granted.path().join("note.txt");

        let (_, evidence) = FileWrite
            .run(
                json!({"path": target, "content": "hi"}),
                &grant(FILE_WRITE, scope.clone()),
                None,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(
            matches!(&evidence[0], Evidence::File { sha256, .. } if *sha256 == sha256_hex(b"hi"))
        );
        let (value, _) = FileRead
            .run(
                json!({"path": target}),
                &grant(FILE_READ, scope.clone()),
                None,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(value["content"], "hi");

        for path in [
            other.path().join("secret.txt"),
            granted
                .path()
                .join("..")
                .join(other.path().file_name().unwrap())
                .join("secret.txt"),
        ] {
            let err = FileRead
                .run(
                    json!({"path": path}),
                    &grant(FILE_READ, scope.clone()),
                    None,
                    CancellationToken::new(),
                )
                .await
                .unwrap_err();
            assert!(matches!(err, InvokeError::OutOfScope(_)), "{err:?}");
        }
        let err = FileWrite
            .run(
                json!({"path": "relative.txt", "content": "x"}),
                &grant(FILE_WRITE, scope),
                None,
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, InvokeError::OutOfScope(_)));
    }

    #[test]
    fn an_app_scope_grants_no_paths() {
        let dir = tempfile::tempdir().unwrap();
        assert!(scoped_path(
            &GrantScope::Apps(vec!["blender.exe".into()]),
            &dir.path().to_string_lossy()
        )
        .is_err());
    }

    #[test]
    fn the_catalog_describes_every_primitive_once() {
        let names: Vec<String> = descriptors().into_iter().map(|d| d.name).collect();
        // The same on every host: a Linux primary registers a Windows
        // node's UI tools from this list.
        assert_eq!(
            names,
            [
                SYSTEM_RUN,
                FILE_READ,
                FILE_WRITE,
                super::super::ui::UI_SNAPSHOT,
                super::super::ui::UI_ACT,
            ]
        );
    }
}
