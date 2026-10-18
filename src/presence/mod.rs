//! Presence: one contract for every surface that shows Bastion or acts on
//! it, and Bastion mode for host agents (`docs/en/presence.md`).
//!
//! - [`hub`] — sessions, alerts, approvals, persona, note; the snapshot.
//! - [`policy`] — `[[presence.rules]]` on a host agent's tool calls.
//! - [`context`] — what a host agent is told so it acts as this Bastion.
//! - [`devices`] — paired screens Bastion pushes the snapshot to.
//! - [`routes`] — the `/presence/*` HTTP surface.
//! - [`hook`] — `bastion hook <host>`, the client side a host's hooks run.
//! - [`cli`] — `bastion presence …`.

pub mod cli;
pub mod context;
pub mod devices;
pub mod hook;
pub mod hub;
pub mod policy;
pub mod routes;

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};
use tokio::sync::broadcast;

use crate::config::PresenceConfig;
use crate::tui::CompanionHandle;

use self::context::ContextSources;
use self::devices::{DeviceRecord, DeviceRegistry};
use self::hub::{HostEvent, PresenceHub};

/// Where presence keeps its files: `$BASTION_DATA_DIR/presence`, else
/// `~/.config/bastion/presence` (next to `companion.json`).
pub fn presence_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("BASTION_DATA_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir).join("presence");
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join(".config")
        .join("bastion")
        .join("presence")
}

/// A short name for this machine, shown on devices.
fn host_name() -> String {
    std::env::var("BASTION_HOST_NAME")
        .ok()
        .or_else(|| std::env::var("HOSTNAME").ok())
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|s| s.trim().to_string())
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "bastion".to_string())
}

pub struct PresenceService {
    pub hub: Arc<PresenceHub>,
    pub devices: Arc<DeviceRegistry>,
    pub companion: CompanionHandle,
    pub context: ContextSources,
    pub config: PresenceConfig,
    pub events_tx: broadcast::Sender<String>,
}

impl PresenceService {
    pub fn new(
        config: PresenceConfig,
        events_tx: broadcast::Sender<String>,
        companion: CompanionHandle,
        context: ContextSources,
    ) -> Arc<Self> {
        let dir = presence_dir();
        Arc::new(Self {
            hub: Arc::new(PresenceHub::new(
                events_tx.clone(),
                host_name(),
                Some(dir.join("state.json")),
            )),
            devices: Arc::new(DeviceRegistry::load(Some(dir.join("devices.json")))),
            companion,
            context,
            config,
            events_tx,
        })
    }

    /// The companion as surfaces see it: level, xp, needs, the most urgent
    /// cue. `None` with the game off — a surface then draws no meters.
    pub fn pet(&self) -> Option<Value> {
        let full = serde_json::to_value(self.companion.snapshot()).ok()?;
        if full["game_enabled"] == Value::Bool(false) {
            return None;
        }
        let mut pet = json!({
            "level": full["level"],
            "xp": full["xp"],
            "needs": full["needs"],
        });
        if let Some(cue) = full["cues"].as_array().and_then(|c| c.first()) {
            pet["cue"] = cue.clone();
        }
        Some(pet)
    }

    /// `persona` is the one host agents act as: the owner's choice, else
    /// the registry's default (see `context::resolve_persona`).
    pub fn snapshot(&self, owner: &str, max_sessions: usize, max_bytes: usize) -> Value {
        let mut snapshot = self
            .hub
            .snapshot(owner, self.pet(), max_sessions, max_bytes);
        let chosen = snapshot["persona"].as_str().map(str::to_string);
        match context::resolve_persona(&self.context.personas, chosen.as_deref()) {
            Some(persona) => snapshot["persona"] = json!(persona),
            None => {
                if let Some(map) = snapshot.as_object_mut() {
                    map.remove("persona");
                }
            }
        }
        snapshot
    }

    /// What a paired device is shown: its owner's snapshot, cut to its caps.
    pub fn snapshot_for(&self, device: &DeviceRecord) -> Value {
        self.snapshot(
            &device.owner,
            device.caps.max_sessions,
            device.caps.max_bytes,
        )
    }

    /// Records a host event and lets the companion feel it: a session
    /// starting or ending, and activity on prompts, tool results and turn
    /// ends. A companion error never fails the event.
    pub fn record(&self, owner: &str, event: &HostEvent) -> u64 {
        let seq = self.hub.record(owner, event);
        let kind = match event.event.as_str() {
            "SessionStart" => Some("session-start"),
            "SessionEnd" => Some("session-stop"),
            "UserPromptSubmit" | "PostToolUse" | "Stop" => Some("activity"),
            _ => None,
        };
        if let Some(kind) = kind {
            if let Err(e) = self
                .companion
                .record_event(kind, &event.source, &self.events_tx)
            {
                tracing::debug!(event = "presence_companion_event_failed", error = %e);
            }
        }
        seq
    }

    /// Starts pushing the snapshot to paired devices.
    pub fn start_device_pusher(self: &Arc<Self>) {
        let service = Arc::clone(self);
        devices::spawn_pusher(
            Arc::clone(&self.devices),
            self.hub.subscribe_changes(),
            move |device| service.snapshot_for(device),
        );
    }
}
