//! `/presence/*` — the HTTP side of the presence contract
//! (`docs/en/presence.md`).
//!
//! Owner routes authenticate exactly like `/companion` (`x-bastion-token`:
//! owner token or app JWT). `GET /presence/snapshot` and
//! `POST /presence/action` also accept a paired device's token
//! (`x-bastion-device-token`), limited to what a device may do.
//!
//! Auth runs before the body is parsed (raw `Bytes`, then `serde_json`), so
//! an unauthenticated caller always gets 401 — never a 415/422 that reveals
//! the route's shape, same discipline as `/mesh/ingest`.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use bastion_memory::PrivacyTier;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::channel::webhook::resolve_owner_or_401;
use crate::channel::OwnerMap;

use super::context::{self, ContextKind, JOURNAL_TAG};
use super::devices::{self, DeviceRecord, DeviceView};
use super::hub::{validate_ids, ApprovalOutcome, HostEvent, Resolution};
use super::policy::{self, cut, RuleDecision};
use super::PresenceService;

const OWNER_MAX_SESSIONS: usize = 50;
const OWNER_MAX_BYTES: usize = 64 * 1024;
const JOURNAL_PART_CHARS: usize = 300;

#[derive(Clone)]
struct PresenceState {
    service: Arc<PresenceService>,
    owner_map: Arc<OwnerMap>,
    jwt_secret: String,
}

enum Caller {
    Owner(String),
    Device(DeviceRecord),
}

impl Caller {
    /// The owner whose presence this caller sees and acts on — a device
    /// only ever its pairing owner's.
    fn owner(&self) -> &str {
        match self {
            Caller::Owner(owner) => owner,
            Caller::Device(device) => &device.owner,
        }
    }
}

fn error(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({ "error": message.into() }))).into_response()
}

fn owner(
    state: &PresenceState,
    headers: &HeaderMap,
    event: &'static str,
) -> Result<String, Box<Response>> {
    resolve_owner_or_401(headers, &state.owner_map, &state.jwt_secret, event)
}

/// A device token, when one is presented, must be valid — it never falls
/// through to owner auth.
fn caller(
    state: &PresenceState,
    headers: &HeaderMap,
    event: &'static str,
) -> Result<Caller, Box<Response>> {
    if let Some(token) = headers
        .get("x-bastion-device-token")
        .and_then(|v| v.to_str().ok())
    {
        return state
            .service
            .devices
            .by_token(token)
            .map(Caller::Device)
            .ok_or_else(|| {
                tracing::warn!(event, reason = "unknown device token");
                Box::new(error(StatusCode::UNAUTHORIZED, "unauthorized"))
            });
    }
    owner(state, headers, event).map(Caller::Owner)
}

fn parse<T: DeserializeOwned>(body: &Bytes) -> Result<T, Box<Response>> {
    serde_json::from_slice(body)
        .map_err(|e| Box::new(error(StatusCode::BAD_REQUEST, format!("invalid body: {e}"))))
}

macro_rules! tri {
    ($e:expr) => {
        match $e {
            Ok(v) => v,
            Err(resp) => return *resp,
        }
    };
}

#[derive(Deserialize)]
struct SnapshotQuery {
    max_sessions: Option<usize>,
    max_bytes: Option<usize>,
}

fn limits(caller: &Caller, query: &SnapshotQuery) -> (usize, usize) {
    let (sessions, bytes) = match caller {
        Caller::Owner(_) => (OWNER_MAX_SESSIONS, OWNER_MAX_BYTES),
        Caller::Device(d) => (d.caps.max_sessions, d.caps.max_bytes),
    };
    (
        query.max_sessions.map_or(sessions, |q| q.min(sessions)),
        query.max_bytes.map_or(bytes, |q| q.min(bytes)),
    )
}

async fn snapshot_handler(
    State(state): State<PresenceState>,
    headers: HeaderMap,
    Query(query): Query<SnapshotQuery>,
) -> Response {
    let caller = tri!(caller(&state, &headers, "presence_snapshot_unauthorized"));
    let (sessions, bytes) = limits(&caller, &query);
    Json(state.service.snapshot(caller.owner(), sessions, bytes)).into_response()
}

