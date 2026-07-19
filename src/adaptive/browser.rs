//! Governed, provider-neutral browser capability (US-204).
//!
//! Exposes a provider-neutral browser *interface* plus ONE functional backend
//! (`HttpFetchBackend` — read-only real web access via `reqwest`) wired into the
//! Core `CapabilityRegistry` as a single `BrowserCapability`. Every browser op
//! therefore crosses the Core approval/egress chokepoint
//! (`bastion_runtime::capability::registry`): the capability declares
//! `is_local() == true` (it is a locally-hosted tool) yet `needs_approval() ==
//! true`, so opening/fetching the web is a gated effect mediated by the Core
//! approval gate before any request leaves the host.
//!
//! Rich interaction (click/type/screenshot via CDP) is explicitly BACKLOG for
//! US-204: those ops exist in the [`BrowserBackend`] interface but the HTTP
//! backend returns a typed "unsupported by this backend" error.
//!
//! # Security invariants (US-204)
//! * Page content is ALWAYS untrusted: [`PageSnapshot::trusted`] is hard-wired
//!   `false` and the snapshot Value returned from [`Capability::invoke`] is
//!   shaped `{ "trusted": false, "text": ... }` so a caller cannot mistake
//!   fetched web content for a trusted instruction source (prompt-injection
//!   guard).
//! * Downloads stay inside the workspace: `dest_rel` is rejected if it is
//!   absolute or contains a `..` component, so a download can never escape
//!   `workspace_root`.
//! * No secret material (cookies / credentials / auth headers) is ever placed
//!   in a returned Value — only `url` / `text` / `status` / `path`.

use async_trait::async_trait;
use bastion_runtime::capability::{Capability, InvokeCtx};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::IpAddr;
use std::path::{Component, PathBuf};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;

/// Maximum snapshot text retained, in bytes. Web pages can be arbitrarily large;
/// we cap the extracted text to a sane size before handing it upward (US-204).
const MAX_SNAPSHOT_BYTES: usize = 16 * 1024;

/// Provider-neutral browser operation set (US-204).
///
/// Serialized with an internally-tagged `op` discriminator so a caller drives
/// the capability with `{ "op": "navigate", "url": "..." }` etc.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum BrowserOp {
    /// Open (or re-point) the session at `url`.
    Navigate { url: String },
    /// Read the current page as text.
    Snapshot,
    /// Download `url` to `dest_rel`, resolved inside the workspace root.
    Download { url: String, dest_rel: String },
    /// Rich interaction (click/type/…). BACKLOG — unsupported by the HTTP backend.
    Interact {
        /// Free-form interaction descriptor (selector, action, value, …).
        action: String,
    },
    /// Capture a screenshot. BACKLOG — unsupported by the HTTP backend.
    Screenshot,
    /// Close the session and clear its state.
    Close,
}

/// A read-only view of a page (US-204).
///
/// SECURITY: [`PageSnapshot::trusted`] is ALWAYS `false`. Page content is
/// attacker-controlled and must never be treated as a trusted instruction
/// source (prompt-injection guard). The constructor is the only way to build
/// one and it hard-wires `trusted = false`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageSnapshot {
    /// The URL the snapshot was taken from.
    pub url: String,
    /// Extracted page text, truncated to [`MAX_SNAPSHOT_BYTES`].
    pub text: String,
    /// Trust classification — ALWAYS `false` (see type docs).
    pub trusted: bool,
}

impl PageSnapshot {
    /// Build an (always-untrusted) snapshot, truncating `text` to the cap.
    pub fn new(url: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            text: truncate_utf8(text.into(), MAX_SNAPSHOT_BYTES),
            // SECURITY (US-204): never derived from input — a snapshot is
            // untrusted by construction.
            trusted: false,
        }
    }
}

