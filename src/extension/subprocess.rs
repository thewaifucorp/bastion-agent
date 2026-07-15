//! The `Subprocess` mechanism (design doc §2): a separate OS process,
//! `env_clear()` (mirrors `bastion-agent-runtime`'s `acpx` adapter pattern —
//! `crates/bastion-agent-runtime/src/acpx.rs`), a VERSIONED NDJSON stdio
//! protocol.
//!
//! The child NEVER gets raw network/memory/registry access. Every
//! cross-boundary request it wants to make (fetch a host, read memory, bind a
//! socket) is a `HostRequest` line the host answers by consulting the SAME
//! `PermissionSet` decision logic `HostFacade` uses — "a extensão pede ao
//! host, o host aplica policy" (design doc §2), never direct access.

use crate::extension::facade::{ExtensionInstance, HostFacade};
use async_trait::async_trait;
use bastion_extension_protocol::{ExtensionError, ExtensionManifest};
use bastion_runtime::capability::{Capability, InvokeCtx};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

/// Wire framing version — independent of `ExtensionManifest.compat` (that's
/// the SemVer range for the whole protocol crate; this is the raw NDJSON
/// message-shape version a subprocess child must speak).
pub const WIRE_VERSION: u32 = 1;

/// Bound on how long a subprocess call may take end to end (spawn +
/// request/response exchange + final result). A hung/malicious child never
/// blocks the daemon indefinitely.
const CALL_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Serialize)]
struct InvokeMsg<'a> {
    v: u32,
    #[serde(rename = "type")]
    kind: &'static str,
    call_id: &'a str,
    args: &'a Value,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum HostRequestKind {
    EgressFetch {
        host: String,
        path: String,
    },
    MemoryRead {
        owner: String,
    },
    NetworkBind {
        port: u16,
    },
    /// Adversarial vector (a), attempted over the subprocess wire: even if a
    /// child asks nicely, `invoke()` never holds a `CapabilityRegistry`
    /// handle to grant this with (see `Capability::invoke`'s signature in
    /// `crates/bastion-runtime/src/capability/registry.rs`) — it is
    /// structurally impossible, not merely policy-denied.
    RegisterCapability {
        name: String,
        description: String,
    },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ChildMsg {
    HostRequest {
        call_id: String,
        request: HostRequestKind,
    },
    Result {
        #[allow(dead_code)]
        call_id: String,
        data: Value,
    },
    Error {
        #[allow(dead_code)]
        call_id: String,
        message: String,
    },
}

#[derive(Debug, Serialize)]
struct HostResponseMsg<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    call_id: &'a str,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn next_call_id(name: &str) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{name}-{n}")
}

/// A capability backed by a subprocess. `invoke()` spawns a fresh child per
/// call (no long-lived process to manage/kill across calls), exchanges the
/// versioned NDJSON protocol, and tears the child down when done.
pub struct SubprocessCapability {
    name: String,
    description: String,
    schema: Value,
    manifest: Arc<ExtensionManifest>,
    command: String,
    args: Vec<String>,
    /// C3-cloud-ready (`docs/revamp/C3-cloud-ready-design.md`, security
    /// point 1): resolves each `manifest.secrets` entry BY NAME into the
    /// child's env at spawn time — never the daemon's own ambient env
    /// (`env_clear()` below still runs first). `None` preserves the
    /// pre-Loop-3-D behavior exactly (no allowlist, child gets nothing).
    secret_resolver: Option<Arc<dyn bastion_types::SecretResolver>>,
}