async fn event_handler(
    State(state): State<PresenceState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let owner = tri!(owner(&state, &headers, "presence_event_unauthorized"));
    let mut event: HostEvent = tri!(parse(&body));
    if let Err(e) = event.validate() {
        return error(StatusCode::BAD_REQUEST, e.to_string());
    }
    event.tool_input = policy::sanitize_tool_input(event.tool_input.as_ref());
    let seq = state.service.record(&owner, &event);
    Json(json!({ "ok": true, "seq": seq })).into_response()
}

#[derive(Deserialize)]
struct ContextRequest {
    source: String,
    session_id: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    prompt: Option<String>,
}

async fn context_handler(
    State(state): State<PresenceState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let owner = tri!(owner(&state, &headers, "presence_context_unauthorized"));
    let req: ContextRequest = tri!(parse(&body));
    if let Err(e) = validate_ids(&req.source, &req.session_id) {
        return error(StatusCode::BAD_REQUEST, e.to_string());
    }
    let kind = match req.kind.as_deref() {
        None | Some("session") => ContextKind::Session,
        Some("prompt") => ContextKind::Prompt,
        Some(other) => return error(StatusCode::BAD_REQUEST, format!("unknown kind '{other}'")),
    };
    let hub = &state.service.hub;
    let chosen = hub.persona(&owner);
    // A prompt in a session that was given another persona (switched since)
    // gets the whole frame again, so the switch takes effect right away.
    let kind = match (
        kind,
        hub.injected_persona(&owner, &req.source, &req.session_id),
    ) {
        (ContextKind::Prompt, Some(given))
            if given
                != context::resolve_persona(&state.service.context.personas, chosen.as_deref()) =>
        {
            ContextKind::Session
        }
        (kind, _) => kind,
    };
    let (text, persona) = context::build(
        &state.service.context,
        &owner,
        chosen.as_deref(),
        kind,
        req.prompt.as_deref(),
        state.service.config.context_max_chars,
    )
    .await;
    if kind == ContextKind::Session {
        hub.mark_injected(&owner, &req.source, &req.session_id, persona.as_deref());
    }
    Json(json!({ "context": text, "persona": persona })).into_response()
}

#[derive(Deserialize)]
struct ApprovalRequest {
    source: String,
    session_id: String,
    #[serde(default)]
    cwd: Option<String>,
    tool_name: String,
    #[serde(default)]
    tool_input: Option<Value>,
}

async fn approval_handler(
    State(state): State<PresenceState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let owner = tri!(owner(&state, &headers, "presence_approval_unauthorized"));
    let req: ApprovalRequest = tri!(parse(&body));
    if let Err(e) = validate_ids(&req.source, &req.session_id) {
        return error(StatusCode::BAD_REQUEST, e.to_string());
    }
    // Rules see the whole input; the cut copy is only for showing the call.
    let rule_input = policy::rule_input(req.tool_input.as_ref());
    let input = policy::sanitize_tool_input(req.tool_input.as_ref());
    let Some((decision, reason)) =
        policy::decide(&state.service.config.rules, &req.tool_name, &rule_input)
    else {
        return Json(json!({ "decision": "none" })).into_response();
    };
    let (decision, reason) = match decision {
        RuleDecision::Allow => ("allow", reason),
        RuleDecision::Deny => ("deny", reason),
        RuleDecision::Ask => ("ask", reason),
        RuleDecision::Remote => {
            let wait = Duration::from_secs(state.service.config.approval_wait_secs.clamp(1, 600));
            let (id, outcome) = state
                .service
                .hub
                .request_approval(
                    &owner,
                    &req.source,
                    &req.session_id,
                    req.cwd.as_deref(),
                    &req.tool_name,
                    input.as_ref(),
                    match &rule_input {
                        policy::RuleInput::Full(text) => Some(text.as_str()),
                        _ => None,
                    },
                    wait,
                )
                .await;
            match outcome {
                ApprovalOutcome::Approved => ("allow", format!("approved in Bastion ({id})")),
                ApprovalOutcome::Denied => ("deny", format!("denied in Bastion ({id})")),
                ApprovalOutcome::TimedOut => (
                    "ask",
                    format!(
                        "no answer in Bastion within {}s ({id}); asking here",
                        wait.as_secs()
                    ),
                ),
            }
        }
    };
    Json(json!({ "decision": decision, "reason": reason })).into_response()
}