/// Opaque per-owner browser session handle (US-204).
///
/// Holds no cookies/credentials — only the current URL and the owner that
/// created it. [`BrowserSession::close`] clears navigational state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserSession {
    /// Caller-supplied opaque session id.
    pub id: String,
    /// The URL last navigated to, if any.
    pub current_url: Option<String>,
    /// The owner (from [`InvokeCtx::owner`]) that created the session.
    pub owner: String,
}

impl BrowserSession {
    /// Create an empty session owned by `owner`.
    pub fn new(id: impl Into<String>, owner: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            current_url: None,
            owner: owner.into(),
        }
    }

    /// Clear navigational state (cancellation / `Close`).
    pub fn close(&mut self) {
        self.current_url = None;
    }
}

/// Provider-neutral browser backend (US-204).
///
/// `navigate` / `snapshot` / `download` are the functional surface a read-only
/// backend must implement. `interact` / `screenshot` carry default impls that
/// bail with a typed "unsupported by this backend" error — the HTTP backend
/// keeps those defaults (rich CDP interaction is BACKLOG).
#[async_trait]
pub trait BrowserBackend: Send + Sync {
    /// Point the session at `url`.
    async fn navigate(&self, session: &mut BrowserSession, url: &str) -> anyhow::Result<()>;

    /// Read the session's current page.
    async fn snapshot(&self, session: &BrowserSession) -> anyhow::Result<PageSnapshot>;

    /// Download `url` to `dest` (already resolved & validated inside workspace).
    async fn download(
        &self,
        session: &BrowserSession,
        url: &str,
        dest: &std::path::Path,
    ) -> anyhow::Result<PathBuf>;

    /// Rich interaction — BACKLOG. Default: unsupported.
    async fn interact(&self, _session: &mut BrowserSession, _action: &str) -> anyhow::Result<()> {
        anyhow::bail!("interact not supported by this backend")
    }

    /// Screenshot — BACKLOG. Default: unsupported.
    async fn screenshot(&self, _session: &BrowserSession) -> anyhow::Result<PathBuf> {
        anyhow::bail!("screenshot not supported by this backend")
    }
}

/// Read-only HTTP-fetch backend (US-204).
///
/// Uses `reqwest` for real, read-only web access: `navigate` records the URL,
/// `snapshot` GETs the current URL and returns the (truncated, untrusted) body,
/// `download` streams bytes to a workspace-relative path. `interact` /
/// `screenshot` keep the trait defaults (unsupported).
pub struct HttpFetchBackend {
    client: reqwest::Client,
}

impl HttpFetchBackend {
    /// Build a backend around a fresh `reqwest::Client`.
    ///
    /// SECURITY (SSRF): redirects are NOT followed
    /// (`redirect::Policy::none()`) so a 3xx can never bounce a validated
    /// public URL to an internal target behind the client's back — a redirect
    /// is surfaced as its (empty) 3xx body instead. Combined with
    /// [`validate_public_url`], which runs before every request.
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("reqwest client with a static redirect policy always builds");
        Self { client }
    }

    /// Build a backend around a caller-provided client (connection reuse).
    pub fn with_client(client: reqwest::Client) -> Self {
        Self { client }
    }
}

impl Default for HttpFetchBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl BrowserBackend for HttpFetchBackend {
    async fn navigate(&self, session: &mut BrowserSession, url: &str) -> anyhow::Result<()> {
        // Record the target. We do not pre-fetch here; snapshot performs the GET.
        session.current_url = Some(url.to_string());
        Ok(())
    }