impl SubprocessCapability {
    /// Answer one host-mediated request from the child, using the SAME
    /// `PermissionSet` decision logic `HostFacade` wraps at activate time —
    /// `invoke()` has no `HostFacade` of its own (a `Capability` never holds
    /// a `CapabilityRegistry` handle by design), so this consults
    /// `manifest.permissions` directly rather than constructing one.
    fn handle_host_request(
        &self,
        ctx: &InvokeCtx,
        kind: HostRequestKind,
    ) -> Result<Value, ExtensionError> {
        match kind {
            HostRequestKind::EgressFetch { host, path } => {
                if !self.manifest.permissions.allows_egress_host(&host) {
                    return Err(ExtensionError::EgressHostNotGranted {
                        extension: self.manifest.id.clone(),
                        host,
                    });
                }
                // Reference behavior only proves authorization; it performs
                // no real network I/O (out of scope for the reference
                // extension — a real adapter would dispatch through the
                // daemon's own HTTP client here, still gated by this same
                // check).
                Ok(serde_json::json!({"authorized_host": host, "path": path}))
            }
            HostRequestKind::MemoryRead { owner } => {
                if !self
                    .manifest
                    .permissions
                    .allows_memory_read(&ctx.owner, &owner)
                {
                    return Err(ExtensionError::MemoryCrossOwnerDenied {
                        extension: self.manifest.id.clone(),
                        requester: ctx.owner.clone(),
                        target: owner,
                    });
                }
                Ok(serde_json::json!({"owner": owner}))
            }
            HostRequestKind::NetworkBind { port } => {
                if !self.manifest.permissions.allows_network_bind() {
                    return Err(ExtensionError::NetworkBindNotGranted {
                        extension: self.manifest.id.clone(),
                    });
                }
                Ok(serde_json::json!({"bound_port": port}))
            }
            HostRequestKind::RegisterCapability { name, description } => {
                tracing::warn!(
                    event = "extension_subprocess_register_capability_denied",
                    extension = %self.manifest.id,
                    capability = %name,
                    description = %description,
                    "subprocess child attempted to register a capability mid-invoke — structurally impossible (invoke() holds no CapabilityRegistry handle) as well as policy-denied",
                );
                Err(ExtensionError::CapabilityNotDeclared {
                    extension: self.manifest.id.clone(),
                    capability: name,
                })
            }
        }
    }
}

#[async_trait]
impl Capability for SubprocessCapability {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn input_schema(&self) -> &Value {
        &self.schema
    }

    /// A subprocess leaves the daemon's own address space — never treated as
    /// local-only by construction, even though its host-mediated egress
    /// requests are separately gated per destination.
    fn is_local(&self) -> bool {
        false
    }

    async fn invoke(&self, args: Value, ctx: &InvokeCtx) -> anyhow::Result<Value> {
        tokio::time::timeout(CALL_TIMEOUT, self.invoke_inner(args, ctx))
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "subprocess extension '{}' timed out after {:?}",
                    self.manifest.id,
                    CALL_TIMEOUT
                )
            })?
    }
}

impl SubprocessCapability {
    /// Resolve every `manifest.secrets` entry BY NAME via the injected
    /// resolver and return them as `(env_var_name, value)` pairs — the ONLY
    /// env vars ever added back after `env_clear()`. Fails closed: a
    /// manifest that declares a secret the resolver cannot currently
    /// resolve aborts the WHOLE call rather than silently spawning the
    /// child without it (a subprocess extension that thinks it has a
    /// credential and doesn't would fail in a much more confusing way
    /// downstream, and a partially-populated secret set is never safer than
    /// none).
    fn resolve_declared_secrets(&self) -> anyhow::Result<Vec<(String, String)>> {
        if self.manifest.secrets.is_empty() {
            return Ok(Vec::new());
        }
        let resolver = self.secret_resolver.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "extension '{}' declares {} secret(s) but no SecretResolver is configured",
                self.manifest.id,
                self.manifest.secrets.len()
            )
        })?;
        let mut resolved = Vec::with_capacity(self.manifest.secrets.len());
        for secret_ref in &self.manifest.secrets {
            let value = resolver.resolve(&secret_ref.name).map_err(|_| {
                // Never interpolate the resolver's own error into this
                // message beyond the reference name it already carries —
                // BastionError::SecretNotFound is name-only by construction,
                // but this call site does not lean on that; it re-derives
                // the same name-only shape independently.
                anyhow::anyhow!(
                    "extension '{}' declares secret '{}' which could not be resolved",
                    self.manifest.id,
                    secret_ref.name
                )
            })?;
            resolved.push((secret_ref.name.clone(), value.expose_secret().to_string()));
        }
        Ok(resolved)
    }

    async fn invoke_inner(&self, args: Value, ctx: &InvokeCtx) -> anyhow::Result<Value> {
        // Design doc §2: subprocess never inherits daemon ambient env.
        // Resolved BEFORE `env_clear()` runs (nothing about resolution
        // itself touches the child's env) — only the declared, resolved
        // secrets are added back, by name, after the clear.
        let declared_secrets = self.resolve_declared_secrets()?;

        let mut cmd = Command::new(&self.command);
        cmd.args(&self.args)
            .env_clear()
            .envs(declared_secrets)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);

        let mut child = cmd.spawn().map_err(|e| {
            anyhow::anyhow!(
                "failed to spawn subprocess extension '{}': {e}",
                self.manifest.id
            )
        })?;

        let call_id = next_call_id(&self.name);
        let mut stdin = child
            .stdin
            .take()
            .expect("Stdio::piped() guarantees a handle");
        let stdout = child
            .stdout
            .take()
            .expect("Stdio::piped() guarantees a handle");
        let mut lines = BufReader::new(stdout).lines();

        let invoke_msg = InvokeMsg {
            v: WIRE_VERSION,
            kind: "invoke",
            call_id: &call_id,
            args: &args,
        };
        write_line(&mut stdin, &invoke_msg).await?;

        loop {
            let raw = lines.next_line().await?.ok_or_else(|| {
                anyhow::anyhow!(
                    "subprocess extension '{}' closed stdout without a result",
                    self.manifest.id
                )
            })?;
            let msg: ChildMsg = serde_json::from_str(&raw).map_err(|e| {
                anyhow::anyhow!(
                    "subprocess extension '{}' sent a malformed message: {e}",
                    self.manifest.id
                )
            })?;
            match msg {
                ChildMsg::Result { data, .. } => {
                    let _ = child.wait().await;
                    return Ok(data);
                }
                ChildMsg::Error { message, .. } => {
                    let _ = child.wait().await;
                    anyhow::bail!(
                        "subprocess extension '{}' reported error: {message}",
                        self.manifest.id
                    );
                }
                ChildMsg::HostRequest {
                    call_id: req_id,
                    request,
                } => {
                    let response = match self.handle_host_request(ctx, request) {
                        Ok(data) => HostResponseMsg {
                            kind: "host_response",
                            call_id: &req_id,
                            ok: true,
                            data: Some(data),
                            error: None,
                        },
                        Err(e) => HostResponseMsg {
                            kind: "host_response",
                            call_id: &req_id,
                            ok: false,
                            data: None,
                            error: Some(e.to_string()),
                        },
                    };
                    write_line(&mut stdin, &response).await?;
                }
            }
        }
    }
}