#[derive(Deserialize)]
struct JournalRequest {
    source: String,
    session_id: String,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    reply: Option<String>,
}

fn journal_entry(req: &JournalRequest) -> Option<String> {
    let clean = |s: &Option<String>| {
        s.as_deref()
            .map(|t| t.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|t| !t.is_empty())
            .map(|t| cut(&t, JOURNAL_PART_CHARS))
    };
    let (prompt, reply) = (clean(&req.prompt), clean(&req.reply));
    if prompt.is_none() && reply.is_none() {
        return None;
    }
    let project = req
        .cwd
        .as_deref()
        .and_then(|c| c.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next())
        .filter(|p| !p.is_empty())
        .unwrap_or(&req.source);
    Some(format!(
        "[{project} · {}] asked: {} — answered: {}",
        req.source,
        prompt.as_deref().unwrap_or("—"),
        reply.as_deref().unwrap_or("—"),
    ))
}

async fn journal_handler(
    State(state): State<PresenceState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let owner = tri!(owner(&state, &headers, "presence_journal_unauthorized"));
    let req: JournalRequest = tri!(parse(&body));
    if let Err(e) = validate_ids(&req.source, &req.session_id) {
        return error(StatusCode::BAD_REQUEST, e.to_string());
    }
    if !state.service.config.journal {
        return Json(json!({ "stored": false })).into_response();
    }
    let Some(entry) = journal_entry(&req) else {
        return Json(json!({ "stored": false })).into_response();
    };
    let stored = state
        .service
        .context
        .memory
        .read()
        .await
        .store_belief(
            &owner,
            Some(JOURNAL_TAG),
            &entry,
            &req.session_id,
            "presence-journal",
            false,
            // Local-only by construction: the journal never travels back to
            // a cloud host through `/presence/context`.
            Some(PrivacyTier::LocalOnly),
        )
        .await;
    match stored {
        Ok(id) => Json(json!({ "stored": true, "id": id })).into_response(),
        Err(e) => {
            tracing::warn!(event = "presence_journal_failed", error = %e);
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not store the journal entry",
            )
        }
    }
}

#[derive(Deserialize)]
struct ActionRequest {
    action: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    persona: Option<String>,
    #[serde(default)]
    care: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    minutes: Option<u64>,
}

async fn action_handler(
    State(state): State<PresenceState>,
    headers: HeaderMap,
    Query(query): Query<SnapshotQuery>,
    body: Bytes,
) -> Response {
    let caller = tri!(caller(&state, &headers, "presence_action_unauthorized"));
    let req: ActionRequest = tri!(parse(&body));
    let service = &state.service;
    let owner_only = || {
        error(
            StatusCode::FORBIDDEN,
            format!("'{}' needs the owner token", req.action),
        )
    };

    match req.action.as_str() {
        "approve" | "deny" => {
            let Some(id) = req.id.as_deref() else {
                return error(
                    StatusCode::BAD_REQUEST,
                    "approve/deny needs the approval id",
                );
            };
            let by = match &caller {
                Caller::Owner(_) => "owner".to_string(),
                Caller::Device(d) => {
                    if !(service.config.device_approvals && d.caps.touch) {
                        return error(
                            StatusCode::FORBIDDEN,
                            "this device may not answer approvals ([presence] device_approvals)",
                        );
                    }
                    format!("device:{}", d.id)
                }
            };
            let resolution = if req.action == "approve" {
                Resolution::Approve
            } else {
                Resolution::Deny
            };
            if let Err(e) = service.hub.resolve(caller.owner(), id, resolution, &by) {
                return error(StatusCode::NOT_FOUND, e.to_string());
            }
        }
        "persona" => {
            if !matches!(caller, Caller::Owner(_)) {
                return owner_only();
            }
            let persona = req
                .persona
                .as_deref()
                .map(str::trim)
                .filter(|p| !p.is_empty());
            if let Some(name) = persona {
                if service.context.personas.get(name).is_none() {
                    return error(StatusCode::BAD_REQUEST, format!("unknown persona '{name}'"));
                }
            }
            service
                .hub
                .set_persona(caller.owner(), persona.map(str::to_string));
        }
        "care" => {
            let Some(care) = req.care.as_deref() else {
                return error(
                    StatusCode::BAD_REQUEST,
                    "care needs one of feed, water, play, sleep",
                );
            };
            if let Err(e) = service.companion.care(care, &service.events_tx) {
                return error(StatusCode::BAD_REQUEST, e.to_string());
            }
            service.hub.touch();
        }
        "note" => {
            if !matches!(caller, Caller::Owner(_)) {
                return owner_only();
            }
            service.hub.set_note(
                caller.owner(),
                req.text.as_deref().unwrap_or_default(),
                req.minutes.unwrap_or(30),
            );
        }
        other => return error(StatusCode::BAD_REQUEST, format!("unknown action '{other}'")),
    }
    let (sessions, bytes) = limits(&caller, &query);
    Json(service.snapshot(caller.owner(), sessions, bytes)).into_response()
}

