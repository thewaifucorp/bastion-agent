//! UI Automation primitives for a Windows node: `ui.snapshot` and `ui.act`
//! (spec `multi-device-brain-and-nodes.md` §9 decision 3, BMD-13, BMD-14).
//!
//! This crate forbids `unsafe`, so nothing here touches COM or the Windows UI
//! Automation API directly. The two capabilities only **orchestrate**: they
//! talk to a separate, out-of-workspace helper binary (`desktop/uia`) over
//! JSON on stdin/stdout ([`SubprocessBackend`]). All the policy that BMD-13
//! and BMD-14 require lives here, in safe, platform-independent code that is
//! unit-tested with a fake backend:
//!
//! - **BMD-13** — the grant carries [`GrantScope::Apps`]; the owning process's
//!   executable (never the window title) is checked against it *before* any
//!   capture or input leaves this device. A window of an app outside the list
//!   is refused with [`InvokeError::OutOfScope`].
//! - **BMD-14** — `ui.act` requires an `intent` (shown to the owner) and a
//!   `snapshot_id` from a snapshot taken on *this* node no more than ten
//!   minutes ago. The owner-facing approval prompt (intent + the referenced
//!   capture) is composed on the primary, which already holds both (BMD-02,
//!   BMD-11); the node re-attaches the referenced capture to the act's result
//!   so the effect stays tied to what was approved.
//! - The target window must belong to the same process as the snapshot; if it
//!   closed or changed, the action fails with [`InvokeError::WindowGone`] and
//!   never touches another window.
//! - A cancel interrupts an action in flight ([`InvokeError::Cancelled`]).
//!
//! Registration of these capabilities happens only under `cfg(windows)`
//! (see [`super::catalog`]); the module itself compiles everywhere so its
//! policy can be tested on any platform.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use bastion_mesh::devices::{
    ApprovalRef, CapabilityDescriptor, CapabilityGrant, Evidence, InvokeError, NodeCapability,
};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

pub const UI_SNAPSHOT: &str = "ui.snapshot";
pub const UI_ACT: &str = "ui.act";

/// A snapshot referenced by `ui.act` must be at most this old.
const SNAPSHOT_TTL: Duration = Duration::from_secs(10 * 60);

/// The window a snapshot or action targets, as the helper resolves it. `exe`
/// is the full path of the process that owns the window; `exe_name` its file
/// name. Scope is decided on `exe` (BMD-13), never on `title`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WindowId {
    pub hwnd: i64,
    pub pid: u32,
    pub exe: String,
    pub exe_name: String,
    pub title: String,
}

impl WindowId {
    fn to_value(&self) -> Value {
        json!({
            "hwnd": self.hwnd,
            "pid": self.pid,
            "exe": self.exe,
            "exe_name": self.exe_name,
            "title": self.title,
        })
    }

    /// A short, human label for `Evidence`.
    fn label(&self) -> String {
        format!("{} — {}", self.exe_name, self.title)
    }
}

/// How to find the window a `ui.snapshot` should capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowMatch {
    /// The window with focus (the default).
    Foreground,
    /// The first top-level window whose title contains this text.
    TitleContains(String),
    /// The first top-level window owned by a process with this executable name.
    ExeName(String),
}

impl WindowMatch {
    fn from_args(args: &Value) -> Self {
        let Some(w) = args.get("window") else {
            return Self::Foreground;
        };
        if let Some(s) = w.get("title_contains").and_then(Value::as_str) {
            return Self::TitleContains(s.to_owned());
        }
        if let Some(s) = w.get("exe_name").and_then(Value::as_str) {
            return Self::ExeName(s.to_owned());
        }
        Self::Foreground
    }

    fn to_value(&self) -> Value {
        match self {
            Self::Foreground => json!({ "foreground": true }),
            Self::TitleContains(s) => json!({ "title_contains": s }),
            Self::ExeName(s) => json!({ "exe_name": s }),
        }
    }
}

/// What a capture returns: the window's structure and a PNG of it.
#[derive(Debug, Clone)]
pub struct Capture {
    pub window: WindowId,
    /// The UI Automation element tree, as JSON. Untrusted node content.
    pub tree: Value,
    pub png_base64: String,
}