async fn write_line<T: Serialize>(
    stdin: &mut tokio::process::ChildStdin,
    msg: &T,
) -> anyhow::Result<()> {
    let mut line = serde_json::to_string(msg)?;
    line.push('\n');
    stdin.write_all(line.as_bytes()).await?;
    stdin.flush().await?;
    Ok(())
}

/// One subprocess-backed capability entry: (name, description, schema,
/// command, args).
pub type SubprocessEntry = (String, String, Value, String, Vec<String>);

/// A `Subprocess`-kind extension: a manifest plus the capabilities it wants
/// backed by a child process.
pub struct SubprocessExtension {
    manifest: ExtensionManifest,
    entries: Vec<SubprocessEntry>,
    /// C3-cloud-ready: threaded into every `SubprocessCapability` this
    /// extension activates. `None` (the default) preserves pre-Loop-3-D
    /// behavior byte-for-byte — a manifest with an empty `secrets` list
    /// never even looks at this field (`resolve_declared_secrets` short-
    /// circuits on an empty list before ever consulting it).
    secret_resolver: Option<Arc<dyn bastion_types::SecretResolver>>,
}

impl SubprocessExtension {
    pub fn new(manifest: ExtensionManifest, entries: Vec<SubprocessEntry>) -> Self {
        Self {
            manifest,
            entries,
            secret_resolver: None,
        }
    }

    /// Inject the [`bastion_types::SecretResolver`] this extension's
    /// declared `manifest.secrets` are resolved through at each `invoke()`.
    /// Builder-style, matching `AgentLoop::with_*` — additive, does not
    /// change `new()`'s signature or any existing call site.
    #[must_use]
    pub fn with_secret_resolver(
        mut self,
        resolver: Arc<dyn bastion_types::SecretResolver>,
    ) -> Self {
        self.secret_resolver = Some(resolver);
        self
    }
}

#[async_trait]
impl ExtensionInstance for SubprocessExtension {
    fn manifest(&self) -> &ExtensionManifest {
        &self.manifest
    }

    async fn activate(&self, facade: &mut HostFacade<'_>) -> Result<(), ExtensionError> {
        let manifest_arc = Arc::new(self.manifest.clone());
        for (name, description, schema, command, args) in &self.entries {
            facade.register_capability(Arc::new(SubprocessCapability {
                name: name.clone(),
                description: description.clone(),
                schema: schema.clone(),
                manifest: manifest_arc.clone(),
                command: command.clone(),
                args: args.clone(),
                secret_resolver: self.secret_resolver.clone(),
            }))?;
        }
        Ok(())
    }

    async fn deactivate(&self, facade: &mut HostFacade<'_>) -> Result<(), ExtensionError> {
        for (name, _, _, _, _) in &self.entries {
            facade.deregister_capability(name);
        }
        Ok(())
    }
}

// Tests for this mechanism live in `tests/extension_subprocess.rs` — they
// spawn the real `reference-extension-echo` child process, and
// `CARGO_BIN_EXE_reference-extension-echo` is only defined by cargo for
// INTEGRATION test targets (tests/*.rs), never for a lib's own `#[cfg(test)]`
// unit tests.
