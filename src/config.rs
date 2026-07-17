//! Bastion configuration — single source of truth for non-secret config.
//!
//! Layering strategy (D-09):
//!   bastion.toml (defaults) → BASTION__* env vars (overrides)
//!
//! Secrets (API keys, tokens) NEVER appear in bastion.toml — they come from .env only.

use crate::channel::OwnerMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Single [[mesh.peer]] entry from bastion.toml.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct MeshPeerConfig {
    pub owner_id: String,
    pub peer_url: String,
    pub age_pubkey: String,
    /// Tags this peer is allowed to receive (filter_for_mesh allowlist).
    /// Default: empty (no beliefs shared). Example: ["mercado", "calendario"].
    #[serde(default)]
    pub allowed_tags: Vec<String>,
}

/// Config section for mesh settings.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct MeshConfig {
    #[serde(default)]
    pub peer: Vec<MeshPeerConfig>,
    /// Interval in minutes between automatic mesh syncs (0 = disable periodic sync, manual /mesh-sync only).
    /// Default: 15.
    #[serde(default = "default_sync_interval")]
    pub sync_interval: u64,
}

fn default_sync_interval() -> u64 {
    15
}

/// `ReflectorConfig` moved to `bastion_cognition::learn` (M2 step 6, V2 fix —
/// `docs/revamp/M1-ADR-substrate-split.md`): the Reflector already took this
/// struct as a constructor parameter (never read the global `Config`), so the
/// only remaining leak was the struct DEFINITION living in this app-level
/// module while `bastion-cognition::learn` (an extension crate) needed it.
/// Extensions never read `crate::config` (app format is app-only); the app
/// parses `bastion.toml`'s `[reflector]` table into `bastion-cognition`'s own
/// type and re-exports it here under the old path so `BastionConfig.reflector`
/// keeps compiling unchanged.
pub use bastion_cognition::learn::ReflectorConfig;

#[derive(Debug, Deserialize, Clone)]
pub struct BastionConfig {
    pub agent: AgentConfig,
    pub session: SessionConfig,
    pub logging: LoggingConfig,
    pub mcp: McpConfig,
    #[serde(default)]
    pub mcp_server: McpServerConfig,
    pub channels: ChannelsConfig,
    #[serde(default)]
    pub mesh: MeshConfig,
    #[serde(default)]
    pub reflector: ReflectorConfig,
    /// CHAN-02/D-05: unified owner-identity table — resolves one canonical owner_id
    /// from any of 6 channel-specific identifiers. Replaces scattered per-channel
    /// env-var parsing (BASTION_TELEGRAM_OWNERS, BASTION_WEBHOOK_OWNERS) as the
    /// source of truth for OwnerMap construction (see `owner_map_for_*` below).
    #[serde(default)]
    pub identity: IdentityConfig,
    /// Ciclo 2.4 (`docs/revamp/C2-backend-profile-design.md`): optional
    /// `[backend]` section. Absent entirely = `#[serde(default)]` empty
    /// `BackendConfig`, which `backend_profile_from_config` maps to
    /// `ConversationBackend::Model` + no delegation — byte-identical to
    /// pre-Ciclo-2.4 behavior for any deployment that doesn't add this section.
    #[serde(default)]
    pub backend: BackendConfig,
    /// M4-07 (`docs/revamp/BACKLOG.md`): optional `[auth.<profile>]` tables —
    /// named credential/entitlement REFERENCES (never a token/secret value)
    /// that `[backend] auth = "<profile>"` (or a future per-owner override)
    /// points at by id. Absent entirely = `#[serde(default)]` empty
    /// `AuthConfig` — byte-identical to every pre-M4-07 deployment (an
    /// `AuthProfileRef` that names a profile nobody configured simply fails
    /// to resolve once a real `AuthResolver` is wired; with none wired,
    /// `NullAuthResolver` keeps resolving everything `Ok`, unchanged).
    #[serde(default)]
    pub auth: AuthConfig,
}

/// M4-07: one configured `[auth.<profile>]` entry — a REFERENCE to a
/// credential/entitlement, never the credential itself (D-09: secrets never
/// in bastion.toml). `HostCli` names a CLI whose OWN subscription-login flow
/// (`claude auth login`, `codex login`, `opencode auth login`) is assumed
/// already done on the host — Bastion never performs that login itself,
/// only checks (by reference, via the CLI's own read-only status surface)
/// that it's already in effect. `ApiKey` is the traditional path: the actual
/// key lives in the named env var, never in this file — orthogonal to
/// subscription backends, never a requirement (M4-07 acceptance criterion).
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum AuthProfileEntry {
    HostCli { cli: String },
    ApiKey { env_var: String },
}

/// Flat map of `[auth.<profile>]` tables, keyed by profile id — the exact
/// string a `[backend] auth = "<profile>"`/`AuthProfileRef` names.
/// `#[serde(default)]` (via the `#[serde(default)]` on `BastionConfig.auth`
/// above) so bastion.toml files with zero `[auth.*]` sections (every
/// deployment before M4-07) keep parsing unchanged.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AuthConfig {
    #[serde(flatten)]
    pub profiles: HashMap<String, AuthProfileEntry>,
}