/// Why a backend call did not produce a result.
#[derive(Debug, Clone)]
pub enum UiaError {
    /// The window closed or is now owned by another process.
    WindowGone,
    /// The action was cancelled mid-flight.
    Cancelled,
    /// The helper could not be started (missing, not executable, …).
    HelperUnavailable(String),
    /// The helper ran and reported a failure.
    Failed(String),
}

impl From<UiaError> for InvokeError {
    fn from(e: UiaError) -> Self {
        match e {
            UiaError::WindowGone => InvokeError::WindowGone,
            UiaError::Cancelled => InvokeError::Cancelled,
            UiaError::HelperUnavailable(m) => {
                InvokeError::Failed(format!("UI automation helper unavailable: {m}"))
            }
            UiaError::Failed(m) => InvokeError::Failed(m),
        }
    }
}

/// The Windows side of UI Automation, behind a trait so the capabilities can
/// be tested without a display. The real implementation is [`SubprocessBackend`].
#[async_trait]
pub trait UiaBackend: Send + Sync {
    /// Resolve the target window (owner process, title) without capturing or
    /// sending input, so scope can be checked before anything is captured.
    async fn resolve(
        &self,
        matcher: &WindowMatch,
        cancel: &CancellationToken,
    ) -> Result<WindowId, UiaError>;

    /// Capture the window's structure and a screenshot. Re-verifies the window
    /// still belongs to `window`'s process; [`UiaError::WindowGone`] otherwise.
    async fn capture(
        &self,
        window: &WindowId,
        cancel: &CancellationToken,
    ) -> Result<Capture, UiaError>;

    /// Perform `action` on `window`. Re-verifies the window still belongs to
    /// `window`'s process *before* sending input; [`UiaError::WindowGone`]
    /// otherwise, never acting on a different window.
    async fn act(
        &self,
        window: &WindowId,
        action: &Value,
        cancel: &CancellationToken,
    ) -> Result<Value, UiaError>;
}

/// A snapshot kept on this node, so `ui.act` can reference it briefly.
#[derive(Debug, Clone)]
struct StoredSnapshot {
    window: WindowId,
    png_base64: String,
    taken_at: Instant,
}

/// Snapshots taken on this node, keyed by id. In memory only: a snapshot never
/// outlives the node process, which is exactly what "taken on this node"
/// guarantees. Expired entries are pruned lazily.
pub struct SnapshotStore {
    inner: Mutex<HashMap<String, StoredSnapshot>>,
}

impl Default for SnapshotStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SnapshotStore {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    fn insert_at(&self, window: WindowId, png_base64: String, taken_at: Instant) -> String {
        let id = new_id();
        let mut map = self.inner.lock().expect("snapshot store poisoned");
        map.retain(|_, s| taken_at.duration_since(s.taken_at) <= SNAPSHOT_TTL);
        map.insert(
            id.clone(),
            StoredSnapshot {
                window,
                png_base64,
                taken_at,
            },
        );
        id
    }

    fn insert(&self, window: WindowId, png_base64: String) -> String {
        self.insert_at(window, png_base64, Instant::now())
    }

    /// The snapshot for `id`, if it exists and is younger than [`SNAPSHOT_TTL`].
    fn get_fresh(&self, id: &str) -> Option<StoredSnapshot> {
        let map = self.inner.lock().expect("snapshot store poisoned");
        let snapshot = map.get(id)?;
        (snapshot.taken_at.elapsed() <= SNAPSHOT_TTL).then(|| snapshot.clone())
    }
}

fn new_id() -> String {
    let mut bytes = [0u8; 16];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The descriptor for `ui.snapshot`, needed without an instance (the catalog
/// advertises it on both sides).
pub fn snapshot_descriptor() -> CapabilityDescriptor {
    CapabilityDescriptor {
        name: UI_SNAPSHOT.into(),
        description: "Capture the structure and a screenshot of a window of a granted app on \
                      this device. Returns a snapshot id that ui.act must reference."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "window": {
                    "type": "object",
                    "description": "Which window to capture; defaults to the foreground window.",
                    "properties": {
                        "foreground": {"type": "boolean"},
                        "title_contains": {"type": "string"},
                        "exe_name": {"type": "string"}
                    }
                }
            }
        }),
    }
}

