//! The presence hub: the daemon's one record of what host agents are doing
//! right now — sessions, alerts, pending approvals, the active persona and
//! a short note — and the snapshot every surface reads.
//!
//! Single writer: every mutation goes through one `Mutex` and ends in
//! `changed`, which bumps `seq`, broadcasts `presence.updated` on `/events`
//! and wakes the device pusher. Nothing here is persisted except the active
//! persona (`state.json`); sessions are rebuilt by the hosts' next events.
//!
//! What never enters the hub: prompts, replies, file contents, tool output.
//! A session row carries at most the main input of the current tool call,
//! cut to [`DET_LEN`] characters.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::{broadcast, oneshot, watch};

use super::policy::{cut, main_input};

pub const PROTOCOL_VERSION: u32 = 1;
pub const NAME_LEN: usize = 24;
pub const DET_LEN: usize = 48;
pub const MODEL_LEN: usize = 32;
const SESSION_TTL_SECS: u64 = 12 * 3600;
const DONE_TO_IDLE_SECS: u64 = 600;
const MAX_ALERTS: usize = 8;
const NOTE_MAX_CHARS: usize = 40;

/// The hook vocabulary `POST /presence/event` accepts.
pub const HOST_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PermissionRequest",
    "PostToolUse",
    "PostToolUseFailure",
    "Notification",
    "SubagentStart",
    "SubagentStop",
    "PreCompact",
    "Stop",
    "StopFailure",
    "SessionEnd",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionState {
    Working,
    Waiting,
    Idle,
    Done,
    Error,
}