    async fn snapshot(&self, session: &BrowserSession) -> anyhow::Result<PageSnapshot> {
        let url = session
            .current_url
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("no current url: navigate before snapshot"))?;
        validate_public_url(url).await?;
        let resp = self.client.get(url).send().await?;
        let final_url = resp.url().to_string();
        let text = resp.text().await?;
        Ok(PageSnapshot::new(final_url, text))
    }

    async fn download(
        &self,
        _session: &BrowserSession,
        url: &str,
        dest: &std::path::Path,
    ) -> anyhow::Result<PathBuf> {
        validate_public_url(url).await?;
        let resp = self.client.get(url).send().await?;
        let bytes = resp.bytes().await?;
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        // SECURITY: `create_new` fails if `dest` already exists — so a
        // pre-planted symlink at the final component causes the write to error
        // (EEXIST) instead of being followed out of the workspace.
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dest)
            .await?;
        file.write_all(&bytes).await?;
        file.flush().await?;
        Ok(dest.to_path_buf())
    }
}

/// SSRF guard: reject anything that isn't a public http(s) URL (US-204).
///
/// Parses the URL, requires an `http`/`https` scheme, then resolves the host
/// and rejects the request if ANY resolved address is loopback, private,
/// link-local (incl. the `169.254.169.254` cloud-metadata address),
/// unspecified, or otherwise internal/reserved. Runs before every fetch, and
/// the client does not follow redirects, so a public host cannot rebound to an
/// internal one. (DNS-rebinding between this check and the socket connect is
/// the residual gap; the approval gate — `needs_approval=true` — is the
/// second line of defence.)
async fn validate_public_url(url: &str) -> anyhow::Result<()> {
    let parsed = reqwest::Url::parse(url).map_err(|e| anyhow::anyhow!("invalid url: {e}"))?;
    match parsed.scheme() {
        "http" | "https" => {}
        other => anyhow::bail!("scheme '{other}' not allowed; only http/https"),
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("url has no host"))?;
    if host.eq_ignore_ascii_case("localhost") || host.to_ascii_lowercase().ends_with(".localhost") {
        anyhow::bail!("refusing to fetch localhost");
    }
    let port = parsed.port_or_known_default().unwrap_or(80);
    let mut resolved_any = false;
    for addr in tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| anyhow::anyhow!("could not resolve host '{host}': {e}"))?
    {
        resolved_any = true;
        if is_blocked_ip(&addr.ip()) {
            anyhow::bail!("refusing to fetch internal/reserved address for host '{host}'");
        }
    }
    if !resolved_any {
        anyhow::bail!("host '{host}' did not resolve to any address");
    }
    Ok(())
}

/// True if `ip` is loopback, private, link-local, unspecified or otherwise
/// internal/reserved and must not be fetched (US-204 SSRF guard).
fn is_blocked_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.octets()[0] == 0
        }
        IpAddr::V6(v6) => {
            // v4-mapped (::ffff:a.b.c.d) is checked as its embedded v4.
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_blocked_ip(&IpAddr::V4(v4));
            }
            let seg0 = v6.segments()[0];
            v6.is_loopback()
                || v6.is_unspecified()
                || (seg0 & 0xfe00) == 0xfc00 // unique-local fc00::/7
                || (seg0 & 0xffc0) == 0xfe80 // link-local fe80::/10
        }
    }
}

/// Truncate `s` to at most `max` bytes without splitting a UTF-8 char.
fn truncate_utf8(mut s: String, max: usize) -> String {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s.truncate(end);
    s
}