/// The descriptor for `ui.act`.
pub fn act_descriptor() -> CapabilityDescriptor {
    CapabilityDescriptor {
        name: UI_ACT.into(),
        description: "Act on a window of a granted app (invoke a control, set a value). Requires \
                      a fresh snapshot_id from ui.snapshot and an intent shown to the owner."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "snapshot_id": {"type": "string", "description": "Id from a recent ui.snapshot"},
                "intent": {"type": "string", "description": "Plain-language intent shown to the owner"},
                "action": {
                    "type": "object",
                    "properties": {
                        "kind": {"type": "string", "enum": ["invoke", "set_value"]},
                        "target": {
                            "type": "object",
                            "properties": {
                                "automation_id": {"type": "string"},
                                "name": {"type": "string"},
                                "ref": {"type": "string"}
                            }
                        },
                        "value": {"type": "string"}
                    },
                    "required": ["kind", "target"]
                }
            },
            "required": ["snapshot_id", "intent", "action"]
        }),
    }
}

/// `ui.snapshot`: capture a granted app's window (BMD-13).
pub struct UiSnapshot {
    backend: std::sync::Arc<dyn UiaBackend>,
    store: std::sync::Arc<SnapshotStore>,
}

impl UiSnapshot {
    pub fn new(
        backend: std::sync::Arc<dyn UiaBackend>,
        store: std::sync::Arc<SnapshotStore>,
    ) -> Self {
        Self { backend, store }
    }
}

#[async_trait]
impl NodeCapability for UiSnapshot {
    fn descriptor(&self) -> CapabilityDescriptor {
        snapshot_descriptor()
    }

    async fn run(
        &self,
        args: Value,
        grant: &CapabilityGrant,
        _approval: Option<&ApprovalRef>,
        cancel: CancellationToken,
    ) -> Result<(Value, Vec<Evidence>), InvokeError> {
        let matcher = WindowMatch::from_args(&args);
        // Resolve first — no capture, no input — so we can check scope on the
        // owning executable before anything about the window leaves the device.
        let window = self.backend.resolve(&matcher, &cancel).await?;
        // BMD-13: refuse a window of an app outside the grant, on the exe.
        if !grant.scope.allows_app(&window.exe) {
            return Err(InvokeError::OutOfScope(format!(
                "{} is not a granted app",
                window.exe_name
            )));
        }
        let capture = self.backend.capture(&window, &cancel).await?;
        let snapshot_id = self
            .store
            .insert(capture.window.clone(), capture.png_base64.clone());
        let value = json!({
            "snapshot_id": snapshot_id,
            "window": capture.window.to_value(),
            "tree": capture.tree,
        });
        let evidence = vec![Evidence::Screenshot {
            png_base64: capture.png_base64,
            window: capture.window.label(),
        }];
        Ok((value, evidence))
    }
}

/// `ui.act`: act on a window referenced by a fresh snapshot (BMD-13, BMD-14).
pub struct UiAct {
    backend: std::sync::Arc<dyn UiaBackend>,
    store: std::sync::Arc<SnapshotStore>,
}

impl UiAct {
    pub fn new(
        backend: std::sync::Arc<dyn UiaBackend>,
        store: std::sync::Arc<SnapshotStore>,
    ) -> Self {
        Self { backend, store }
    }
}

#[async_trait]
impl NodeCapability for UiAct {
    fn descriptor(&self) -> CapabilityDescriptor {
        act_descriptor()
    }

    async fn run(
        &self,
        args: Value,
        grant: &CapabilityGrant,
        _approval: Option<&ApprovalRef>,
        cancel: CancellationToken,
    ) -> Result<(Value, Vec<Evidence>), InvokeError> {
        // The grant is `needs_approval` (BMD-14): the node has already refused
        // this call unless the primary attached an ApprovalRef, so by the time
        // `run` is reached the owner approved. The prompt shown to the owner
        // (intent + the referenced capture) is composed on the primary, which
        // holds both; here we only require the fields and tie the effect to the
        // approved capture.
        let intent = args
            .get("intent")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| InvokeError::Failed("ui.act needs a non-empty intent".into()))?
            .to_owned();
        let snapshot_id = args
            .get("snapshot_id")
            .and_then(Value::as_str)
            .ok_or_else(|| InvokeError::Failed("ui.act needs a snapshot_id".into()))?;
        let action = args
            .get("action")
            .filter(|a| a.is_object())
            .cloned()
            .ok_or_else(|| InvokeError::Failed("ui.act needs an action object".into()))?;