/// Ciclo 2.4 declarative config for the kernel's `BackendProfile` (M4 scope
/// per the design doc §6: no rich UX/login flow here, just TOML). Lives in
/// the app crate — the kernel (`bastion-runtime`) never parses TOML or knows
/// this shape (M1 ADR: crates are mechanism, config format is app policy).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct BackendConfig {
    /// `"model"` (default, or the key omitted) | `"runtime:<id>"` — `<id>`
    /// must match a `RuntimeDescriptor::id` the composition root (`main.rs`)
    /// actually registered (Codex/acpx conditioned on `health()`/auth at
    /// startup). An id that isn't registered, or is registered but
    /// unhealthy, fails the turn with a typed error (§5.6) — it never
    /// silently falls back to `model`.
    #[serde(default)]
    pub conversation: Option<String>,
    /// Runtime id for delegated tasks (A-07) — independent of `conversation`
    /// (a `model`-conversation owner can still delegate; a
    /// `runtime`-conversation owner isn't forced to also delegate). `None` =
    /// delegation disabled.
    #[serde(default)]
    pub task_runtime: Option<String>,
    /// Opaque credential/entitlement reference threaded to the adapter,
    /// orthogonal to backend choice (A-01 §1.1). Resolution happens outside
    /// this config (host-authenticated CLI, subscription login, ...).
    #[serde(default)]
    pub auth: Option<String>,
}

/// Maps the app's `[backend]` TOML section onto the kernel's
/// `BackendProfile` (Ciclo 2.4). `coverage_note` is intentionally left `None`
/// here — the composition root fills it in from the resolved runtime's own
/// `RuntimeDescriptor::policy_coverage` once it looks the id up in the
/// `RuntimeRegistry` (main.rs), never invented here from the config string.
pub fn backend_profile_from_config(
    cfg: &BackendConfig,
) -> bastion_runtime::agent::backend::BackendProfile {
    use bastion_runtime::agent::backend::{BackendProfile, ConversationBackend};

    let conversation = match cfg.conversation.as_deref() {
        None | Some("model") | Some("") => ConversationBackend::Model,
        Some(spec) => {
            let id = spec.strip_prefix("runtime:").unwrap_or(spec);
            ConversationBackend::Runtime(id.to_string())
        }
    };

    BackendProfile {
        conversation,
        task_runtime: cfg.task_runtime.clone(),
        auth: cfg.auth.clone().map(bastion_agent_runtime::AuthProfileRef),
        coverage_note: None,
    }
}

/// Single `[[identity]]` entry from bastion.toml — one row per human owner.
///
/// Mirrors `MeshPeerConfig`'s array-of-tables shape. One optional column per
/// supported channel identifier; `owner_id` is the only required field.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct IdentityEntry {
    pub owner_id: String,
    #[serde(default)]
    pub telegram_chat_id: Option<String>,
    #[serde(default)]
    pub webhook_token: Option<String>,
    #[serde(default)]
    pub whatsapp_phone: Option<String>,
    #[serde(default)]
    pub discord_user_id: Option<String>,
    #[serde(default)]
    pub slack_user_id: Option<String>,
    #[serde(default)]
    pub email_address: Option<String>,
}

/// Config section holding the full `[[identity]]` array-of-tables (CHAN-02/D-05).
///
/// `#[serde(transparent)]`: this single-field wrapper deserializes directly from
/// the bare TOML array `[[identity]]` (a sequence) rather than requiring the
/// redundant nested `[[identity.identity]]` shape — the wrapper only exists so
/// `owner_map_for_*` functions take a named `&IdentityConfig` type (matching
/// 10-RESEARCH.md Pattern 2), not to introduce an extra TOML nesting level.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(transparent)]
pub struct IdentityConfig {
    pub identity: Vec<IdentityEntry>,
}

/// Column-extractor function pointer type, factored out to satisfy
/// `clippy::type_complexity` on the `columns` array below.
type IdentityColumnExtractor = fn(&IdentityEntry) -> &Option<String>;

/// Fail loud (T-10-02-01) on any misconfiguration that would create an ambiguous
/// owner mapping: an empty `owner_id`, or a channel-identifier value repeated
/// across two or more `[[identity]]` rows. Two rows both OMITTING the same column
/// (both `None`) is NOT a collision — only `Some(x) == Some(x)` across DIFFERENT
/// rows is ambiguous.
fn validate_identity_table(cfg: &IdentityConfig) -> anyhow::Result<()> {
    for entry in &cfg.identity {
        if entry.owner_id.is_empty() {
            anyhow::bail!("identity table validation failed: empty owner_id in [[identity]] entry");
        }
    }

    // (column name, extractor) pairs — checked independently, first duplicate wins.
    let columns: [(&str, IdentityColumnExtractor); 6] = [
        ("telegram_chat_id", |e| &e.telegram_chat_id),
        ("webhook_token", |e| &e.webhook_token),
        ("whatsapp_phone", |e| &e.whatsapp_phone),
        ("discord_user_id", |e| &e.discord_user_id),
        ("slack_user_id", |e| &e.slack_user_id),
        ("email_address", |e| &e.email_address),
    ];

    for (column_name, extract) in columns {
        let mut seen = std::collections::HashSet::new();
        for entry in &cfg.identity {
            if let Some(value) = extract(entry) {
                if !seen.insert(value.clone()) {
                    anyhow::bail!(
                        "identity table validation failed: duplicate {} value '{}' across [[identity]] rows",
                        column_name,
                        value
                    );
                }
            }
        }
    }

    Ok(())
}