/// Resolve `dest_rel` against `workspace_root`, rejecting any escape (US-204).
///
/// SECURITY: rejects absolute paths and any `..` component so a download can
/// never be written outside `workspace_root`. Returns the joined absolute path.
fn resolve_in_workspace(
    workspace_root: &std::path::Path,
    dest_rel: &str,
) -> anyhow::Result<PathBuf> {
    let rel = PathBuf::from(dest_rel);
    if rel.is_absolute() {
        anyhow::bail!("dest_rel must be relative, got absolute path: {dest_rel}");
    }
    for comp in rel.components() {
        match comp {
            Component::ParentDir => {
                anyhow::bail!("dest_rel must not contain '..': {dest_rel}")
            }
            Component::Prefix(_) | Component::RootDir => {
                anyhow::bail!("dest_rel must be workspace-relative: {dest_rel}")
            }
            Component::CurDir | Component::Normal(_) => {}
        }
    }
    if dest_rel.trim().is_empty() {
        anyhow::bail!("dest_rel must not be empty");
    }
    let joined = workspace_root.join(rel);
    // SECURITY (symlink): reject a component-clean path whose closest EXISTING
    // ancestor canonicalizes outside the workspace — i.e. a symlink in the
    // workspace pointing elsewhere. `..` is already refused above; this closes
    // the symlink-escape the lexical check can't see.
    let base =
        std::fs::canonicalize(workspace_root).unwrap_or_else(|_| workspace_root.to_path_buf());
    let mut probe = joined.clone();
    let existing = loop {
        if probe.exists() {
            break probe;
        }
        match probe.parent() {
            Some(p) => probe = p.to_path_buf(),
            None => break base.clone(),
        }
    };
    if let Ok(canon) = std::fs::canonicalize(&existing) {
        if !canon.starts_with(&base) {
            anyhow::bail!("dest_rel resolves outside the workspace (symlink escape)");
        }
    }
    Ok(joined)
}

/// The governed browser capability (US-204).
///
/// Registers through the Core `CapabilityRegistry`; every op crosses the Core
/// egress + approval chokepoint. `is_local()` is `true` (the tool runs on-host)
/// while `needs_approval()` is `true` (reaching the web is a gated effect).
pub struct BrowserCapability {
    backend: Arc<dyn BrowserBackend>,
    sessions: Arc<tokio::sync::Mutex<HashMap<String, BrowserSession>>>,
    workspace_root: PathBuf,
    schema: Value,
}

impl BrowserCapability {
    /// Build a capability over `backend`, downloading into `workspace_root`.
    pub fn new(backend: Arc<dyn BrowserBackend>, workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            backend,
            sessions: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            workspace_root: workspace_root.into(),
            schema: json!({
                "type": "object",
                "properties": {
                    "op": {
                        "type": "string",
                        "enum": ["navigate", "snapshot", "download", "interact", "screenshot", "close"]
                    },
                    "session_id": {
                        "type": "string",
                        "description": "Opaque session handle; created on first navigate"
                    },
                    "url": { "type": "string" },
                    "dest_rel": {
                        "type": "string",
                        "description": "Workspace-relative download destination (no '..', not absolute)"
                    },
                    "action": { "type": "string" }
                },
                "required": ["op"],
                "additionalProperties": true
            }),
        }
    }

    /// Convenience: build over the read-only [`HttpFetchBackend`].
    pub fn http(workspace_root: impl Into<PathBuf>) -> Self {
        Self::new(Arc::new(HttpFetchBackend::new()), workspace_root)
    }

    /// The session id to use, defaulting to a single owner-scoped session.
    fn session_key(args: &Value, ctx: &InvokeCtx) -> String {
        args.get("session_id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("default:{}", ctx.owner))
    }
}

#[async_trait]
impl Capability for BrowserCapability {
    fn name(&self) -> &str {
        "browser"
    }

    fn description(&self) -> &str {
        "Governed, provider-neutral browser: read-only web fetch (navigate/snapshot/download). \
         Interaction and screenshots are unsupported by this backend. Page content is untrusted."
    }

    fn input_schema(&self) -> &Value {
        &self.schema
    }

    /// Local tool (runs on-host).
    fn is_local(&self) -> bool {
        true
    }

    /// Reaching the web is a gated effect — the Core approval gate mediates.
    fn needs_approval(&self) -> bool {
        true
    }