        // The snapshot must have been taken on this node, no more than ten
        // minutes ago (in-memory store => taken on this node).
        let snapshot = self.store.get_fresh(snapshot_id).ok_or_else(|| {
            InvokeError::Failed(
                "unknown snapshot_id, or older than ten minutes; take a fresh ui.snapshot".into(),
            )
        })?;

        // BMD-13: check the snapshot's owning executable against the grant
        // BEFORE any input is sent. The backend then verifies the live window
        // still belongs to that same process (WindowGone otherwise), so a scope
        // decision on the recorded exe holds for the window actually acted on.
        if !grant.scope.allows_app(&snapshot.window.exe) {
            return Err(InvokeError::OutOfScope(format!(
                "{} is not a granted app",
                snapshot.window.exe_name
            )));
        }

        let result = self.backend.act(&snapshot.window, &action, &cancel).await?;

        let value = json!({
            "snapshot_id": snapshot_id,
            "intent": intent,
            "window": snapshot.window.to_value(),
            "result": result,
        });
        // Re-attach the exact capture the owner approved against (BMD-14), plus
        // a text record of the intent for the audit trail.
        let evidence = vec![
            Evidence::Screenshot {
                png_base64: snapshot.png_base64,
                window: snapshot.window.label(),
            },
            Evidence::Text {
                text: format!("ui.act on {}: {intent}", snapshot.window.label()),
            },
        ];
        Ok((value, evidence))
    }
}

/// The real backend: it runs the out-of-workspace `desktop/uia` helper once per
/// call, sending one JSON command on stdin and reading one JSON reply on
/// stdout. The helper is where COM / UI Automation `unsafe` lives; this crate
/// stays `unsafe`-free.
pub struct SubprocessBackend {
    bin: PathBuf,
}

impl SubprocessBackend {
    pub fn new(bin: PathBuf) -> Self {
        Self { bin }
    }

    /// Locate the helper: `$BASTION_UIA_BIN`, else `uia[.exe]` next to the
    /// running executable. The path is used lazily; a missing helper surfaces
    /// as [`UiaError::HelperUnavailable`] when a capability runs.
    pub fn locate() -> Self {
        if let Some(path) = std::env::var_os("BASTION_UIA_BIN") {
            return Self::new(PathBuf::from(path));
        }
        let name = if cfg!(windows) { "uia.exe" } else { "uia" };
        let bin = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join(name)))
            .unwrap_or_else(|| PathBuf::from(name));
        Self::new(bin)
    }

    async fn run_cmd(&self, request: Value, cancel: &CancellationToken) -> Result<Value, UiaError> {
        use tokio::io::AsyncWriteExt;

        let mut child = tokio::process::Command::new(&self.bin)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| UiaError::HelperUnavailable(e.to_string()))?;

        let mut line = serde_json::to_vec(&request).map_err(|e| UiaError::Failed(e.to_string()))?;
        line.push(b'\n');
        {
            let mut stdin = child
                .stdin
                .take()
                .ok_or_else(|| UiaError::Failed("helper stdin unavailable".into()))?;
            if let Err(e) = stdin.write_all(&line).await {
                return Err(UiaError::HelperUnavailable(e.to_string()));
            }
            // Drop stdin so the helper sees EOF and processes the command.
        }

        // Cancellation cuts the action in flight: on cancel the child is
        // dropped and killed (kill_on_drop).
        let output = tokio::select! {
            output = child.wait_with_output() => {
                output.map_err(|e| UiaError::Failed(e.to_string()))?
            }
            () = cancel.cancelled() => return Err(UiaError::Cancelled),
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(UiaError::Failed(format!(
                "helper exited with {:?}: {}",
                output.status.code(),
                stderr.trim()
            )));
        }

        let reply: Value = serde_json::from_slice(&output.stdout)
            .map_err(|e| UiaError::Failed(format!("helper reply was not JSON: {e}")))?;
        parse_reply(reply)
    }
}