/// Project the `telegram_chat_id` column of the identity table into the plain
/// `OwnerMap` shape `telegram.rs` already consumes — telegram.rs is NOT modified.
pub fn owner_map_for_telegram(cfg: &IdentityConfig) -> OwnerMap {
    OwnerMap(
        cfg.identity
            .iter()
            .filter_map(|e| {
                e.telegram_chat_id
                    .as_ref()
                    .map(|id| (id.clone(), e.owner_id.clone()))
            })
            .collect(),
    )
}

/// Project the `webhook_token` column into an `OwnerMap` — webhook.rs unchanged.
pub fn owner_map_for_webhook(cfg: &IdentityConfig) -> OwnerMap {
    OwnerMap(
        cfg.identity
            .iter()
            .filter_map(|e| {
                e.webhook_token
                    .as_ref()
                    .map(|id| (id.clone(), e.owner_id.clone()))
            })
            .collect(),
    )
}

/// Project the `whatsapp_phone` column into an `OwnerMap` (CHAN-01).
pub fn owner_map_for_whatsapp(cfg: &IdentityConfig) -> OwnerMap {
    OwnerMap(
        cfg.identity
            .iter()
            .filter_map(|e| {
                e.whatsapp_phone
                    .as_ref()
                    .map(|id| (id.clone(), e.owner_id.clone()))
            })
            .collect(),
    )
}

/// Project the `discord_user_id` column into an `OwnerMap` (CHAN-03).
pub fn owner_map_for_discord(cfg: &IdentityConfig) -> OwnerMap {
    OwnerMap(
        cfg.identity
            .iter()
            .filter_map(|e| {
                e.discord_user_id
                    .as_ref()
                    .map(|id| (id.clone(), e.owner_id.clone()))
            })
            .collect(),
    )
}

/// Project the `slack_user_id` column into an `OwnerMap` (CHAN-03).
pub fn owner_map_for_slack(cfg: &IdentityConfig) -> OwnerMap {
    OwnerMap(
        cfg.identity
            .iter()
            .filter_map(|e| {
                e.slack_user_id
                    .as_ref()
                    .map(|id| (id.clone(), e.owner_id.clone()))
            })
            .collect(),
    )
}

/// Project the `email_address` column into an `OwnerMap` (CHAN-03).
pub fn owner_map_for_email(cfg: &IdentityConfig) -> OwnerMap {
    OwnerMap(
        cfg.identity
            .iter()
            .filter_map(|e| {
                e.email_address
                    .as_ref()
                    .map(|id| (id.clone(), e.owner_id.clone()))
            })
            .collect(),
    )
}

/// `AgentConfig` moved to `bastion_types` (M2 step 6, V2 fix —
/// `docs/revamp/M1-ADR-substrate-split.md`): `interop::export::{export_full,
/// export_template}` (moving to `bastion-mesh`) only ever read this
/// sub-struct (`cfg.agent.{default_model,daily_budget_usd}`) through the
/// whole `BastionConfig` — a leak of the app's config format into an
/// extension crate. Narrowing their signature to `&AgentConfig` and moving
/// the struct to `bastion-types` (pure `Deserialize` data, mirroring the
/// `MeshPeerConfig`/`ReflectorConfig` precedents) lets `bastion-mesh` depend
/// on the type without depending on `crate::config`. Re-exported here so
/// `BastionConfig.agent` keeps compiling unchanged.
pub use bastion_types::AgentConfig;

#[derive(Debug, Deserialize, Clone)]
pub struct SessionConfig {
    pub db_path: String,
    pub autocompact_threshold: f64,
    pub keep_last_n: u32,
}

/// Runtime-owned model selection written by the local `/model` command.
/// It intentionally lives beside the session database instead of in
/// `bastion.toml`: operators keep a reviewable default in TOML while a user's
/// interactive choice survives daemon restarts without rewriting that file.
#[derive(Debug, Deserialize, Serialize)]
struct ModelSelection {
    model: String,
}

pub fn model_selection_path(cfg: &BastionConfig) -> PathBuf {
    Path::new(&cfg.session.db_path)
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .join("model-selection.json")
}

pub fn load_model_selection(cfg: &BastionConfig) -> Option<String> {
    let raw = std::fs::read_to_string(model_selection_path(cfg)).ok()?;
    let selection: ModelSelection = serde_json::from_str(&raw).ok()?;
    (!selection.model.trim().is_empty()).then_some(selection.model)
}