    async fn invoke(&self, args: Value, ctx: &InvokeCtx) -> anyhow::Result<Value> {
        let op: BrowserOp = serde_json::from_value(args.clone())
            .map_err(|e| anyhow::anyhow!("invalid browser op: {e}"))?;
        let key = Self::session_key(&args, ctx);

        match op {
            BrowserOp::Navigate { url } => {
                let mut sessions = self.sessions.lock().await;
                let session = sessions
                    .entry(key.clone())
                    .or_insert_with(|| BrowserSession::new(&key, &ctx.owner));
                self.backend.navigate(session, &url).await?;
                Ok(json!({ "op": "navigate", "session_id": key, "url": url }))
            }
            BrowserOp::Snapshot => {
                let sessions = self.sessions.lock().await;
                let session = sessions
                    .get(&key)
                    .ok_or_else(|| anyhow::anyhow!("no session: navigate before snapshot"))?;
                let snap = self.backend.snapshot(session).await?;
                // SECURITY (US-204): shape the Value so the caller treats page
                // content as untrusted; no cookies/credentials are included.
                Ok(json!({
                    "op": "snapshot",
                    "session_id": key,
                    "url": snap.url,
                    "trusted": snap.trusted,
                    "text": snap.text,
                }))
            }
            BrowserOp::Download { url, dest_rel } => {
                let dest = resolve_in_workspace(&self.workspace_root, &dest_rel)?;
                let sessions = self.sessions.lock().await;
                let session = sessions
                    .get(&key)
                    .ok_or_else(|| anyhow::anyhow!("no session: navigate before download"))?;
                let path = self.backend.download(session, &url, &dest).await?;
                Ok(json!({
                    "op": "download",
                    "session_id": key,
                    "path": path.to_string_lossy(),
                }))
            }
            BrowserOp::Interact { action } => {
                let mut sessions = self.sessions.lock().await;
                let session = sessions
                    .get_mut(&key)
                    .ok_or_else(|| anyhow::anyhow!("no session: navigate before interact"))?;
                self.backend.interact(session, &action).await?;
                Ok(json!({ "op": "interact", "session_id": key }))
            }
            BrowserOp::Screenshot => {
                let sessions = self.sessions.lock().await;
                let session = sessions
                    .get(&key)
                    .ok_or_else(|| anyhow::anyhow!("no session: navigate before screenshot"))?;
                let path = self.backend.screenshot(session).await?;
                Ok(json!({
                    "op": "screenshot",
                    "session_id": key,
                    "path": path.to_string_lossy(),
                }))
            }
            BrowserOp::Close => {
                // Cancellation: drop the session from the map, clearing state.
                let mut sessions = self.sessions.lock().await;
                let removed = sessions.remove(&key).is_some();
                Ok(json!({ "op": "close", "session_id": key, "closed": removed }))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fake backend recording calls; no network. Its `snapshot` returns a
    /// canned body so we can assert the untrusted shaping.
    struct FakeBackend;

    #[async_trait]
    impl BrowserBackend for FakeBackend {
        async fn navigate(&self, session: &mut BrowserSession, url: &str) -> anyhow::Result<()> {
            session.current_url = Some(url.to_string());
            Ok(())
        }
        async fn snapshot(&self, session: &BrowserSession) -> anyhow::Result<PageSnapshot> {
            let url = session.current_url.clone().unwrap_or_default();
            Ok(PageSnapshot::new(url, "<html>hi</html>"))
        }
        async fn download(
            &self,
            _session: &BrowserSession,
            _url: &str,
            dest: &std::path::Path,
        ) -> anyhow::Result<PathBuf> {
            Ok(dest.to_path_buf())
        }
        // interact / screenshot keep the trait's unsupported defaults.
    }

    fn ctx() -> InvokeCtx {
        InvokeCtx {
            owner: "alice".to_string(),
            privacy_tier: None,
        }
    }

    fn cap() -> BrowserCapability {
        BrowserCapability::new(Arc::new(FakeBackend), PathBuf::from("/tmp/ws"))
    }

    #[test]
    fn browser_op_serde_round_trip() {
        let cases = vec![
            BrowserOp::Navigate {
                url: "https://example.com".to_string(),
            },
            BrowserOp::Snapshot,
            BrowserOp::Download {
                url: "https://example.com/f".to_string(),
                dest_rel: "sub/file.bin".to_string(),
            },
            BrowserOp::Interact {
                action: "click #ok".to_string(),
            },
            BrowserOp::Screenshot,
            BrowserOp::Close,
        ];
        for op in cases {
            let v = serde_json::to_value(&op).unwrap();
            let back: BrowserOp = serde_json::from_value(v).unwrap();
            assert_eq!(op, back);
        }
    }

    #[test]
    fn navigate_tag_is_present() {
        let v = serde_json::to_value(BrowserOp::Navigate {
            url: "u".to_string(),
        })
        .unwrap();
        assert_eq!(v["op"], json!("navigate"));
        assert_eq!(v["url"], json!("u"));
    }

    #[test]
    fn download_rejects_path_traversal() {
        let root = PathBuf::from("/tmp/ws");
        assert!(resolve_in_workspace(&root, "../etc/passwd").is_err());
        assert!(resolve_in_workspace(&root, "a/../../etc/passwd").is_err());
        assert!(resolve_in_workspace(&root, "/abs/path").is_err());
        assert!(resolve_in_workspace(&root, "").is_err());
        let ok = resolve_in_workspace(&root, "sub/file.bin").unwrap();
        assert_eq!(ok, PathBuf::from("/tmp/ws/sub/file.bin"));
    }

    #[test]
    fn snapshot_is_always_untrusted() {
        let snap = PageSnapshot::new("https://x", "body");
        assert!(!snap.trusted);
    }

    #[test]
    fn snapshot_text_is_truncated() {
        let big = "a".repeat(MAX_SNAPSHOT_BYTES + 1000);
        let snap = PageSnapshot::new("u", big);
        assert!(snap.text.len() <= MAX_SNAPSHOT_BYTES);
        assert!(!snap.trusted);
    }

    #[tokio::test]
    async fn interact_and_screenshot_default_unsupported() {
        let mut s = BrowserSession::new("s", "alice");
        let b = FakeBackend;
        let e = b.interact(&mut s, "click").await.unwrap_err();
        assert!(e.to_string().contains("not supported"));
        let e = b.screenshot(&s).await.unwrap_err();
        assert!(e.to_string().contains("not supported"));
    }

    #[tokio::test]
    async fn invoke_interact_returns_unsupported_error() {
        let c = cap();
        c.invoke(json!({ "op": "navigate", "url": "https://x" }), &ctx())
            .await
            .unwrap();
        let err = c
            .invoke(json!({ "op": "interact", "action": "click" }), &ctx())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not supported"));
    }

    #[tokio::test]
    async fn snapshot_invoke_marks_content_untrusted() {
        let c = cap();
        c.invoke(json!({ "op": "navigate", "url": "https://x" }), &ctx())
            .await
            .unwrap();
        let out = c.invoke(json!({ "op": "snapshot" }), &ctx()).await.unwrap();
        assert_eq!(out["trusted"], json!(false));
        assert!(out["text"].is_string());
        // No secret material fields leak into the Value.
        assert!(out.get("cookies").is_none());
    }

    #[tokio::test]
    async fn navigate_creates_session_close_removes_it() {
        let c = cap();
        c.invoke(
            json!({ "op": "navigate", "session_id": "s1", "url": "https://x" }),
            &ctx(),
        )
        .await
        .unwrap();
        assert_eq!(c.sessions.lock().await.len(), 1);

        let out = c
            .invoke(json!({ "op": "close", "session_id": "s1" }), &ctx())
            .await
            .unwrap();
        assert_eq!(out["closed"], json!(true));
        assert!(c.sessions.lock().await.is_empty());
    }

    #[test]
    fn capability_is_local_and_needs_approval() {
        let c = cap();
        assert!(c.is_local());
        assert!(c.needs_approval());
        assert_eq!(c.name(), "browser");
    }
}