/// Turn the helper's `{"ok":true,...}` / `{"ok":false,"error":"..."}` envelope
/// into a payload or a typed error.
fn parse_reply(reply: Value) -> Result<Value, UiaError> {
    if reply.get("ok").and_then(Value::as_bool) == Some(true) {
        return Ok(reply);
    }
    let error = reply.get("error").and_then(Value::as_str).unwrap_or("");
    let detail = reply
        .get("detail")
        .and_then(Value::as_str)
        .unwrap_or(error)
        .to_owned();
    Err(match error {
        "window_gone" => UiaError::WindowGone,
        "cancelled" => UiaError::Cancelled,
        _ => UiaError::Failed(if detail.is_empty() {
            "helper reported a failure".into()
        } else {
            detail
        }),
    })
}

fn window_from_reply(reply: &Value) -> Result<WindowId, UiaError> {
    reply
        .get("window")
        .and_then(|w| serde_json::from_value::<WindowId>(w.clone()).ok())
        .ok_or_else(|| UiaError::Failed("helper reply had no window".into()))
}

#[async_trait]
impl UiaBackend for SubprocessBackend {
    async fn resolve(
        &self,
        matcher: &WindowMatch,
        cancel: &CancellationToken,
    ) -> Result<WindowId, UiaError> {
        let reply = self
            .run_cmd(
                json!({"cmd": "resolve", "match": matcher.to_value()}),
                cancel,
            )
            .await?;
        window_from_reply(&reply)
    }

    async fn capture(
        &self,
        window: &WindowId,
        cancel: &CancellationToken,
    ) -> Result<Capture, UiaError> {
        let reply = self
            .run_cmd(
                json!({"cmd": "capture", "hwnd": window.hwnd, "pid": window.pid, "exe": window.exe}),
                cancel,
            )
            .await?;
        Ok(Capture {
            window: window_from_reply(&reply).unwrap_or_else(|_| window.clone()),
            tree: reply.get("tree").cloned().unwrap_or(Value::Null),
            png_base64: reply
                .get("png_base64")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        })
    }