pub fn save_model_selection(path: &Path, model: &str) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let temporary = path.with_extension("json.tmp");
    let contents = serde_json::to_vec_pretty(&ModelSelection {
        model: model.to_string(),
    })
    .expect("model selection must serialize");
    std::fs::write(&temporary, contents)?;
    std::fs::rename(temporary, path)
}

pub fn clear_model_selection(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct LoggingConfig {
    pub log_path: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct McpConfig {
    pub tool_call_timeout_secs: u64,
    #[serde(default)]
    pub servers: HashMap<String, McpServerEntry>,
}

/// Moved to `bastion-types` (M2 step 5) — pure `Deserialize` data referenced by
/// `bastion-mcp`'s `McpClient::connect_from_config`, which cannot depend on this
/// product-level config module. Re-exported here so `crate::config::McpServerEntry`
/// (e.g. `tests/mcp_client_e2e.rs`) keeps resolving unchanged.
pub use bastion_types::McpServerEntry;

/// Individual token entry for the MCP server (static token auth, D-05).
#[derive(Debug, Deserialize, Clone)]
pub struct McpServerTokenConfig {
    /// If true, this token can list/read resources but not invoke tools.
    #[serde(default)]
    pub read_only: bool,
    /// Owner identity bound to this token.
    pub owner_id: String,
    /// 09-REVIEW.md CR-03: opt this token into invoking capabilities that require
    /// leaving the host (`CapabilityRegistry::invoke`'s `external` egress check).
    /// Default `false` — tools invoked with this token get `PrivacyTier::LocalOnly`
    /// (fail-closed: only capabilities with `is_local() == true` will run) unless the
    /// operator explicitly sets this to `true`.
    #[serde(default)]
    pub cloud_ok: bool,
}

/// Config section for the MCP server (not the client — D-08).
#[derive(Debug, Deserialize, Clone, Default)]
pub struct McpServerConfig {
    /// Enable the streamable HTTP MCP server.
    #[serde(default)]
    pub enabled: bool,
    /// Path to mount on, e.g. "/mcp".
    #[serde(default = "default_mcp_server_path")]
    pub mount_path: String,
    /// Per-token permissions map.
    #[serde(default)]
    pub tokens: HashMap<String, McpServerTokenConfig>,
}

fn default_mcp_server_path() -> String {
    "/mcp".into()
}

#[derive(Debug, Deserialize, Clone)]
pub struct ChannelsConfig {
    pub telegram: ChannelConfig,
    pub webhook: ChannelConfig,
    /// CHAN-01/CHAN-03: new channel sections are optional — absent from bastion.toml
    /// today, `#[serde(default)]` keeps existing deployments parsing unchanged.
    #[serde(default)]
    pub whatsapp: Option<ChannelConfig>,
    #[serde(default)]
    pub discord: Option<ChannelConfig>,
    #[serde(default)]
    pub slack: Option<ChannelConfig>,
    #[serde(default)]
    pub email: Option<ChannelConfig>,
    /// VOICE-01: voice needs extra fields (wake-word opt-in, voice id) beyond the
    /// plain enabled toggle, so it gets a dedicated struct instead of `ChannelConfig`.
    #[serde(default)]
    pub voice: VoiceChannelConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ChannelConfig {
    pub enabled: bool,
}

/// VOICE-01 config section: local voice channel (push-to-talk default, D-10).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct VoiceChannelConfig {
    #[serde(default)]
    pub enabled: bool,
    /// D-10: wake-word ("modo aberto") is opt-in — off by default (push-to-talk only).
    #[serde(default)]
    pub wake_word_enabled: bool,
    /// Kokoro voice id. Default `pf_dora` — confirmed pt-BR voice (10-RESEARCH.md).
    #[serde(default = "default_voice_id")]
    pub voice: String,
}

fn default_voice_id() -> String {
    "pf_dora".to_string()
}

/// Load [[mesh.peer]] entries from bastion.toml into a MeshPeerMap.
/// Called once at daemon startup. Errors are logged but do not abort startup
/// (daemon runs without mesh peers if none configured).
pub fn load_mesh_peers(config: &BastionConfig) -> bastion_mesh::mesh::MeshPeerMap {
    let mut map = bastion_mesh::mesh::MeshPeerMap::new();
    for entry in &config.mesh.peer {
        map.register(
            entry.owner_id.clone(),
            bastion_mesh::mesh::MeshPeer {
                peer_url: entry.peer_url.clone(),
                age_pubkey: entry.age_pubkey.clone(),
                allowed_tags: entry.allowed_tags.clone(),
            },
        );
        tracing::info!(
            event    = "mesh_peer_loaded",
            owner_id = %entry.owner_id,
            peer_url = %entry.peer_url,
        );
    }
    map
}

/// Validate age public key format. Must match ^age1[0-9a-z]+$ (bech32 age key).
///
/// SEC-01: called before any config write to prevent injection via malformed key strings.
fn validate_age_pubkey(key: &str) -> anyhow::Result<()> {
    // Static regex — compile once. age keys are lowercase bech32: age1 + [0-9a-z]+
    let re = regex::Regex::new(r"^age1[0-9a-z]+$").expect("static regex must compile");
    if !re.is_match(key) {
        anyhow::bail!("invalid age_pubkey format — must match ^age1[0-9a-z]+$ (SEC-01)");
    }
    Ok(())
}

/// Append a new [[mesh.peer]] entry to bastion.toml using toml_edit.
///
/// SEC-01: uses toml_edit (programmatic table construction, no string interpolation).
///         age_pubkey validated against ^age1[0-9a-z]+$ before touching the file.
/// WR-02: bails on read error instead of overwriting config with empty + new entry.
///        Existing entries (including allowed_tags) are preserved via toml_edit parse/append.
///        Atomic write via temp-file + rename prevents partial write corruption.
pub async fn append_mesh_peer(
    owner_id: &str,
    peer_url: &str,
    age_pubkey: &str,
    allowed_tags: &[String],
) -> anyhow::Result<()> {
    use toml_edit::{value, DocumentMut};

    // SEC-01: validate age_pubkey format before touching the file
    validate_age_pubkey(age_pubkey)?;

    let path = std::env::var("BASTION_CONFIG").unwrap_or_else(|_| "bastion.toml".to_string());

    // WR-02: bail on read error — do NOT fall back to empty string.
    // Falling back to "" would overwrite the entire config with just the new peer entry.
    let current = tokio::fs::read_to_string(&path)
        .await
        .map_err(|e| anyhow::anyhow!("failed to read '{}' before appending peer: {}", path, e))?;

    // Parse as mutable TOML document (toml_edit preserves comments and formatting)
    let mut doc: DocumentMut = current
        .parse()
        .map_err(|e| anyhow::anyhow!("failed to parse '{}' as TOML: {}", path, e))?;

    // Build the new [[mesh.peer]] entry as a toml_edit Table
    let mut peer_entry = toml_edit::Table::new();
    peer_entry["owner_id"] = value(owner_id);
    peer_entry["peer_url"] = value(peer_url);
    peer_entry["age_pubkey"] = value(age_pubkey);
    if !allowed_tags.is_empty() {
        let mut tags_array = toml_edit::Array::new();
        for t in allowed_tags {
            tags_array.push(t.as_str());
        }
        peer_entry["allowed_tags"] = toml_edit::Item::Value(toml_edit::Value::Array(tags_array));
    }

    // Ensure doc["mesh"] exists as a table
    if !doc.contains_key("mesh") {
        doc["mesh"] = toml_edit::Item::Table(toml_edit::Table::new());
    }

    // Append to [[mesh.peer]] array-of-tables
    match doc["mesh"]["peer"].as_array_of_tables_mut() {
        Some(arr) => {
            arr.push(peer_entry);
        }
        None => {
            // [[mesh.peer]] key doesn't exist yet — create it
            let mut aot = toml_edit::ArrayOfTables::new();
            aot.push(peer_entry);
            doc["mesh"]["peer"] = toml_edit::Item::ArrayOfTables(aot);
        }
    }

    // Atomic write: write to .tmp then rename to prevent partial write corruption
    let tmp_path = format!("{}.tmp", path);
    tokio::fs::write(&tmp_path, doc.to_string())
        .await
        .map_err(|e| anyhow::anyhow!("failed to write tmp config '{}': {}", tmp_path, e))?;
    tokio::fs::rename(&tmp_path, &path)
        .await
        .map_err(|e| anyhow::anyhow!("failed to rename '{}' → '{}': {}", tmp_path, path, e))?;

    Ok(())
}

/// Load BastionConfig from a TOML file, with env var overrides.
///
/// Env var naming convention (config-rs separator "__"):
///   BASTION__AGENT__DEFAULT_MODEL=claude-opus-4-7
///   BASTION__SESSION__DB_PATH=/data/sessions.db
pub fn load_config(path: &str) -> anyhow::Result<BastionConfig> {
    let cfg = config::Config::builder()
        .add_source(config::File::with_name(path))
        .add_source(config::Environment::with_prefix("BASTION").separator("__"))
        .build()?;
    let cfg: BastionConfig = cfg.try_deserialize()?;
    // T-10-02-01: fail loud on ambiguous identity mapping before the daemon can start.
    validate_identity_table(&cfg.identity)?;
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_config_from_bastion_toml() {
        let cfg = load_config("bastion.toml").expect("bastion.toml must exist at repo root");
        // default_model is deployment-specific (Mario runs OpenRouter free); assert it's set,
        // not a specific value — this test verifies config parsing, not the chosen model.
        assert!(
            !cfg.agent.default_model.is_empty(),
            "default_model must be set in bastion.toml"
        );
        assert!(cfg.agent.daily_budget_usd > 0.0);
        assert!(cfg.mcp.servers.contains_key("memupalace"));
        assert_eq!(
            cfg.mcp.servers["memupalace"].url,
            "http://127.0.0.1:8001/mcp"
        );
    }

    // ── SEC-01 age_pubkey validation tests ───────────────────────────────────

    /// SEC-01: append_mesh_peer must reject age_pubkey not matching ^age1[0-9a-z]+$
    #[tokio::test]
    async fn test_append_mesh_peer_rejects_invalid_age_pubkey() {
        let result = append_mesh_peer(
            "owner1",
            "https://peer.example.com",
            "not-an-age-key", // does not match ^age1[0-9a-z]+$
            &[],
        )
        .await;
        assert!(result.is_err(), "must reject invalid age_pubkey");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("age_pubkey") || msg.contains("SEC-01"),
            "error must reference age_pubkey: {msg}",
        );
    }

    /// SEC-01: TOML-breaking characters in age_pubkey must be caught by regex before touching file.
    #[tokio::test]
    async fn test_append_mesh_peer_rejects_toml_injection_in_age_pubkey() {
        // injection attempt via TOML-breaking characters (quotes, newlines)
        let result = append_mesh_peer(
            "owner1",
            "https://peer.example.com",
            "age1abcdef\"\nmalicious_key = true\nage1", // injection payload
            &[],
        )
        .await;
        assert!(
            result.is_err(),
            "must reject age_pubkey with TOML-breaking characters"
        );
    }

    /// SEC-01: valid age_pubkey passes regex (does not write to file — bails on missing config).
    /// This confirms the regex itself is not overly restrictive.
    #[tokio::test]
    async fn test_validate_age_pubkey_accepts_valid_key() {
        // validate_age_pubkey only — no filesystem I/O
        let result =
            validate_age_pubkey("age1ql3z7hjy54pw3yywmz2fxnftqqhrlrr2e9xsmrwckkl2u5dc3kzqsrcq7t");
        assert!(result.is_ok(), "valid age pubkey must pass validation");
    }

    /// SEC-01: age_pubkey with uppercase must be rejected (bech32 is lowercase only).
    #[tokio::test]
    async fn test_validate_age_pubkey_rejects_uppercase() {
        let result = validate_age_pubkey("AGE1UPPERCASE");
        assert!(result.is_err(), "uppercase age_pubkey must be rejected");
    }

    // ── CHAN-02/D-05 identity table validation tests ─────────────────────────

    /// Minimal valid bastion.toml required-sections boilerplate, with `{extra}`
    /// substituted in for the `[[identity]]` rows under test.
    fn minimal_toml_with_identity(extra: &str) -> String {
        format!(
            r#"
[agent]
default_model = "test-model"
daily_budget_usd = 1.0

[session]
db_path = "/tmp/test-sessions.db"
autocompact_threshold = 0.8
keep_last_n = 20

[logging]
log_path = "/tmp/test.log"

[mcp]
tool_call_timeout_secs = 30

[channels.telegram]
enabled = true

[channels.webhook]
enabled = false

{extra}
"#,
            extra = extra
        )
    }

    /// Write `contents` to a fresh temp file and return its path (kept alive by the
    /// returned `NamedTempFile` guard — caller must hold it for the test's duration).
    /// `config::File::with_name` appends its own extension resolution, so we write a
    /// `.toml` file and pass the path WITHOUT the extension, matching `load_config`'s
    /// existing convention (`load_config("bastion.toml")` in the test above resolves
    /// via a bare filename too — but config-rs also accepts an explicit full path
    /// with extension). We pass the full path with `.toml` extension directly.
    fn write_temp_toml(contents: &str) -> tempfile::TempPath {
        use std::io::Write;
        let mut file = tempfile::Builder::new()
            .suffix(".toml")
            .tempfile()
            .expect("failed to create temp file");
        file.write_all(contents.as_bytes())
            .expect("failed to write temp toml");
        file.into_temp_path()
    }

    /// Test 1: duplicate `telegram_chat_id` across two `[[identity]]` rows fails.
    #[test]
    fn test_validate_identity_table_rejects_duplicate_telegram_chat_id() {
        let toml = minimal_toml_with_identity(
            r#"
[[identity]]
owner_id = "alice"
telegram_chat_id = "111"

[[identity]]
owner_id = "bob"
telegram_chat_id = "111"
"#,
        );
        let path = write_temp_toml(&toml);
        let path_str = path.to_str().unwrap().to_string();
        let result = load_config(&path_str);
        assert!(
            result.is_err(),
            "duplicate telegram_chat_id must fail load_config"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("duplicate"),
            "error must mention 'duplicate': {msg}"
        );
    }

    /// Test 2: empty `owner_id` in an `[[identity]]` row fails.
    #[test]
    fn test_validate_identity_table_rejects_empty_owner_id() {
        let toml = minimal_toml_with_identity(
            r#"
[[identity]]
owner_id = ""
telegram_chat_id = "111"
"#,
        );
        let path = write_temp_toml(&toml);
        let path_str = path.to_str().unwrap().to_string();
        let result = load_config(&path_str);
        assert!(result.is_err(), "empty owner_id must fail load_config");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("empty owner_id"),
            "error must mention 'empty owner_id': {msg}"
        );
    }

    /// Test 3: N distinct rows (including columns entirely absent) load fine.
    #[test]
    fn test_validate_identity_table_accepts_distinct_entries() {
        let toml = minimal_toml_with_identity(
            r#"
[[identity]]
owner_id = "alice"
telegram_chat_id = "111"
whatsapp_phone = "+5511900000001"

[[identity]]
owner_id = "bob"
discord_user_id = "222"

[[identity]]
owner_id = "carol"
"#,
        );
        let path = write_temp_toml(&toml);
        let path_str = path.to_str().unwrap().to_string();
        let cfg = load_config(&path_str).expect("distinct identity rows must load");
        assert_eq!(cfg.identity.identity.len(), 3);
    }

    /// Test 4: two rows both omitting `discord_user_id` (both `None`) is NOT a
    /// duplicate — only `Some(x) == Some(x)` across DIFFERENT rows is ambiguous.
    #[test]
    fn test_validate_identity_table_absent_column_is_not_a_collision() {
        let toml = minimal_toml_with_identity(
            r#"
[[identity]]
owner_id = "alice"
telegram_chat_id = "111"

[[identity]]
owner_id = "bob"
telegram_chat_id = "222"
"#,
        );
        let path = write_temp_toml(&toml);
        let path_str = path.to_str().unwrap().to_string();
        let result = load_config(&path_str);
        assert!(
            result.is_ok(),
            "two rows both omitting discord_user_id must not collide: {:?}",
            result.err()
        );
    }

    // ── CHAN-02/D-05 owner_map_for_* projection tests ────────────────────────

    fn two_entries_one_with(field_setter: impl Fn(&mut IdentityEntry)) -> IdentityConfig {
        let mut with_value = IdentityEntry {
            owner_id: "alice".to_string(),
            telegram_chat_id: None,
            webhook_token: None,
            whatsapp_phone: None,
            discord_user_id: None,
            slack_user_id: None,
            email_address: None,
        };
        field_setter(&mut with_value);
        let without_value = IdentityEntry {
            owner_id: "bob".to_string(),
            telegram_chat_id: None,
            webhook_token: None,
            whatsapp_phone: None,
            discord_user_id: None,
            slack_user_id: None,
            email_address: None,
        };
        IdentityConfig {
            identity: vec![with_value, without_value],
        }
    }

    /// Test 1: owner_map_for_telegram — 2 entries (one Some, one None), resolves
    /// the Some row's identifier and produces exactly 1 inner map entry.
    #[test]
    fn test_owner_map_for_telegram() {
        let cfg = two_entries_one_with(|e| e.telegram_chat_id = Some("42".to_string()));
        let map = owner_map_for_telegram(&cfg);
        assert_eq!(map.resolve("42"), Some("alice"));
        assert_eq!(map.0.len(), 1);
    }

    /// Test 2: owner_map_for_whatsapp keyed on whatsapp_phone.
    #[test]
    fn test_owner_map_for_whatsapp() {
        let cfg = two_entries_one_with(|e| e.whatsapp_phone = Some("+5511900000000".to_string()));
        let map = owner_map_for_whatsapp(&cfg);
        assert_eq!(map.resolve("+5511900000000"), Some("alice"));
        assert_eq!(map.0.len(), 1);
    }

    /// Test 3: owner_map_for_discord keyed on discord_user_id.
    #[test]
    fn test_owner_map_for_discord() {
        let cfg = two_entries_one_with(|e| e.discord_user_id = Some("111222333".to_string()));
        let map = owner_map_for_discord(&cfg);
        assert_eq!(map.resolve("111222333"), Some("alice"));
        assert_eq!(map.0.len(), 1);
    }

    /// Test 4: owner_map_for_slack keyed on slack_user_id.
    #[test]
    fn test_owner_map_for_slack() {
        let cfg = two_entries_one_with(|e| e.slack_user_id = Some("U01ABCDEF".to_string()));
        let map = owner_map_for_slack(&cfg);
        assert_eq!(map.resolve("U01ABCDEF"), Some("alice"));
        assert_eq!(map.0.len(), 1);
    }

    /// Test 5: owner_map_for_email keyed on email_address.
    #[test]
    fn test_owner_map_for_email() {
        let cfg = two_entries_one_with(|e| e.email_address = Some("alice@example.com".to_string()));
        let map = owner_map_for_email(&cfg);
        assert_eq!(map.resolve("alice@example.com"), Some("alice"));
        assert_eq!(map.0.len(), 1);
    }

    /// Test 6: owner_map_for_webhook keyed on webhook_token.
    #[test]
    fn test_owner_map_for_webhook() {
        let cfg = two_entries_one_with(|e| e.webhook_token = Some("token-alice".to_string()));
        let map = owner_map_for_webhook(&cfg);
        assert_eq!(map.resolve("token-alice"), Some("alice"));
        assert_eq!(map.0.len(), 1);
    }

    /// Test 7: an empty IdentityConfig produces an empty OwnerMap from every
    /// projection function.
    #[test]
    fn test_owner_map_for_all_channels_empty_identity_config() {
        let cfg = IdentityConfig { identity: vec![] };
        assert_eq!(owner_map_for_telegram(&cfg).resolve("anything"), None);
        assert_eq!(owner_map_for_webhook(&cfg).resolve("anything"), None);
        assert_eq!(owner_map_for_whatsapp(&cfg).resolve("anything"), None);
        assert_eq!(owner_map_for_discord(&cfg).resolve("anything"), None);
        assert_eq!(owner_map_for_slack(&cfg).resolve("anything"), None);
        assert_eq!(owner_map_for_email(&cfg).resolve("anything"), None);
    }

    // ── Ciclo 2.4 — `[backend]` / BackendProfile mapping ─────────────────────

    /// Acceptance criterion 1 (`docs/revamp/C2-backend-profile-design.md`):
    /// bastion.toml's real `[backend]`-less config must still load and map to
    /// `ConversationBackend::Model` + no delegation — the exact default the
    /// whole test suite already runs against.
    #[test]
    fn test_bastion_toml_has_no_backend_section_and_defaults_to_model() {
        let cfg = load_config("bastion.toml").expect("bastion.toml must exist at repo root");
        use bastion_runtime::agent::backend::ConversationBackend;
        let profile = backend_profile_from_config(&cfg.backend);
        assert_eq!(profile.conversation, ConversationBackend::Model);
        assert!(profile.task_runtime.is_none());
        assert!(profile.auth.is_none());
        assert!(profile.coverage_note.is_none());
    }

    #[test]
    fn test_backend_config_absent_conversation_maps_to_model() {
        use bastion_runtime::agent::backend::ConversationBackend;
        let cfg = BackendConfig::default();
        assert_eq!(
            backend_profile_from_config(&cfg).conversation,
            ConversationBackend::Model
        );
    }

    #[test]
    fn test_backend_config_explicit_model_string_maps_to_model() {
        use bastion_runtime::agent::backend::ConversationBackend;
        let cfg = BackendConfig {
            conversation: Some("model".to_string()),
            ..Default::default()
        };
        assert_eq!(
            backend_profile_from_config(&cfg).conversation,
            ConversationBackend::Model
        );
    }

    #[test]
    fn test_backend_config_runtime_prefix_maps_to_runtime_id() {
        use bastion_runtime::agent::backend::ConversationBackend;
        let cfg = BackendConfig {
            conversation: Some("runtime:codex_app_server".to_string()),
            ..Default::default()
        };
        assert_eq!(
            backend_profile_from_config(&cfg).conversation,
            ConversationBackend::Runtime("codex_app_server".to_string())
        );
    }

    /// A bare id without the `runtime:` prefix is tolerated too (not a
    /// footgun for a typo'd config) — anything that isn't literally `"model"`
    /// or empty is treated as a runtime id.
    #[test]
    fn test_backend_config_bare_id_without_prefix_maps_to_runtime_id() {
        use bastion_runtime::agent::backend::ConversationBackend;
        let cfg = BackendConfig {
            conversation: Some("acpx_claude".to_string()),
            ..Default::default()
        };
        assert_eq!(
            backend_profile_from_config(&cfg).conversation,
            ConversationBackend::Runtime("acpx_claude".to_string())
        );
    }

    #[test]
    fn test_backend_config_task_runtime_and_auth_pass_through() {
        let cfg = BackendConfig {
            conversation: None,
            task_runtime: Some("acpx_claude".to_string()),
            auth: Some("host-claude-login".to_string()),
        };
        let profile = backend_profile_from_config(&cfg);
        assert_eq!(profile.task_runtime.as_deref(), Some("acpx_claude"));
        assert_eq!(
            profile.auth.map(|a| a.0),
            Some("host-claude-login".to_string())
        );
    }

    /// `[backend]` parses from real TOML too, not just the Rust struct
    /// literal above — exercises the actual `Deserialize` derive.
    #[test]
    fn test_backend_section_parses_from_toml() {
        let toml = minimal_toml_with_identity(
            r#"
[backend]
conversation = "runtime:codex_app_server"
task_runtime = "acpx_claude"
auth = "host-chatgpt-login"
"#,
        );
        let path = write_temp_toml(&toml);
        let path_str = path.to_str().unwrap().to_string();
        let cfg = load_config(&path_str).expect("[backend] section must parse");
        use bastion_runtime::agent::backend::ConversationBackend;
        let profile = backend_profile_from_config(&cfg.backend);
        assert_eq!(
            profile.conversation,
            ConversationBackend::Runtime("codex_app_server".to_string())
        );
        assert_eq!(profile.task_runtime.as_deref(), Some("acpx_claude"));
    }

    #[test]
    fn model_selection_is_atomic_and_clearable() {
        let directory = tempfile::tempdir().expect("temporary model state directory");
        let path = directory.path().join("model-selection.json");

        save_model_selection(&path, "gemini-2.5-pro").expect("save selection");
        let selection: ModelSelection =
            serde_json::from_slice(&std::fs::read(&path).expect("read selection"))
                .expect("parse selection");
        assert_eq!(selection.model, "gemini-2.5-pro");

        clear_model_selection(&path).expect("clear selection");
        assert!(!path.exists());
        clear_model_selection(&path).expect("clearing a missing selection is safe");
    }
}