impl SessionState {
    /// Snapshot order: what needs the person first.
    fn rank(self) -> u8 {
        match self {
            SessionState::Waiting => 0,
            SessionState::Error => 1,
            SessionState::Working => 2,
            SessionState::Done => 3,
            SessionState::Idle => 4,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AlertKind {
    Approval,
    Question,
    Error,
}

/// One normalized host event (`POST /presence/event`).
#[derive(Clone, Debug, Default, Deserialize)]
pub struct HostEvent {
    pub source: String,
    pub session_id: String,
    pub event: String,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub tool_input: Option<Value>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub notification_type: Option<String>,
    #[serde(default)]
    pub agent_id: Option<String>,
}

/// `source`: 1-32 of `[A-Za-z0-9._-]` (the companion's own rule, so the same
/// value feeds both). `session_id`: 1-128 characters.
pub fn validate_ids(source: &str, session_id: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !source.is_empty()
            && source.len() <= 32
            && source
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')),
        "source must be 1-32 ASCII letters, digits, '-', '_' or '.'"
    );
    anyhow::ensure!(
        !session_id.is_empty() && session_id.chars().count() <= 128,
        "session_id must be 1-128 characters"
    );
    Ok(())
}

impl HostEvent {
    pub fn validate(&self) -> anyhow::Result<()> {
        validate_ids(&self.source, &self.session_id)?;
        anyhow::ensure!(
            HOST_EVENTS.contains(&self.event.as_str()),
            "unknown event '{}'",
            self.event
        );
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolution {
    Approve,
    Deny,
}

/// How a `remote` approval ended for the host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalOutcome {
    Approved,
    Denied,
    TimedOut,
}

struct Session {
    source: String,
    short: String,
    name: String,
    cwd: Option<String>,
    state: SessionState,
    tool: Option<String>,
    det: Option<String>,
    model: Option<String>,
    since: u64,
    updated: u64,
}

struct Alert {
    id: String,
    kind: AlertKind,
    session_key: String,
    tool: Option<String>,
    det: Option<String>,
    exp: Option<u64>,
    /// An approval's whole main input (control characters dropped, at most
    /// [`APPROVAL_INPUT_MAX`]): what the person is actually approving,
    /// since `det` is cut for display. Owner snapshots only.
    input: Option<String>,
    /// `input` is the whole call. False when the call was too long to keep
    /// (or to check) — a surface must then not offer a blind approve.
    input_complete: bool,
}

pub const APPROVAL_INPUT_MAX: usize = 2000;

/// Characters that can make text read differently from what it is: every
/// default-ignorable code point and format character (zero-width, bidi
/// overrides and isolates, word joiners, BOM, variation selectors, fillers,
/// tags), line and paragraph separators, and anything else that takes no
/// column on screen (combining marks). Control characters are handled on
/// their own.
pub fn is_invisible(c: char) -> bool {
    use unicode_width::UnicodeWidthChar;
    if c.is_control() {
        return false;
    }
    matches!(c as u32,
        0x00AD | 0x034F | 0x0600..=0x0605 | 0x061C | 0x06DD | 0x070F
        | 0x0890..=0x0891 | 0x08E2 | 0x115F..=0x1160 | 0x17B4..=0x17B5
        | 0x180B..=0x180F | 0x200B..=0x200F | 0x2028..=0x202E
        | 0x2060..=0x206F | 0x3164 | 0xFE00..=0xFE0F | 0xFEFF | 0xFFA0
        | 0xFFF0..=0xFFFB | 0x110BD | 0x110CD | 0x13430..=0x1343F
        | 0x1BCA0..=0x1BCA3 | 0x1D173..=0x1D17A | 0xE0000..=0xE0FFF)
        || c.width() == Some(0)
}

/// Control characters (newlines and tabs included) become spaces, so an
/// approval can never hide part of the call behind a line break; invisible
/// formatting characters become a visible `�`.
pub fn clean_input(text: &str) -> String {
    let flat: String = text
        .chars()
        .map(|c| {
            if c.is_control() {
                ' '
            } else if is_invisible(c) {
                '\u{FFFD}'
            } else {
                c
            }
        })
        .collect();
    // Runs of spaces collapse too: padding can never push part of the call
    // out of a surface's view.
    let flat = flat.split_whitespace().collect::<Vec<_>>().join(" ");
    cut(&flat, APPROVAL_INPUT_MAX)
}

struct PendingApproval {
    alert: Alert,
    reply: oneshot::Sender<Resolution>,
}

#[derive(Default)]
struct Today {
    day: u64,
    turns: u64,
    tools: u64,
    errors: u64,
}

#[derive(Default, Serialize, Deserialize)]
struct Persisted {
    /// Active persona per owner.
    #[serde(default)]
    personas: HashMap<String, String>,
}

struct Inner {
    sessions: HashMap<String, Session>,
    alerts: Vec<Alert>,
    approvals: HashMap<String, PendingApproval>,
    seq: u64,
    personas: HashMap<String, String>,
    notes: HashMap<String, (String, u64)>,
    today: HashMap<String, Today>,
    /// The persona each session was last given, so a switch mid-session
    /// re-sends the persona frame on the next prompt.
    injected: HashMap<String, Option<String>>,
}

pub struct PresenceHub {
    inner: Mutex<Inner>,
    events_tx: broadcast::Sender<String>,
    changed: watch::Sender<u64>,
    host: String,
    state_path: Option<PathBuf>,
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The id a surface knows a session by: 8 hex digits of a SHA-256 of its
/// key, so ids from different owners and hosts never collide and the host's
/// own id never leaves the daemon.
pub fn short_id(key: &str) -> String {
    let digest = Sha256::digest(key.as_bytes());
    digest.iter().take(4).map(|b| format!("{b:02x}")).collect()
}

/// Separates the owner from the rest of a session key; never valid in a
/// `source` (see `validate_ids`).
const OWNER_SEP: char = '\u{1f}';

/// `<owner>␟<source>:<session_id>` — every session, alert and approval is
/// scoped to the owner who reported it, and only that owner (or that owner's
/// devices) sees or answers it.
fn session_key(owner: &str, source: &str, session_id: &str) -> String {
    format!("{owner}{OWNER_SEP}{source}:{session_id}")
}

fn owned_by(key: &str, owner: &str) -> bool {
    key.split_once(OWNER_SEP).is_some_and(|(o, _)| o == owner)
}

/// An approval id nobody can guess or enumerate.
fn new_approval_id(taken: &HashMap<String, PendingApproval>) -> String {
    loop {
        let id = format!("ap-{:08x}", rand::random::<u32>());
        if !taken.contains_key(&id) {
            return id;
        }
    }
}

/// The folder a session works in, as its display name.
fn display_name(cwd: Option<&str>, source: &str) -> String {
    let name = cwd
        .map(|c| c.trim_end_matches(['/', '\\']))
        .and_then(|c| c.rsplit(['/', '\\']).next())
        .filter(|n| !n.is_empty())
        .unwrap_or(source);
    cut(name, NAME_LEN)
}

/// The main input, single-line, relative to the session folder when it is
/// inside it, cut to [`DET_LEN`].
fn describe(input: Option<&Value>, cwd: Option<&str>) -> Option<String> {
    let text = main_input(input)?;
    let line = text.lines().next().unwrap_or_default().trim().to_string();
    let relative = match cwd {
        Some(dir) if !dir.is_empty() => line
            .strip_prefix(dir)
            .map(|rest| rest.trim_start_matches(['/', '\\']).to_string())
            .filter(|rest| !rest.is_empty())
            .unwrap_or(line),
        _ => line,
    };
    (!relative.is_empty()).then(|| cut(&relative, DET_LEN))
}

impl PresenceHub {
    /// `state_path`: where the active persona survives a restart (`None` in
    /// tests).
    pub fn new(
        events_tx: broadcast::Sender<String>,
        host: String,
        state_path: Option<PathBuf>,
    ) -> Self {
        let persisted: Persisted = state_path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
        let (changed, _) = watch::channel(0);
        Self {
            inner: Mutex::new(Inner {
                sessions: HashMap::new(),
                alerts: Vec::new(),
                approvals: HashMap::new(),
                seq: 0,
                personas: persisted.personas,
                notes: HashMap::new(),
                today: HashMap::new(),
                injected: HashMap::new(),
            }),
            events_tx,
            changed,
            host: cut(&host, NAME_LEN),
            state_path,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Bumps `seq`, tells `/events` and the device pusher. Called with the
    /// lock held so `seq` order matches mutation order.
    fn bump(&self, inner: &mut Inner) -> u64 {
        inner.seq += 1;
        let seq = inner.seq;
        let _ = self
            .events_tx
            .send(json!({ "event": "presence.updated", "seq": seq }).to_string());
        self.changed.send_replace(seq);
        seq
    }

    pub fn subscribe_changes(&self) -> watch::Receiver<u64> {
        self.changed.subscribe()
    }

    /// Records that `persona` was just given to a host session; `true` when
    /// it differs from what that session had (or it had nothing yet).
    pub fn mark_injected(
        &self,
        owner: &str,
        source: &str,
        session_id: &str,
        persona: Option<&str>,
    ) -> bool {
        let key = session_key(owner, source, session_id);
        let mut inner = self.lock();
        let persona = persona.map(str::to_string);
        let changed = inner.injected.get(&key) != Some(&persona);
        inner.injected.insert(key, persona);
        changed
    }

    /// The persona a host session was last given (`None`: never given one).
    pub fn injected_persona(
        &self,
        owner: &str,
        source: &str,
        session_id: &str,
    ) -> Option<Option<String>> {
        self.lock()
            .injected
            .get(&session_key(owner, source, session_id))
            .cloned()
    }

    /// This machine's name as surfaces show it.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Something outside the hub that surfaces show changed (the companion,
    /// the device list): bump `seq` so they refetch.
    pub fn touch(&self) {
        let mut inner = self.lock();
        self.bump(&mut inner);
    }

    pub fn persona(&self, owner: &str) -> Option<String> {
        self.lock().personas.get(owner).cloned()
    }

    pub fn set_persona(&self, owner: &str, persona: Option<String>) {
        let mut inner = self.lock();
        match &persona {
            Some(name) => inner.personas.insert(owner.to_string(), name.clone()),
            None => inner.personas.remove(owner),
        };
        if let Some(path) = &self.state_path {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let body = serde_json::to_string(&Persisted {
                personas: inner.personas.clone(),
            })
            .unwrap_or_default();
            if let Err(e) = std::fs::write(path, body) {
                tracing::warn!(event = "presence_state_save_failed", error = %e);
            }
        }
        let _ = self
            .events_tx
            .send(json!({ "event": "presence.persona" }).to_string());
        self.bump(&mut inner);
    }

    pub fn set_note(&self, owner: &str, text: &str, minutes: u64) {
        let mut inner = self.lock();
        let text = text.trim();
        if text.is_empty() {
            inner.notes.remove(owner);
        } else {
            let minutes = minutes.clamp(1, 24 * 60);
            inner.notes.insert(
                owner.to_string(),
                (cut(text, NOTE_MAX_CHARS), now_secs() + minutes * 60),
            );
        }
        self.bump(&mut inner);
    }

    /// Applies one host event. Returns the new `seq`.
    pub fn record(&self, owner: &str, event: &HostEvent) -> u64 {
        let now = now_secs();
        let key = session_key(owner, &event.source, &event.session_id);
        let mut inner = self.lock();
        prune(&mut inner, now);

        if event.event == "SessionEnd" {
            inner.sessions.remove(&key);
            inner.injected.remove(&key);
            inner.alerts.retain(|a| a.session_key != key);
            return self.bump(&mut inner);
        }

        let session = inner
            .sessions
            .entry(key.clone())
            .or_insert_with(|| Session {
                source: event.source.clone(),
                short: short_id(&key),
                name: display_name(event.cwd.as_deref(), &event.source),
                cwd: event.cwd.clone(),
                state: SessionState::Idle,
                tool: None,
                det: None,
                model: None,
                since: now,
                updated: now,
            });
        if let Some(cwd) = event.cwd.as_deref().filter(|c| !c.is_empty()) {
            if session.cwd.as_deref() != Some(cwd) {
                session.cwd = Some(cwd.to_string());
                session.name = display_name(Some(cwd), &event.source);
            }
        }
        if let Some(model) = event.model.as_deref().filter(|m| !m.is_empty()) {
            session.model = Some(cut(model, MODEL_LEN));
        }
        session.updated = now;

        let set_state = |session: &mut Session, state: SessionState| {
            if session.state != state {
                session.state = state;
                session.since = now;
            }
        };
        let mut clear_questions = false;
        let mut question = false;
        let mut error = false;
        match event.event.as_str() {
            "SessionStart" => set_state(session, SessionState::Idle),
            "UserPromptSubmit" => {
                set_state(session, SessionState::Working);
                session.tool = None;
                session.det = None;
                clear_questions = true;
            }
            "PreToolUse" => {
                set_state(session, SessionState::Working);
                session.tool = event.tool_name.as_deref().map(|t| cut(t, NAME_LEN));
                session.det = describe(event.tool_input.as_ref(), session.cwd.as_deref());
            }
            "PermissionRequest" => {
                set_state(session, SessionState::Waiting);
                question = true;
            }
            "Notification" => {
                if matches!(
                    event.notification_type.as_deref(),
                    Some("permission_prompt") | Some("elicitation_dialog")
                ) {
                    set_state(session, SessionState::Waiting);
                    question = true;
                }
            }
            "PostToolUse" | "PostToolUseFailure" => {
                set_state(session, SessionState::Working);
                clear_questions = true;
            }
            "SubagentStart" => {
                set_state(session, SessionState::Working);
                session.det = Some(cut(
                    &format!("subagent {}", event.agent_id.as_deref().unwrap_or_default()),
                    DET_LEN,
                ));
            }
            "SubagentStop" => session.det = None,
            "PreCompact" => {
                session.det = Some("compacting context".to_string());
            }
            "Stop" => {
                set_state(session, SessionState::Done);
                session.tool = None;
                session.det = None;
                clear_questions = true;
            }
            "StopFailure" => {
                set_state(session, SessionState::Error);
                error = true;
                clear_questions = true;
            }
            _ => {}
        }
        let (tool, det) = (session.tool.clone(), session.det.clone());

        let today = today_for(&mut inner.today, owner, now);
        match event.event.as_str() {
            "Stop" => today.turns += 1,
            "PostToolUse" | "PostToolUseFailure" => today.tools += 1,
            "StopFailure" => today.errors += 1,
            _ => {}
        }
        if clear_questions {
            inner
                .alerts
                .retain(|a| !(a.session_key == key && a.kind == AlertKind::Question));
        }
        if error {
            inner.alerts.retain(|a| a.session_key != key);
        }
        // A Bastion approval for this session already shows the call; a
        // second "question" for the same prompt would only duplicate it.
        let has_approval = inner.approvals.values().any(|p| p.alert.session_key == key);
        if (question && !has_approval) || error {
            let kind = if error {
                AlertKind::Error
            } else {
                AlertKind::Question
            };
            if !inner
                .alerts
                .iter()
                .any(|a| a.session_key == key && a.kind == kind)
            {
                let id = format!("{}-{}", if error { "er" } else { "q" }, inner.seq + 1);
                inner.alerts.push(Alert {
                    id,
                    kind,
                    session_key: key.clone(),
                    tool,
                    det,
                    exp: None,
                    input: None,
                    input_complete: false,
                });
            }
        }
        self.bump(&mut inner)
    }

    /// Opens a `remote` approval and waits for a surface to answer, at most
    /// `wait`. The session shows as waiting meanwhile; on timeout the
    /// approval is withdrawn and the host asks the person itself.
    #[allow(clippy::too_many_arguments)]
    pub async fn request_approval(
        &self,
        owner: &str,
        source: &str,
        session_id: &str,
        cwd: Option<&str>,
        tool: &str,
        input: Option<&Value>,
        full_input: Option<&str>,
        wait: Duration,
    ) -> (String, ApprovalOutcome) {
        let now = now_secs();
        let key = session_key(owner, source, session_id);
        let (tx, rx) = oneshot::channel();
        let id = {
            let mut inner = self.lock();
            prune(&mut inner, now);
            let id = new_approval_id(&inner.approvals);
            let det = describe(input, cwd);
            let alert = Alert {
                id: id.clone(),
                kind: AlertKind::Approval,
                session_key: key.clone(),
                tool: Some(cut(tool, NAME_LEN)),
                det,
                exp: Some(now + wait.as_secs()),
                input: full_input.map(clean_input).filter(|t| !t.is_empty()),
                // Complete only when kept whole and free of invisible
                // characters (shown as `�`, so the person sees the call is odd).
                input_complete: full_input.is_some_and(|t| {
                    t.chars().count() <= APPROVAL_INPUT_MAX && !t.chars().any(is_invisible)
                }),
            };
            if let Some(session) = inner.sessions.get_mut(&key) {
                session.state = SessionState::Waiting;
                session.since = now;
                session.updated = now;
            }
            inner
                .approvals
                .insert(id.clone(), PendingApproval { alert, reply: tx });
            // Only the id: `/events` is one feed for every owner, so the call
            // itself is read from each owner's own snapshot.
            let _ = self
                .events_tx
                .send(json!({ "event": "approval.requested", "id": id }).to_string());
            self.bump(&mut inner);
            id
        };

        let outcome = match tokio::time::timeout(wait, rx).await {
            Ok(Ok(Resolution::Approve)) => ApprovalOutcome::Approved,
            Ok(Ok(Resolution::Deny)) => ApprovalOutcome::Denied,
            // Timed out, or the sender vanished (cannot happen while the
            // entry is in the map; treated as a timeout either way).
            _ => {
                let mut inner = self.lock();
                if inner.approvals.remove(&id).is_some() {
                    let _ = self.events_tx.send(
                        json!({ "event": "approval.resolved", "id": id, "decision": "timeout", "by": "timeout" })
                            .to_string(),
                    );
                    restore_working(&mut inner, &key);
                    self.bump(&mut inner);
                }
                ApprovalOutcome::TimedOut
            }
        };
        (id, outcome)
    }

    /// Whether `owner`'s pending approval `id` carries its whole call
    /// (`None`: no such approval for this owner).
    pub fn approval_complete(&self, owner: &str, id: &str) -> Option<bool> {
        self.lock()
            .approvals
            .get(id)
            .filter(|p| owned_by(&p.alert.session_key, owner))
            .map(|p| p.alert.input_complete)
    }

    /// Answers one of `owner`'s pending approvals. `by`: `owner` or
    /// `device:<id>`. Another owner's approval is indistinguishable from a
    /// missing one.
    pub fn resolve(
        &self,
        owner: &str,
        id: &str,
        resolution: Resolution,
        by: &str,
    ) -> anyhow::Result<()> {
        let mut inner = self.lock();
        if !inner
            .approvals
            .get(id)
            .is_some_and(|p| owned_by(&p.alert.session_key, owner))
        {
            anyhow::bail!("no pending approval '{id}'");
        }
        let pending = inner
            .approvals
            .remove(id)
            .ok_or_else(|| anyhow::anyhow!("no pending approval '{id}'"))?;
        let key = pending.alert.session_key.clone();
        let _ = pending.reply.send(resolution);
        let decision = match resolution {
            Resolution::Approve => "approve",
            Resolution::Deny => "deny",
        };
        let _ = self.events_tx.send(
            json!({ "event": "approval.resolved", "id": id, "decision": decision, "by": by })
                .to_string(),
        );
        restore_working(&mut inner, &key);
        self.bump(&mut inner);
        Ok(())
    }

    /// `owner`'s snapshot (see `docs/en/presence.md`). `pet` is the
    /// companion's reduced view, `None` to leave it out.
    /// `max_sessions`/`max_bytes` cut it to a surface's capacity; alerts
    /// survive the cut first.
    pub fn snapshot(
        &self,
        owner: &str,
        pet: Option<Value>,
        max_sessions: usize,
        max_bytes: usize,
    ) -> Value {
        let now = now_secs();
        let mut inner = self.lock();
        prune(&mut inner, now);
        let today = today_for(&mut inner.today, owner, now);
        let today = json!({ "turns": today.turns, "tools": today.tools, "errors": today.errors });

        let mut sessions: Vec<&Session> = inner
            .sessions
            .iter()
            .filter(|(k, _)| owned_by(k, owner))
            .map(|(_, s)| s)
            .collect();
        sessions.sort_by(|a, b| {
            a.state
                .rank()
                .cmp(&b.state.rank())
                .then(b.updated.cmp(&a.updated))
        });
        let short_of: HashMap<&str, &str> = inner
            .sessions
            .iter()
            .map(|(k, s)| (k.as_str(), s.short.as_str()))
            .collect();

        let mut approvals: Vec<&Alert> = inner
            .approvals
            .values()
            .map(|p| &p.alert)
            .filter(|a| owned_by(&a.session_key, owner))
            .collect();
        approvals.sort_by(|a, b| a.exp.cmp(&b.exp).then(a.id.cmp(&b.id)));
        let alerts: Vec<Value> = approvals
            .into_iter()
            .chain(
                inner
                    .alerts
                    .iter()
                    .filter(|a| owned_by(&a.session_key, owner)),
            )
            .take(MAX_ALERTS)
            .map(|a| {
                let mut row = alert_json(a);
                if let Some(short) = short_of.get(a.session_key.as_str()) {
                    row["sid"] = json!(short);
                }
                row
            })
            .collect();

        let mut rows: Vec<Value> = sessions.iter().copied().map(session_json).collect();
        let total = rows.len();
        rows.truncate(max_sessions);
        let mut snapshot = json!({
            "v": PROTOCOL_VERSION,
            "seq": inner.seq,
            "now": now,
            "host": self.host,
            "sessions": rows,
            "more": total.saturating_sub(max_sessions),
            "alerts": alerts,
            "today": today,
        });
        if let Some(persona) = inner.personas.get(owner) {
            snapshot["persona"] = json!(persona);
        }
        if let Some(pet) = pet {
            snapshot["pet"] = pet;
        }
        if let Some((text, exp)) = inner.notes.get(owner) {
            snapshot["note"] = json!({ "text": text, "exp": exp });
        }
        trim_to_bytes(snapshot, max_bytes)
    }
}

/// Back to `working` once the session's last approval is answered.
fn restore_working(inner: &mut Inner, key: &str) {
    let still_waiting = inner.approvals.values().any(|p| p.alert.session_key == key);
    if !still_waiting {
        if let Some(session) = inner.sessions.get_mut(key) {
            if session.state == SessionState::Waiting {
                session.state = SessionState::Working;
                session.since = now_secs();
            }
        }
    }
}

/// `owner`'s counters for the current (UTC) day, reset when the day turns.
fn today_for<'a>(all: &'a mut HashMap<String, Today>, owner: &str, now: u64) -> &'a mut Today {
    let day = now / 86_400;
    let today = all.entry(owner.to_string()).or_default();
    if today.day != day {
        *today = Today {
            day,
            ..Today::default()
        };
    }
    today
}

/// Drops sessions silent for [`SESSION_TTL_SECS`] (and their alerts),
/// turns old `done` sessions `idle`, and drops an expired note.
fn prune(inner: &mut Inner, now: u64) {
    let stale: Vec<String> = inner
        .sessions
        .iter()
        .filter(|(_, s)| now.saturating_sub(s.updated) > SESSION_TTL_SECS)
        .map(|(k, _)| k.clone())
        .collect();
    for key in &stale {
        inner.sessions.remove(key);
        inner.injected.remove(key);
    }
    if !stale.is_empty() {
        inner.alerts.retain(|a| !stale.contains(&a.session_key));
    }
    for session in inner.sessions.values_mut() {
        if session.state == SessionState::Done
            && now.saturating_sub(session.since) > DONE_TO_IDLE_SECS
        {
            session.state = SessionState::Idle;
            session.since = now;
        }
    }
    inner.notes.retain(|_, (_, exp)| *exp > now);
}

fn session_json(s: &Session) -> Value {
    let mut row = json!({
        "id": s.short,
        "src": s.source,
        "name": s.name,
        "st": s.state,
        "since": s.since,
    });
    if let Some(tool) = &s.tool {
        row["tool"] = json!(tool);
    }
    if let Some(det) = &s.det {
        row["det"] = json!(det);
    }
    if let Some(model) = &s.model {
        row["model"] = json!(model);
    }
    row
}

fn alert_json(a: &Alert) -> Value {
    let mut row = json!({ "id": a.id, "kind": a.kind, "sid": short_id(&a.session_key) });
    if let Some(tool) = &a.tool {
        row["tool"] = json!(tool);
    }
    if let Some(det) = &a.det {
        row["det"] = json!(det);
    }
    if let Some(exp) = a.exp {
        row["exp"] = json!(exp);
    }
    if let Some(input) = &a.input {
        row["input"] = json!(input);
    }
    if a.kind == AlertKind::Approval {
        row["input_complete"] = json!(a.input_complete);
    }
    row
}

/// Drops sessions from the end (the least urgent) until the serialized
/// snapshot fits; the dropped ones are counted in `more`.
pub fn trim_to_bytes(mut snapshot: Value, max_bytes: usize) -> Value {
    loop {
        let size = serde_json::to_vec(&snapshot).map(|b| b.len()).unwrap_or(0);
        if size <= max_bytes {
            return snapshot;
        }
        let popped = snapshot["sessions"]
            .as_array_mut()
            .and_then(|rows| rows.pop())
            .is_some();
        if !popped {
            return snapshot;
        }
        let more = snapshot["more"].as_u64().unwrap_or(0) + 1;
        snapshot["more"] = json!(more);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const OWNER: &str = "alice";

    fn hub() -> PresenceHub {
        let (tx, _) = broadcast::channel(64);
        PresenceHub::new(tx, "workstation".into(), None)
    }

    fn event(name: &str) -> HostEvent {
        HostEvent {
            source: "claude-code".into(),
            session_id: "s-1".into(),
            event: name.into(),
            cwd: Some("/home/dev/project".into()),
            ..HostEvent::default()
        }
    }

    fn snap(hub: &PresenceHub) -> Value {
        hub.snapshot(OWNER, None, 10, 8192)
    }

    fn only_session(snapshot: &Value) -> Value {
        snapshot["sessions"][0].clone()
    }

    #[test]
    fn a_turn_moves_through_working_waiting_and_done() {
        let hub = hub();
        hub.record(OWNER, &event("SessionStart"));
        let s = snap(&hub);
        assert_eq!(only_session(&s)["st"], "idle");
        assert_eq!(only_session(&s)["name"], "project");
        assert_eq!(only_session(&s)["src"], "claude-code");

        hub.record(OWNER, &event("UserPromptSubmit"));
        let mut pre = event("PreToolUse");
        pre.tool_name = Some("Bash".into());
        pre.tool_input = Some(json!({ "command": "cargo test --all\nsecond line" }));
        hub.record(OWNER, &pre);
        let s = snap(&hub);
        assert_eq!(only_session(&s)["st"], "working");
        assert_eq!(only_session(&s)["tool"], "Bash");
        assert_eq!(only_session(&s)["det"], "cargo test --all");

        let mut note = event("Notification");
        note.notification_type = Some("permission_prompt".into());
        hub.record(OWNER, &note);
        let s = snap(&hub);
        assert_eq!(only_session(&s)["st"], "waiting");
        assert_eq!(s["alerts"][0]["kind"], "question");
        assert_eq!(s["alerts"][0]["sid"], only_session(&s)["id"]);

        hub.record(OWNER, &event("PostToolUse"));
        assert_eq!(snap(&hub)["alerts"].as_array().unwrap().len(), 0);

        hub.record(OWNER, &event("Stop"));
        let s = snap(&hub);
        assert_eq!(only_session(&s)["st"], "done");
        assert_eq!(s["today"]["turns"], 1);
        assert_eq!(s["today"]["tools"], 1);

        hub.record(OWNER, &event("SessionEnd"));
        assert_eq!(snap(&hub)["sessions"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn a_stop_failure_raises_an_error_alert() {
        let hub = hub();
        hub.record(OWNER, &event("UserPromptSubmit"));
        hub.record(OWNER, &event("StopFailure"));
        let s = snap(&hub);
        assert_eq!(only_session(&s)["st"], "error");
        assert_eq!(s["alerts"][0]["kind"], "error");
        assert_eq!(s["today"]["errors"], 1);
    }

    #[test]
    fn file_paths_are_shown_relative_to_the_session_folder() {
        let hub = hub();
        let mut pre = event("PreToolUse");
        pre.tool_name = Some("Edit".into());
        pre.tool_input = Some(json!({ "file_path": "/home/dev/project/src/main.rs" }));
        hub.record(OWNER, &pre);
        assert_eq!(only_session(&snap(&hub))["det"], "src/main.rs");
    }

    #[tokio::test]
    async fn an_approval_is_answered_from_a_surface() {
        let hub = std::sync::Arc::new(hub());
        hub.record(OWNER, &event("UserPromptSubmit"));
        let waiter = {
            let hub = hub.clone();
            tokio::spawn(async move {
                hub.request_approval(
                    OWNER,
                    "claude-code",
                    "s-1",
                    Some("/home/dev/project"),
                    "Bash",
                    Some(&json!({ "command": "git push origin main" })),
                    Some("git push origin main"),
                    Duration::from_secs(5),
                )
                .await
            })
        };
        let id = loop {
            let s = snap(&hub);
            if let Some(alert) = s["alerts"].as_array().and_then(|a| a.first()) {
                assert_eq!(alert["kind"], "approval");
                assert_eq!(alert["det"], "git push origin main");
                assert_eq!(alert["input"], "git push origin main");
                assert_eq!(alert["input_complete"], true);
                assert_eq!(only_session(&s)["st"], "waiting");
                break alert["id"].as_str().unwrap().to_string();
            }
            tokio::task::yield_now().await;
        };
        assert!(
            id.starts_with("ap-") && id.len() == 11,
            "unguessable id: {id}"
        );
        hub.resolve(OWNER, &id, Resolution::Approve, "owner")
            .unwrap();
        let (_, outcome) = waiter.await.unwrap();
        assert_eq!(outcome, ApprovalOutcome::Approved);
        assert_eq!(only_session(&snap(&hub))["st"], "working");
        assert!(hub.resolve(OWNER, &id, Resolution::Deny, "owner").is_err());
    }

    #[tokio::test]
    async fn owners_never_see_or_answer_each_other() {
        let hub = std::sync::Arc::new(hub());
        hub.record("bob", &event("UserPromptSubmit"));
        hub.set_persona("bob", Some("grace".into()));
        hub.set_note("bob", "lunch", 30);
        let waiter = {
            let hub = hub.clone();
            tokio::spawn(async move {
                hub.request_approval(
                    "bob",
                    "claude-code",
                    "s-1",
                    None,
                    "Bash",
                    None,
                    None,
                    Duration::from_millis(300),
                )
                .await
            })
        };
        let id = loop {
            let bob = hub.snapshot("bob", None, 10, 8192);
            if let Some(id) = bob["alerts"][0]["id"].as_str() {
                break id.to_string();
            }
            tokio::task::yield_now().await;
        };
        let alice = snap(&hub);
        assert_eq!(alice["sessions"].as_array().unwrap().len(), 0);
        assert_eq!(alice["alerts"].as_array().unwrap().len(), 0);
        assert!(alice.get("persona").is_none());
        assert!(alice.get("note").is_none());
        assert!(hub
            .resolve(OWNER, &id, Resolution::Approve, "owner")
            .is_err());
        let (_, outcome) = waiter.await.unwrap();
        assert_eq!(outcome, ApprovalOutcome::TimedOut);
    }

    #[tokio::test]
    async fn an_unanswered_approval_times_out_and_is_withdrawn() {
        let hub = hub();
        let (_, outcome) = hub
            .request_approval(
                OWNER,
                "claude-code",
                "s-1",
                None,
                "Bash",
                None,
                None,
                Duration::from_millis(20),
            )
            .await;
        assert_eq!(outcome, ApprovalOutcome::TimedOut);
        assert_eq!(snap(&hub)["alerts"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn the_snapshot_is_cut_to_a_device_capacity() {
        let hub = hub();
        for i in 0..12 {
            let mut e = event("UserPromptSubmit");
            e.session_id = format!("s-{i}");
            e.cwd = Some(format!("/work/project-number-{i}"));
            hub.record(OWNER, &e);
        }
        let s = hub.snapshot(OWNER, None, 5, 100_000);
        assert_eq!(s["sessions"].as_array().unwrap().len(), 5);
        assert_eq!(s["more"], 7);
        let small = hub.snapshot(OWNER, None, 12, 700);
        assert!(serde_json::to_vec(&small).unwrap().len() <= 700);
        assert_eq!(
            small["sessions"].as_array().unwrap().len() as u64 + small["more"].as_u64().unwrap(),
            12
        );
    }

    #[test]
    fn persona_and_note_show_in_the_snapshot_and_the_persona_persists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let (tx, mut rx) = broadcast::channel(64);
        let hub = PresenceHub::new(tx.clone(), "h".into(), Some(path.clone()));
        hub.set_persona(OWNER, Some("ada".into()));
        hub.set_note(
            OWNER,
            "back in 10 minutes, grabbing a coffee downstairs",
            30,
        );
        let s = snap(&hub);
        assert_eq!(s["persona"], "ada");
        assert_eq!(
            s["note"]["text"].as_str().unwrap().chars().count(),
            NOTE_MAX_CHARS
        );
        let first: Value = serde_json::from_str(&rx.try_recv().unwrap()).unwrap();
        assert_eq!(first["event"], "presence.persona");
        let reopened = PresenceHub::new(tx, "h".into(), Some(path));
        assert_eq!(reopened.persona(OWNER).as_deref(), Some("ada"));
        assert_eq!(reopened.persona("bob"), None);
    }

    #[test]
    fn invisible_characters_are_shown_and_never_complete() {
        assert_eq!(clean_input("git\u{202E}push  \n x"), "git\u{FFFD}push x");
        assert!(is_invisible('\u{200B}'));
        assert!(!is_invisible('a'));
        // Combining marks, variation selectors, fillers, separators and
        // other default-ignorables are invisible too; ordinary text is not.
        for c in [
            '\u{0301}',
            '\u{FE0F}',
            '\u{3164}',
            '\u{2028}',
            '\u{034F}',
            '\u{E0100}',
        ] {
            assert!(is_invisible(c), "{:04X}", c as u32);
        }
        for c in ['é', 'ç', '日', '-', ' ', '~'] {
            assert!(!is_invisible(c), "{c}");
        }
        assert_eq!(clean_input("rm\u{0338} -rf"), "rm\u{FFFD} -rf");
    }

    #[test]
    fn host_events_are_validated() {
        assert!(event("PreToolUse").validate().is_ok());
        let mut bad = event("Nope");
        assert!(bad.validate().is_err());
        bad.event = "Stop".into();
        bad.source = "has space".into();
        assert!(bad.validate().is_err());
        assert_eq!(short_id("alice\u{1f}claude-code:s-1").len(), 8);
    }
}