    async fn act(
        &self,
        window: &WindowId,
        action: &Value,
        cancel: &CancellationToken,
    ) -> Result<Value, UiaError> {
        let reply = self
            .run_cmd(
                json!({
                    "cmd": "act",
                    "hwnd": window.hwnd,
                    "pid": window.pid,
                    "exe": window.exe,
                    "action": action,
                }),
                cancel,
            )
            .await?;
        Ok(reply.get("result").cloned().unwrap_or(Value::Null))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bastion_mesh::devices::GrantScope;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// A backend that records what it was asked to do and returns scripted
    /// results, so the capabilities' policy can be tested without a display.
    #[derive(Default)]
    struct FakeBackend {
        resolved: Mutex<Option<WindowId>>,
        captures: AtomicUsize,
        acts: AtomicUsize,
        acted_on: Mutex<Option<WindowId>>,
        act_error: Mutex<Option<UiaError>>,
        honor_cancel: bool,
    }

    fn window(exe: &str) -> WindowId {
        let exe_name = exe.rsplit(['/', '\\']).next().unwrap_or(exe).to_owned();
        WindowId {
            hwnd: 0x1234,
            pid: 42,
            exe: exe.to_owned(),
            exe_name,
            title: "Untitled".into(),
        }
    }

    #[async_trait]
    impl UiaBackend for FakeBackend {
        async fn resolve(
            &self,
            _matcher: &WindowMatch,
            _cancel: &CancellationToken,
        ) -> Result<WindowId, UiaError> {
            self.resolved
                .lock()
                .unwrap()
                .clone()
                .ok_or(UiaError::WindowGone)
        }

        async fn capture(
            &self,
            window: &WindowId,
            _cancel: &CancellationToken,
        ) -> Result<Capture, UiaError> {
            self.captures.fetch_add(1, Ordering::SeqCst);
            Ok(Capture {
                window: window.clone(),
                tree: json!({"name": "root", "children": []}),
                png_base64: "UE5H".into(),
            })
        }

        async fn act(
            &self,
            window: &WindowId,
            _action: &Value,
            cancel: &CancellationToken,
        ) -> Result<Value, UiaError> {
            self.acts.fetch_add(1, Ordering::SeqCst);
            *self.acted_on.lock().unwrap() = Some(window.clone());
            if self.honor_cancel && cancel.is_cancelled() {
                return Err(UiaError::Cancelled);
            }
            if let Some(e) = self.act_error.lock().unwrap().take() {
                return Err(e);
            }
            Ok(json!({"done": true}))
        }
    }

    fn grant(scope: GrantScope) -> CapabilityGrant {
        CapabilityGrant {
            capability: "ui".into(),
            scope,
            needs_approval: true,
        }
    }

    fn apps(names: &[&str]) -> GrantScope {
        GrantScope::Apps(names.iter().map(|s| (*s).to_owned()).collect())
    }

    // ---- BMD-13: snapshot refuses a window of an app outside the grant ----

    #[tokio::test]
    async fn snapshot_refuses_an_ungranted_app_before_capturing() {
        let backend = Arc::new(FakeBackend {
            resolved: Mutex::new(Some(window("C:\\apps\\evil.exe"))),
            ..Default::default()
        });
        let store = Arc::new(SnapshotStore::new());
        let cap = UiSnapshot::new(backend.clone(), store);

        let err = cap
            .run(
                json!({}),
                &grant(apps(&["blender.exe"])),
                None,
                CancellationToken::new(),
            )
            .await
            .unwrap_err();

        assert!(matches!(err, InvokeError::OutOfScope(_)), "{err:?}");
        // Nothing was captured — the refusal happened before any capture.
        assert_eq!(backend.captures.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn snapshot_captures_a_granted_app_and_stores_it() {
        let backend = Arc::new(FakeBackend {
            resolved: Mutex::new(Some(window("C:\\apps\\blender.exe"))),
            ..Default::default()
        });
        let store = Arc::new(SnapshotStore::new());
        let cap = UiSnapshot::new(backend.clone(), store.clone());

        let (value, evidence) = cap
            .run(
                json!({"window": {"exe_name": "blender.exe"}}),
                &grant(apps(&["blender.exe"])),
                None,
                CancellationToken::new(),
            )
            .await
            .unwrap();

        assert_eq!(backend.captures.load(Ordering::SeqCst), 1);
        let id = value["snapshot_id"].as_str().unwrap();
        assert!(store.get_fresh(id).is_some());
        assert_eq!(value["window"]["exe_name"], "blender.exe");
        assert!(value["tree"].is_object());
        assert!(matches!(&evidence[0], Evidence::Screenshot { .. }));
    }

    // ---- BMD-13: act refuses a window of an app outside the grant ----

    #[tokio::test]
    async fn act_refuses_an_ungranted_app_before_sending_input() {
        let backend = Arc::new(FakeBackend::default());
        let store = Arc::new(SnapshotStore::new());
        // A snapshot of an app the grant does not cover.
        let id = store.insert(window("C:\\apps\\evil.exe"), "UE5H".into());
        let cap = UiAct::new(backend.clone(), store);

        let err = cap
            .run(
                json!({"snapshot_id": id, "intent": "click ok", "action": {"kind": "invoke", "target": {}}}),
                &grant(apps(&["blender.exe"])),
                None,
                CancellationToken::new(),
            )
            .await
            .unwrap_err();

        assert!(matches!(err, InvokeError::OutOfScope(_)), "{err:?}");
        // No input was ever sent.
        assert_eq!(backend.acts.load(Ordering::SeqCst), 0);
    }

    // ---- ui.act requires a snapshot_id and an intent ----

    #[tokio::test]
    async fn act_requires_a_non_empty_intent() {
        let backend = Arc::new(FakeBackend::default());
        let store = Arc::new(SnapshotStore::new());
        let id = store.insert(window("C:\\apps\\blender.exe"), "UE5H".into());
        let cap = UiAct::new(backend.clone(), store);

        for intent in ["", "   "] {
            let err = cap
                .run(
                    json!({"snapshot_id": id, "intent": intent, "action": {"kind": "invoke", "target": {}}}),
                    &grant(apps(&["blender.exe"])),
                    None,
                    CancellationToken::new(),
                )
                .await
                .unwrap_err();
            assert!(matches!(err, InvokeError::Failed(_)), "{err:?}");
        }
        assert_eq!(backend.acts.load(Ordering::SeqCst), 0);
    }

    // ---- ui.act rejects an unknown or stale snapshot ----

    #[tokio::test]
    async fn act_rejects_unknown_and_stale_snapshots() {
        let backend = Arc::new(FakeBackend::default());
        let store = Arc::new(SnapshotStore::new());
        let cap = UiAct::new(backend.clone(), store.clone());
        let g = grant(apps(&["blender.exe"]));

        // Unknown id.
        let err = cap
            .run(
                json!({"snapshot_id": "deadbeef", "intent": "go", "action": {"kind": "invoke", "target": {}}}),
                &g,
                None,
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, InvokeError::Failed(_)), "{err:?}");

        // Stale id: inserted eleven minutes ago.
        let stale = Instant::now() - Duration::from_secs(11 * 60);
        let id = store.insert_at(window("C:\\apps\\blender.exe"), "UE5H".into(), stale);
        let err = cap
            .run(
                json!({"snapshot_id": id, "intent": "go", "action": {"kind": "invoke", "target": {}}}),
                &g,
                None,
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, InvokeError::Failed(_)), "{err:?}");
        assert_eq!(backend.acts.load(Ordering::SeqCst), 0);
    }

    // ---- req 3: a changed/closed window fails with WindowGone ----

    #[tokio::test]
    async fn act_reports_window_gone_and_never_touches_another_window() {
        let backend = Arc::new(FakeBackend {
            act_error: Mutex::new(Some(UiaError::WindowGone)),
            ..Default::default()
        });
        let store = Arc::new(SnapshotStore::new());
        let win = window("C:\\apps\\blender.exe");
        let id = store.insert(win.clone(), "UE5H".into());
        let cap = UiAct::new(backend.clone(), store);

        let err = cap
            .run(
                json!({"snapshot_id": id, "intent": "click", "action": {"kind": "invoke", "target": {}}}),
                &grant(apps(&["blender.exe"])),
                None,
                CancellationToken::new(),
            )
            .await
            .unwrap_err();

        assert!(matches!(err, InvokeError::WindowGone), "{err:?}");
        // The backend was asked to act only on the snapshot's own window.
        assert_eq!(*backend.acted_on.lock().unwrap(), Some(win));
    }

    // ---- req 5: cancellation interrupts the action ----

    #[tokio::test]
    async fn act_is_cancelled() {
        let backend = Arc::new(FakeBackend {
            honor_cancel: true,
            ..Default::default()
        });
        let store = Arc::new(SnapshotStore::new());
        let id = store.insert(window("C:\\apps\\blender.exe"), "UE5H".into());
        let cap = UiAct::new(backend, store);

        let cancel = CancellationToken::new();
        cancel.cancel();
        let err = cap
            .run(
                json!({"snapshot_id": id, "intent": "click", "action": {"kind": "invoke", "target": {}}}),
                &grant(apps(&["blender.exe"])),
                None,
                cancel,
            )
            .await
            .unwrap_err();

        assert!(matches!(err, InvokeError::Cancelled), "{err:?}");
    }

    // ---- BMD-14: the act carries the referenced capture and the intent ----

    #[tokio::test]
    async fn act_reattaches_the_referenced_capture_and_intent() {
        let backend = Arc::new(FakeBackend::default());
        let store = Arc::new(SnapshotStore::new());
        let id = store.insert(window("C:\\apps\\blender.exe"), "PNGDATA".into());
        let cap = UiAct::new(backend, store);

        let (value, evidence) = cap
            .run(
                json!({"snapshot_id": id, "intent": "render the scene", "action": {"kind": "invoke", "target": {"name": "Render"}}}),
                &grant(apps(&["blender.exe"])),
                None,
                CancellationToken::new(),
            )
            .await
            .unwrap();

        assert_eq!(value["intent"], "render the scene");
        assert!(matches!(
            &evidence[0],
            Evidence::Screenshot { png_base64, .. } if png_base64 == "PNGDATA"
        ));
    }

    #[test]
    fn window_match_defaults_to_foreground() {
        assert_eq!(WindowMatch::from_args(&json!({})), WindowMatch::Foreground);
        assert_eq!(
            WindowMatch::from_args(&json!({"window": {"title_contains": "Blender"}})),
            WindowMatch::TitleContains("Blender".into())
        );
        assert_eq!(
            WindowMatch::from_args(&json!({"window": {"exe_name": "blender.exe"}})),
            WindowMatch::ExeName("blender.exe".into())
        );
    }

    #[test]
    fn helper_window_gone_maps_to_invoke_error() {
        let err = parse_reply(json!({"ok": false, "error": "window_gone"})).unwrap_err();
        assert!(matches!(InvokeError::from(err), InvokeError::WindowGone));
    }
}