async fn devices_list_handler(State(state): State<PresenceState>, headers: HeaderMap) -> Response {
    let owner = tri!(owner(&state, &headers, "presence_devices_unauthorized"));
    Json(json!({ "devices": state.service.devices.list(&owner) })).into_response()
}

#[derive(Deserialize)]
struct PairRequest {
    addr: String,
    code: String,
    #[serde(default)]
    name: Option<String>,
}

/// The URL a device on the LAN uses to reach this daemon: the configured
/// `advertise_url`, else this machine's address on the route to the device
/// plus the webhook port. A daemon listening on loopback only cannot be
/// reached by a device at all — said plainly instead of pairing a device
/// that could never answer.
fn bastion_url_for(configured: &str, device_addr: &str) -> Result<String, String> {
    if !configured.trim().is_empty() {
        return Ok(configured.trim().trim_end_matches('/').to_string());
    }
    let listen = std::env::var("BASTION_WEBHOOK_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into());
    let listen: std::net::SocketAddr = listen.parse().map_err(|_| {
        "set [presence] advertise_url: BASTION_WEBHOOK_ADDR is not an ip:port".to_string()
    })?;
    if listen.ip().is_loopback() {
        return Err(format!(
            "the daemon listens on {listen} only, which a device cannot reach: listen on \
             0.0.0.0:{} (BASTION_WEBHOOK_ADDR) or set [presence] advertise_url",
            listen.port()
        ));
    }
    let host = device_addr.split(':').next().unwrap_or(device_addr);
    let local = std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| s.connect((host, 80)).map(|_| s))
        .and_then(|s| s.local_addr())
        .map_err(|e| format!("cannot find this machine's address towards {host}: {e}"))?;
    Ok(format!("http://{}:{}", local.ip(), listen.port()))
}

async fn devices_pair_handler(
    State(state): State<PresenceState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let owner = tri!(owner(&state, &headers, "presence_pair_unauthorized"));
    let req: PairRequest = tri!(parse(&body));
    if let Err(e) = devices::validate_addr(&req.addr) {
        return error(StatusCode::BAD_REQUEST, e.to_string());
    }
    let bastion_url = match bastion_url_for(&state.service.config.advertise_url, &req.addr) {
        Ok(url) => url,
        Err(message) => return error(StatusCode::BAD_REQUEST, message),
    };
    let client = reqwest::Client::new();
    let host_name = state.service.hub.host().to_string();
    match devices::pair(
        &client,
        &req.addr,
        req.code.trim(),
        &host_name,
        &bastion_url,
        &owner,
        req.name.as_deref(),
    )
    .await
    {
        Ok(record) => {
            let view = DeviceView::from(&record);
            if let Err(e) = state.service.devices.upsert(record) {
                tracing::warn!(event = "presence_device_save_failed", error = %e);
                return error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "could not save the device",
                );
            }
            state.service.hub.touch();
            tracing::info!(event = "presence_device_paired", device = %view.id);
            Json(json!({ "device": view, "bastion_url": bastion_url })).into_response()
        }
        Err(e) => error(StatusCode::BAD_GATEWAY, e.to_string()),
    }
}

async fn devices_delete_handler(
    State(state): State<PresenceState>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Response {
    let owner = tri!(owner(&state, &headers, "presence_unpair_unauthorized"));
    match state.service.devices.remove(&owner, &id) {
        Ok(true) => Json(json!({ "removed": true })).into_response(),
        Ok(false) => error(StatusCode::NOT_FOUND, format!("no device '{id}'")),
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

pub fn router(service: Arc<PresenceService>, owner_map: OwnerMap, jwt_secret: String) -> Router {
    Router::new()
        .route("/presence/snapshot", get(snapshot_handler))
        .route("/presence/event", post(event_handler))
        .route("/presence/context", post(context_handler))
        .route("/presence/approval", post(approval_handler))
        .route("/presence/journal", post(journal_handler))
        .route("/presence/action", post(action_handler))
        .route("/presence/devices", get(devices_list_handler))
        .route("/presence/devices/pair", post(devices_pair_handler))
        .route("/presence/devices/{id}", delete(devices_delete_handler))
        .with_state(PresenceState {
            service,
            owner_map: Arc::new(owner_map),
            jwt_secret,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PresenceConfig;
    use crate::presence::context::ContextSources;
    use crate::presence::devices::DeviceRegistry;
    use crate::presence::hub::PresenceHub;
    use crate::presence::policy::PresenceRule;
    use crate::tui::CompanionHandle;
    use bastion_cognition::goal::{GoalEngine, ScoringConfig};
    use bastion_memory::sqlite::SqliteMemory;
    use bastion_memory::{Memory, SharedMemory};
    use bastion_personas::persona::PersonaRegistry;
    use bastion_types::Persona;
    use std::collections::HashMap;

    struct Fixture {
        _db: tempfile::NamedTempFile,
        app: Router,
        service: Arc<PresenceService>,
    }

    async fn fixture(rules: Vec<PresenceRule>, wait_secs: u64) -> Fixture {
        let db = tempfile::NamedTempFile::new().unwrap();
        let path = db.path().to_str().unwrap().to_string();
        bastion_runtime::session::SessionManager::new(&path)
            .init_schema()
            .await
            .expect("init_schema");
        let memory: SharedMemory = Arc::new(tokio::sync::RwLock::new(Box::new(SqliteMemory::new(
            &path,
        ))
            as Box<dyn Memory>));
        let mut personas = HashMap::new();
        for (name, tier) in [
            ("ada", bastion_memory::PrivacyTier::CloudOk),
            ("vault", bastion_memory::PrivacyTier::LocalOnly),
        ] {
            personas.insert(
                name.to_string(),
                Persona {
                    name: name.to_string(),
                    description: Some(format!("{name} persona")),
                    system_prompt: format!("You are {name}, the careful one."),
                    tier,
                    weight: if name == "ada" { 0.9 } else { 0.1 },
                    ..Default::default()
                },
            );
        }
        let (events_tx, _) = tokio::sync::broadcast::channel(64);
        // Built by hand, not `PresenceService::new`: nothing here may write
        // under the developer's real `~/.config/bastion` (`CompanionHandle::load`
        // only reads; no test below sends an event that feeds the companion).
        let service = Arc::new(PresenceService {
            hub: Arc::new(PresenceHub::new(
                events_tx.clone(),
                "test-host".into(),
                None,
            )),
            devices: Arc::new(DeviceRegistry::load(None)),
            companion: CompanionHandle::load(false),
            context: ContextSources {
                memory,
                personas: Arc::new(PersonaRegistry::new_from_map(personas)),
                goals: GoalEngine::new(&path, ScoringConfig::default()),
            },
            config: PresenceConfig {
                rules,
                approval_wait_secs: wait_secs,
                ..PresenceConfig::default()
            },
            events_tx,
        });
        let app = router(
            service.clone(),
            OwnerMap::from_pairs(&[("tok-alice", "alice")]),
            "jwt-secret-for-tests".into(),
        );
        Fixture {
            _db: db,
            app,
            service,
        }
    }

    async fn call(
        app: &Router,
        method: &str,
        uri: &str,
        token: Option<&str>,
        body: Value,
    ) -> (StatusCode, Value) {
        let mut builder = axum::http::Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json");
        if let Some(t) = token {
            builder = builder.header("x-bastion-token", t);
        }
        let req = builder
            .body(axum::body::Body::from(body.to_string()))
            .unwrap();
        let resp = tower::ServiceExt::oneshot(app.clone(), req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    #[tokio::test]
    async fn every_route_needs_the_owner_token() {
        let f = fixture(vec![], 1).await;
        for (method, uri) in [
            ("GET", "/presence/snapshot"),
            ("POST", "/presence/event"),
            ("POST", "/presence/context"),
            ("POST", "/presence/approval"),
            ("POST", "/presence/journal"),
            ("POST", "/presence/action"),
            ("GET", "/presence/devices"),
            ("POST", "/presence/devices/pair"),
        ] {
            let (status, _) = call(&f.app, method, uri, None, json!({})).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {uri}");
        }
    }

    #[tokio::test]
    async fn tool_events_show_in_the_snapshot_without_unlisted_fields() {
        let f = fixture(vec![], 1).await;
        let (status, v) = call(
            &f.app,
            "POST",
            "/presence/event",
            Some("tok-alice"),
            json!({
                "source": "claude-code", "session_id": "abc", "event": "PreToolUse",
                "cwd": "/w/demo", "tool_name": "Bash",
                "tool_input": { "command": "ls -la", "content": "never stored" }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["ok"], true);
        let (_, snap) = call(
            &f.app,
            "GET",
            "/presence/snapshot",
            Some("tok-alice"),
            json!({}),
        )
        .await;
        assert_eq!(snap["sessions"][0]["name"], "demo");
        assert_eq!(snap["sessions"][0]["det"], "ls -la");
        assert!(!snap.to_string().contains("never stored"));
        let (status, _) = call(
            &f.app,
            "POST",
            "/presence/event",
            Some("tok-alice"),
            json!({ "source": "claude-code", "session_id": "abc", "event": "Bogus" }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn session_context_carries_the_persona_and_respects_egress() {
        let f = fixture(vec![], 1).await;
        {
            let memory = f.service.context.memory.read().await;
            memory
                .store_belief(
                    "alice",
                    None,
                    "Deploys go through the staging branch",
                    "s",
                    "t",
                    false,
                    Some(bastion_memory::PrivacyTier::CloudOk),
                )
                .await
                .unwrap();
            memory
                .store_belief(
                    "alice",
                    None,
                    "The vault password hint is blue",
                    "s",
                    "t",
                    false,
                    Some(bastion_memory::PrivacyTier::LocalOnly),
                )
                .await
                .unwrap();
        }
        let (status, v) = call(
            &f.app,
            "POST",
            "/presence/context",
            Some("tok-alice"),
            json!({ "source": "claude-code", "session_id": "abc", "kind": "session" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let text = v["context"].as_str().unwrap();
        assert_eq!(v["persona"], "ada");
        assert!(text.contains("You are ada, the careful one."));
        assert!(text.contains("staging branch"));
        assert!(
            !text.contains("vault password"),
            "local_only beliefs never leave"
        );
        assert!(
            text.contains("- vault"),
            "roster lists the local persona by name"
        );
        assert!(
            !text.contains("vault persona"),
            "but not its local-only description"
        );

        let (_, v) = call(
            &f.app,
            "POST",
            "/presence/context",
            Some("tok-alice"),
            json!({ "source": "claude-code", "session_id": "abc", "kind": "prompt", "prompt": "open a PR to staging" }),
        )
        .await;
        assert!(v["context"].as_str().unwrap().contains("staging branch"));
    }

    #[tokio::test]
    async fn persona_action_switches_the_context_and_rejects_unknown_names() {
        let f = fixture(vec![], 1).await;
        let (status, _) = call(
            &f.app,
            "POST",
            "/presence/action",
            Some("tok-alice"),
            json!({ "action": "persona", "persona": "nobody" }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        // The in-memory hub (no state path) keeps the choice without a file.
        let (status, snap) = call(
            &f.app,
            "POST",
            "/presence/action",
            Some("tok-alice"),
            json!({ "action": "persona", "persona": "vault" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(snap["persona"], "vault");
        let (_, v) = call(
            &f.app,
            "POST",
            "/presence/context",
            Some("tok-alice"),
            json!({ "source": "claude-code", "session_id": "abc" }),
        )
        .await;
        let text = v["context"].as_str().unwrap();
        assert!(text.contains("This persona is local-only"));
        assert!(!text.contains("You are vault"));

        // Switching back mid-session: the next prompt carries the new frame.
        call(
            &f.app,
            "POST",
            "/presence/action",
            Some("tok-alice"),
            json!({ "action": "persona", "persona": "ada" }),
        )
        .await;
        let (_, v) = call(
            &f.app,
            "POST",
            "/presence/context",
            Some("tok-alice"),
            json!({ "source": "claude-code", "session_id": "abc", "kind": "prompt", "prompt": "hello there" }),
        )
        .await;
        assert!(v["context"].as_str().unwrap().contains("You are ada"));
        let (_, v) = call(
            &f.app,
            "POST",
            "/presence/context",
            Some("tok-alice"),
            json!({ "source": "claude-code", "session_id": "abc", "kind": "prompt", "prompt": "hello there" }),
        )
        .await;
        assert!(
            !v["context"].as_str().unwrap().contains("You are ada"),
            "sent once per switch"
        );
    }

    #[tokio::test]
    async fn rules_decide_and_remote_rules_wait_for_a_surface() {
        let rules = vec![
            PresenceRule {
                tool: "Bash".into(),
                input: Some("*rm -rf*".into()),
                decision: RuleDecision::Deny,
            },
            PresenceRule {
                tool: "Bash".into(),
                input: Some("git push*".into()),
                decision: RuleDecision::Remote,
            },
        ];
        let f = fixture(rules, 5).await;
        let ask = |command: &str| json!({ "source": "claude-code", "session_id": "abc", "tool_name": "Bash", "tool_input": { "command": command } });
        let (_, v) = call(
            &f.app,
            "POST",
            "/presence/approval",
            Some("tok-alice"),
            ask("ls"),
        )
        .await;
        assert_eq!(v["decision"], "none");
        let (_, v) = call(
            &f.app,
            "POST",
            "/presence/approval",
            Some("tok-alice"),
            ask("rm -rf build"),
        )
        .await;
        assert_eq!(v["decision"], "deny");
        // Padding past the display cut does not hide the call from a rule.
        let padded = format!("{} && rm -rf build", "true ".repeat(80));
        let (_, v) = call(
            &f.app,
            "POST",
            "/presence/approval",
            Some("tok-alice"),
            ask(&padded),
        )
        .await;
        assert_eq!(v["decision"], "deny");

        let app = f.app.clone();
        let pending = tokio::spawn(async move {
            call(
                &app,
                "POST",
                "/presence/approval",
                Some("tok-alice"),
                ask("git push origin main"),
            )
            .await
        });
        let id = loop {
            let (_, snap) = call(
                &f.app,
                "GET",
                "/presence/snapshot",
                Some("tok-alice"),
                json!({}),
            )
            .await;
            if let Some(id) = snap["alerts"][0]["id"].as_str() {
                break id.to_string();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        let (status, _) = call(
            &f.app,
            "POST",
            "/presence/action",
            Some("tok-alice"),
            json!({ "action": "approve", "id": id }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (_, v) = pending.await.unwrap();
        assert_eq!(v["decision"], "allow");
        let (status, _) = call(
            &f.app,
            "POST",
            "/presence/action",
            Some("tok-alice"),
            json!({ "action": "deny", "id": "ap-999" }),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn the_journal_is_stored_local_only_and_never_injected() {
        let f = fixture(vec![], 1).await;
        let (_, v) = call(
            &f.app,
            "POST",
            "/presence/journal",
            Some("tok-alice"),
            json!({ "source": "claude-code", "session_id": "abc", "cwd": "/w/demo", "prompt": "refactor the staging deploy", "reply": "Done." }),
        )
        .await;
        assert_eq!(v["stored"], true);
        let beliefs = f
            .service
            .context
            .memory
            .read()
            .await
            .retrieve_tagged("alice", Some(JOURNAL_TAG))
            .await
            .unwrap();
        let entry = beliefs
            .iter()
            .find(|b| b.persona_tag.as_deref() == Some(JOURNAL_TAG))
            .unwrap();
        assert_eq!(entry.tier, Some(PrivacyTier::LocalOnly));
        assert!(entry.content.starts_with("[demo · claude-code]"));
        let (_, v) = call(
            &f.app,
            "POST",
            "/presence/context",
            Some("tok-alice"),
            json!({ "source": "claude-code", "session_id": "abc", "kind": "prompt", "prompt": "staging deploy again" }),
        )
        .await;
        assert!(!v["context"].as_str().unwrap().contains("refactor"));
    }

    #[test]
    fn a_configured_advertise_url_wins_and_is_normalized() {
        assert_eq!(
            bastion_url_for("http://10.0.0.5:8080/", "10.0.0.9").unwrap(),
            "http://10.0.0.5:8080"
        );
    }
}
