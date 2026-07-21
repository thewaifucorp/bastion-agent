//! Staged configuration proposals (observability A3): the web app PROPOSES,
//! a trusted surface APPROVES, the daemon applies with provenance.
//!
//! Bastion's identity is "authority explicit": the web surface (bearer
//! token in a browser) must never mutate the agent's constitution directly.
//! So a change submitted from `/app` becomes a PENDING row here, visible as
//! a `config.change_requested` event on `/events`, and only the console
//! `/proposal approve <id>` (Scope::ConsoleOnly, like `/credential`)
//! applies it — with a timestamped backup next to the file it replaces.
//! Rejected or approved, the row keeps who/when/what for audit.
//!
//! v1 shipped one kind: `persona_edit` (write `personas/<slug>/SOUL.md`).
//! A4 S2 adds two more, both applied through the unified
//! [`ConfigStore`](crate::config_store::ConfigStore) with origin `"web"`:
//!
//! - `model_config`: default model (hot-swapped into the running
//!   `SharedProvider`, exactly like `/model`) and/or the fallback ladder
//!   (persisted; loaded at the NEXT startup — see [`apply`]).
//! - `secret_set`: provider API key by REFERENCE — the payload carries only
//!   `{provider_id, env_key}`; the VALUE is never written to this sqlite
//!   store. The web POST stashes it in the daemon's in-memory
//!   [`PendingSecretValues`] keyed by proposal id; console approve writes it
//!   to `BASTION_SECRETS_DIR/<ENV_KEY>` (0600) and drops it from the map. A
//!   daemon restart between propose and approve loses the value on purpose —
//!   approve then fails with "secret value expired, re-submit from the web".

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bastion_types::SecretValue;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tokio::task::spawn_blocking;

use crate::config_store::{
    fallback_models_value_json, model_value_json, ConfigStore, KEY_MODEL_FALLBACKS,
    KEY_MODEL_SELECTED,
};

pub const MAX_CONTENT_BYTES: usize = 256 * 1024;

/// Upper bound for a `secret_set` value (8 KiB): every provider API key is
/// far smaller; anything bigger is a paste mistake, refused early.
pub const MAX_SECRET_VALUE_BYTES: usize = 8 * 1024;

/// Most fallback ladders are 1-3 entries; 16 is already absurd — cap it.
pub const MAX_FALLBACK_MODELS: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalStatus {
    Pending,
    Approved,
    Rejected,
}

/// Tagged enum so kinds can join without a schema change (payload is opaque
/// JSON in the table). `secret_set` deliberately has NO value field — the
/// secret value never touches this store (see module docs).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProposalPayload {
    PersonaEdit { slug: String, content: String },
    /// A4 S2: default model and/or fallback ladder. `None` = leave that half
    /// untouched; at least one must be `Some` (guarded at create and apply).
    ModelConfig {
        default_model: Option<String>,
        fallback_models: Option<Vec<String>>,
    },
    /// A4 S2: set a provider API key by reference. `env_key` must be one of
    /// the known provider env keys (`crate::model_catalog`) and must match
    /// `provider_id` — guarded at create and again at apply.
    SecretSet {
        provider_id: String,
        env_key: String,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct Proposal {
    pub id: String,
    pub owner_id: String,
    pub origin: String,
    pub payload: ProposalPayload,
    pub status: ProposalStatus,
    pub created_at: i64,
    pub resolved_at: Option<i64>,
}

/// One path segment, no traversal — same discipline as `extension/ui.rs`.
pub fn is_safe_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= 64
        && slug
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// `^[A-Z][A-Z0-9_]{2,63}$`, hand-rolled (no regex dep needed for one
/// pattern): uppercase start, 3-64 chars of `[A-Z0-9_]`. The env key later
/// becomes a FILE NAME under `BASTION_SECRETS_DIR`, so this is also the
/// no-traversal guard (`/` and `.` are simply not in the alphabet).
pub fn is_valid_env_key(key: &str) -> bool {
    (3..=64).contains(&key.len())
        && key.as_bytes()[0].is_ascii_uppercase()
        && key
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
}

/// A4 S2: in-memory-ONLY holding pen for `secret_set` values, keyed by
/// proposal id. Values live here between the web POST and the console
/// approve, and nowhere else — never sqlite, never a log line
/// ([`SecretValue`]'s `Debug` redacts). A daemon restart empties it by
/// construction; approve then fails with the "expired" message instead of
/// applying half a proposal. Cheap to clone (an `Arc`).
#[derive(Clone, Default)]
pub struct PendingSecretValues {
    inner: Arc<tokio::sync::Mutex<HashMap<String, SecretValue>>>,
}

impl PendingSecretValues {
    /// Stash the value for a just-created proposal.
    pub async fn put(&self, proposal_id: &str, value: SecretValue) {
        self.inner
            .lock()
            .await
            .insert(proposal_id.to_string(), value);
    }

    /// Remove and return — consumed exactly once, by apply.
    pub async fn take(&self, proposal_id: &str) -> Option<SecretValue> {
        self.inner.lock().await.remove(proposal_id)
    }
}

/// Everything [`apply`] may need beyond the payload itself. Persona edits
/// use none of it; the A4 S2 kinds require `config_store` (fail typed when
/// absent, e.g. in a context that never wired it). All fields are cheap
/// clones — the daemon builds this once in `daemon_loop` and the console
/// cockpit clones it per approve with the approving owner as `actor`.
#[derive(Clone, Default)]
pub struct ApplyResources {
    /// The unified audited write path — REQUIRED for `model_config` and
    /// `secret_set` (origin `"web"`: the change was authored on the web,
    /// the console only approved it).
    pub config_store: Option<ConfigStore>,
    /// The live `SharedProvider` `/model` hot-swaps. `Some` in the daemon:
    /// an approved default model takes effect immediately, exactly like
    /// `/model`. `None` = persist only (takes effect next restart).
    pub provider: Option<bastion_providers::SharedProvider>,
    /// The holding pen the web POST filled for `secret_set` proposals.
    pub pending_secrets: PendingSecretValues,
    /// Resolved `BASTION_SECRETS_DIR` (read once at daemon start). `None` =
    /// `secret_set` approval fails typed — there is nowhere durable to put
    /// the file.
    pub secrets_dir: Option<PathBuf>,
    /// The APPROVING owner — recorded as the config-override actor.
    pub actor: Option<String>,
}

const SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS config_proposals (
    id           TEXT PRIMARY KEY,
    owner_id     TEXT NOT NULL,
    origin       TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    status       TEXT NOT NULL,
    created_at   INTEGER NOT NULL,
    resolved_at  INTEGER
);
CREATE INDEX IF NOT EXISTS idx_config_proposals_status
    ON config_proposals(status, created_at);
";

#[derive(Clone)]
pub struct SqliteProposalStore {
    db_path: String,
}

fn now_nanos() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}

fn open_conn(path: &str) -> anyhow::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")?;
    Ok(conn)
}

type ProposalRow = (String, String, String, String, String, i64, Option<i64>);

fn row_to_proposal(row: &rusqlite::Row) -> rusqlite::Result<ProposalRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
    ))
}

fn decode(
    (id, owner_id, origin, payload_json, status, created_at, resolved_at): ProposalRow,
) -> anyhow::Result<Proposal> {
    Ok(Proposal {
        id,
        owner_id,
        origin,
        payload: serde_json::from_str(&payload_json)?,
        status: match status.as_str() {
            "pending" => ProposalStatus::Pending,
            "approved" => ProposalStatus::Approved,
            "rejected" => ProposalStatus::Rejected,
            other => anyhow::bail!("unknown proposal status '{other}'"),
        },
        created_at,
        resolved_at,
    })
}

impl SqliteProposalStore {
    pub fn new(db_path: impl Into<String>) -> Self {
        Self {
            db_path: db_path.into(),
        }
    }

    pub async fn init_schema(&self) -> anyhow::Result<()> {
        let path = self.db_path.clone();
        spawn_blocking(move || {
            open_conn(&path)?.execute_batch(SCHEMA_SQL)?;
            Ok::<_, anyhow::Error>(())
        })
        .await?
    }

    pub async fn create(
        &self,
        owner: &str,
        origin: &str,
        payload: &ProposalPayload,
    ) -> anyhow::Result<Proposal> {
        let path = self.db_path.clone();
        let proposal = Proposal {
            id: format!("prop-{:016x}", rand_u64()),
            owner_id: owner.to_string(),
            origin: origin.to_string(),
            payload: payload.clone(),
            status: ProposalStatus::Pending,
            created_at: now_nanos(),
            resolved_at: None,
        };
        let row = (
            proposal.id.clone(),
            proposal.owner_id.clone(),
            proposal.origin.clone(),
            serde_json::to_string(&proposal.payload)?,
            proposal.created_at,
        );
        spawn_blocking(move || {
            open_conn(&path)?.execute(
                "INSERT INTO config_proposals
                    (id, owner_id, origin, payload_json, status, created_at, resolved_at)
                 VALUES (?1, ?2, ?3, ?4, 'pending', ?5, NULL)",
                rusqlite::params![row.0, row.1, row.2, row.3, row.4],
            )?;
            Ok::<_, anyhow::Error>(())
        })
        .await??;
        Ok(proposal)
    }

    pub async fn list_for_owner(&self, owner: &str) -> anyhow::Result<Vec<Proposal>> {
        let path = self.db_path.clone();
        let owner = owner.to_string();
        let rows = spawn_blocking(move || {
            let conn = open_conn(&path)?;
            let mut stmt = conn.prepare(
                "SELECT id, owner_id, origin, payload_json, status, created_at, resolved_at
                 FROM config_proposals WHERE owner_id = ?1
                 ORDER BY created_at DESC LIMIT 100",
            )?;
            let rows = stmt
                .query_map([owner], row_to_proposal)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok::<_, anyhow::Error>(rows)
        })
        .await??;
        rows.into_iter().map(decode).collect()
    }

    pub async fn get(&self, owner: &str, id: &str) -> anyhow::Result<Option<Proposal>> {
        let path = self.db_path.clone();
        let owner = owner.to_string();
        let id = id.to_string();
        let row = spawn_blocking(move || {
            let conn = open_conn(&path)?;
            let mut stmt = conn.prepare(
                "SELECT id, owner_id, origin, payload_json, status, created_at, resolved_at
                 FROM config_proposals WHERE owner_id = ?1 AND id = ?2",
            )?;
            let row = stmt
                .query_map(rusqlite::params![owner, id], row_to_proposal)?
                .next()
                .transpose()?;
            Ok::<_, anyhow::Error>(row)
        })
        .await??;
        row.map(decode).transpose()
    }

    /// Flip a PENDING proposal to approved/rejected. Fails (returns false)
    /// if the row is not pending — approvals are once-only.
    pub async fn resolve(
        &self,
        owner: &str,
        id: &str,
        status: ProposalStatus,
    ) -> anyhow::Result<bool> {
        let path = self.db_path.clone();
        let owner = owner.to_string();
        let id = id.to_string();
        let status_str = match status {
            ProposalStatus::Approved => "approved",
            ProposalStatus::Rejected => "rejected",
            ProposalStatus::Pending => anyhow::bail!("cannot resolve back to pending"),
        };
        let now = now_nanos();
        let changed = spawn_blocking(move || {
            let conn = open_conn(&path)?;
            let n = conn.execute(
                "UPDATE config_proposals SET status = ?1, resolved_at = ?2
                 WHERE owner_id = ?3 AND id = ?4 AND status = 'pending'",
                rusqlite::params![status_str, now, owner, id],
            )?;
            Ok::<_, anyhow::Error>(n)
        })
        .await??;
        Ok(changed == 1)
    }
}

fn rand_u64() -> u64 {
    // Uniqueness, not secrecy: ids are per-owner rows, not credentials.
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let addr = &nanos as *const _ as u64;
    nanos.wrapping_mul(0x9e3779b97f4a7c15) ^ addr.rotate_left(17)
}

/// Apply an APPROVED proposal. Returns a human line for the console.
/// Persona edits back up the previous SOUL.md beside itself before writing;
/// the A4 S2 kinds write through the unified [`ConfigStore`] (origin
/// `"web"`, actor = the approving owner from `res`). `proposal_id` keys the
/// [`PendingSecretValues`] lookup for `secret_set` and is unused otherwise.
pub async fn apply(
    root: &Path,
    proposal_id: &str,
    payload: &ProposalPayload,
    res: &ApplyResources,
) -> anyhow::Result<String> {
    match payload {
        ProposalPayload::PersonaEdit { slug, content } => {
            if !is_safe_slug(slug) {
                anyhow::bail!("unsafe persona slug '{slug}'");
            }
            if content.len() > MAX_CONTENT_BYTES {
                anyhow::bail!("persona content exceeds {MAX_CONTENT_BYTES} bytes");
            }
            let dir = root.join("personas").join(slug);
            let soul = dir.join("SOUL.md");
            tokio::fs::create_dir_all(&dir).await?;
            let mut backed_up = false;
            if tokio::fs::try_exists(&soul).await.unwrap_or(false) {
                let backup = dir.join(format!("SOUL.md.bak-{}", now_nanos()));
                tokio::fs::copy(&soul, &backup).await?;
                backed_up = true;
            }
            tokio::fs::write(&soul, content).await?;
            Ok(format!(
                "personas/{slug}/SOUL.md written{}. Restart the daemon (the \
                 turn responder loads personas at boot) for it to take effect.",
                if backed_up {
                    " (previous version backed up beside it)"
                } else {
                    ""
                }
            ))
        }
        ProposalPayload::ModelConfig {
            default_model,
            fallback_models,
        } => apply_model_config(default_model.as_deref(), fallback_models.as_deref(), res).await,
        ProposalPayload::SecretSet {
            provider_id,
            env_key,
        } => apply_secret_set(proposal_id, provider_id, env_key, res).await,
    }
}

async fn apply_model_config(
    default_model: Option<&str>,
    fallback_models: Option<&[String]>,
    res: &ApplyResources,
) -> anyhow::Result<String> {
    let store = res.config_store.as_ref().ok_or_else(|| {
        anyhow::anyhow!("no config store in this apply context — model_config cannot be applied")
    })?;
    if default_model.is_none() && fallback_models.is_none() {
        anyhow::bail!("empty model_config proposal — nothing to apply");
    }
    let actor = res.actor.as_deref();
    let mut lines = Vec::new();

    if let Some(model) = default_model {
        let model = model.trim();
        if model.is_empty() {
            anyhow::bail!("model_config.default_model must not be empty");
        }
        // Fail-closed connectivity guard BEFORE resolve_provider: some
        // bastion-core provider constructors panic on a missing/empty env
        // key (`ANTHROPIC_API_KEY required`), which must never take the
        // daemon down from an approve. NOTE this checks the ENV VAR only —
        // providers read env directly at construction, so a key that exists
        // solely as a BASTION_SECRETS_DIR file (visible as connected on
        // GET /providers) is not enough to hot-swap yet.
        // TODO(A4 seam): route provider construction through the daemon's
        // SecretResolver in bastion-core so secrets-dir keys work here too.
        let kind = bastion_providers::registry::resolve_provider_kind(model);
        if let Some(env_key) = crate::model_catalog::env_key_for_provider(kind) {
            let present = std::env::var(env_key).is_ok_and(|v| !v.is_empty());
            if !present {
                anyhow::bail!(
                    "provider '{kind}' is not connected in this process ({env_key} is not set \
                     in the daemon's environment) — set it and re-approve, or pick another model"
                );
            }
        }
        // Same order as `/model` (`switch_model`): resolve (validates the
        // id), persist, then swap the live provider between turns.
        let new_provider = bastion_providers::registry::resolve_provider(model)?;
        store
            .apply(KEY_MODEL_SELECTED, &model_value_json(model), "web", actor)
            .await?;
        if let Some(provider) = &res.provider {
            *provider.write().await = new_provider;
            tracing::info!(event = "provider_swapped", model = %model, origin = "web");
            lines.push(format!("default model switched to {model} (live)"));
        } else {
            lines.push(format!(
                "default model set to {model}; no live provider handle here — takes effect next restart"
            ));
        }
    }

    if let Some(fallbacks) = fallback_models {
        if fallbacks.len() > MAX_FALLBACK_MODELS {
            anyhow::bail!("more than {MAX_FALLBACK_MODELS} fallback models");
        }
        if fallbacks.iter().any(|m| m.trim().is_empty()) {
            anyhow::bail!("model_config.fallback_models must not contain empty ids");
        }
        store
            .apply(
                KEY_MODEL_FALLBACKS,
                &fallback_models_value_json(fallbacks),
                "web",
                actor,
            )
            .await?;
        // TODO(A4 seam): the running AgentLoop's fallback ladder is passed
        // at construction (main.rs `AgentLoop::new`) with no runtime setter
        // — hot-swapping it needs a kernel seam in bastion-core. Persisted
        // here; the startup loader reads KEY_MODEL_FALLBACKS.
        lines.push(format!(
            "{} fallback model(s) persisted; the ladder is loaded at startup, so it takes \
             effect on the next restart",
            fallbacks.len()
        ));
    }

    Ok(lines.join(" | "))
}

async fn apply_secret_set(
    proposal_id: &str,
    provider_id: &str,
    env_key: &str,
    res: &ApplyResources,
) -> anyhow::Result<String> {
    let store = res.config_store.as_ref().ok_or_else(|| {
        anyhow::anyhow!("no config store in this apply context — secret_set cannot be applied")
    })?;
    // Re-validate at apply even though create already did (defense in depth
    // — a row written by any other path must meet the same bar).
    if !is_valid_env_key(env_key) {
        anyhow::bail!("invalid env key '{env_key}' (must match ^[A-Z][A-Z0-9_]{{2,63}}$)");
    }
    match crate::model_catalog::env_key_for_provider(provider_id) {
        Some(expected) if expected == env_key => {}
        Some(expected) => {
            anyhow::bail!("provider '{provider_id}' uses {expected}, not '{env_key}'")
        }
        None => anyhow::bail!("unknown provider '{provider_id}' — not an API-key provider"),
    }
    let Some(dir) = res.secrets_dir.as_deref() else {
        anyhow::bail!(
            "BASTION_SECRETS_DIR is not set — there is nowhere durable to write the secret \
             file. Set it, restart, and re-submit from the web."
        );
    };
    let Some(value) = res.pending_secrets.take(proposal_id).await else {
        anyhow::bail!(
            "secret value expired (the daemon restarted since the web submitted it, or it \
             was already consumed) — re-submit from the web"
        );
    };
    write_secret_file(dir, env_key, &value).await?;
    // Audit marker ONLY — the value itself never reaches the config store.
    store
        .apply(
            &format!("secret.set:{env_key}"),
            r#"{"set":true}"#,
            "web",
            res.actor.as_deref(),
        )
        .await?;
    Ok(format!(
        "{env_key} written to the secrets dir (mode 0600) for provider '{provider_id}'. \
         GET /providers now reports it connected via secrets_dir; providers read this env \
         key from the environment at construction, so restart (or export it) for API calls \
         to pick it up."
    ))
}

/// `<dir>/<env_key>`, created 0600 (and re-tightened on overwrite — `mode`
/// only applies at creation). The exact file shape
/// `secret::MountedFileSecretResolver` reads back.
async fn write_secret_file(dir: &Path, env_key: &str, value: &SecretValue) -> anyhow::Result<()> {
    use tokio::io::AsyncWriteExt;
    tokio::fs::create_dir_all(dir).await?;
    let path = dir.join(env_key);
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&path).await?;
    file.write_all(value.expose_secret().as_bytes()).await?;
    file.flush().await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).await?;
    }
    Ok(())
}

/// List persona slugs by reading the personas directory — the editor needs
/// dir slugs (file identity), which are not the frontmatter display names
/// `/loadout` reports.
pub async fn list_persona_slugs(root: &Path) -> Vec<String> {
    let mut slugs = Vec::new();
    let Ok(mut rd) = tokio::fs::read_dir(root.join("personas")).await else {
        return slugs;
    };
    while let Ok(Some(entry)) = rd.next_entry().await {
        let p = entry.path();
        if p.is_dir() && p.join("SOUL.md").is_file() {
            if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                slugs.push(name.to_string());
            }
        }
    }
    slugs.sort();
    slugs
}

pub async fn read_persona(root: &Path, slug: &str) -> anyhow::Result<Option<String>> {
    if !is_safe_slug(slug) {
        anyhow::bail!("unsafe persona slug '{slug}'");
    }
    let soul = root.join("personas").join(slug).join("SOUL.md");
    match tokio::fs::read_to_string(&soul).await {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// The personas root — the daemon loads from `./personas` (see `main.rs`
/// `PersonaRegistry::load_dir(".")`), so the root is the working directory.
pub fn personas_root() -> PathBuf {
    PathBuf::from(".")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_rejects_traversal_and_separators() {
        assert!(is_safe_slug("ada"));
        assert!(is_safe_slug("code-reviewer_2"));
        assert!(!is_safe_slug(""));
        assert!(!is_safe_slug(".."));
        assert!(!is_safe_slug("a/b"));
        assert!(!is_safe_slug("a\\b"));
        assert!(!is_safe_slug(&"x".repeat(65)));
    }

    async fn store() -> (tempfile::NamedTempFile, SqliteProposalStore) {
        let f = tempfile::NamedTempFile::new().unwrap();
        let s = SqliteProposalStore::new(f.path().to_str().unwrap().to_owned());
        s.init_schema().await.unwrap();
        (f, s)
    }

    fn edit(slug: &str) -> ProposalPayload {
        ProposalPayload::PersonaEdit {
            slug: slug.into(),
            content: "---\nname: ada\n---\nbe rigorous".into(),
        }
    }

    #[tokio::test]
    async fn create_list_resolve_roundtrip_and_once_only_approval() {
        let (_f, s) = store().await;
        let p = s.create("alice", "web", &edit("ada")).await.unwrap();
        assert_eq!(p.status, ProposalStatus::Pending);

        let listed = s.list_for_owner("alice").await.unwrap();
        assert_eq!(listed.len(), 1);
        assert!(s.list_for_owner("mallory").await.unwrap().is_empty());

        assert!(s
            .resolve("alice", &p.id, ProposalStatus::Approved)
            .await
            .unwrap());
        // once-only: a second resolve must not flip anything
        assert!(!s
            .resolve("alice", &p.id, ProposalStatus::Rejected)
            .await
            .unwrap());
        let after = s.get("alice", &p.id).await.unwrap().unwrap();
        assert_eq!(after.status, ProposalStatus::Approved);
    }

    #[tokio::test]
    async fn resolve_is_owner_scoped() {
        let (_f, s) = store().await;
        let p = s.create("alice", "web", &edit("ada")).await.unwrap();
        assert!(!s
            .resolve("mallory", &p.id, ProposalStatus::Approved)
            .await
            .unwrap());
    }

    /// Test shorthand: apply with an empty resource bag (persona edits and
    /// guard failures need none of it).
    async fn apply_bare(root: &Path, payload: &ProposalPayload) -> anyhow::Result<String> {
        apply(root, "prop-test", payload, &ApplyResources::default()).await
    }

    #[tokio::test]
    async fn apply_writes_soul_and_backs_up_previous() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let payload = edit("ada");
        let msg = apply_bare(root, &payload).await.unwrap();
        assert!(msg.contains("personas/ada/SOUL.md written"));
        let soul = root.join("personas/ada/SOUL.md");
        assert!(soul.is_file());

        // second apply backs up the first
        apply_bare(root, &payload).await.unwrap();
        let backups: Vec<_> = std::fs::read_dir(root.join("personas/ada"))
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with("SOUL.md.bak-"))
            .collect();
        assert_eq!(backups.len(), 1);

        assert_eq!(list_persona_slugs(root).await, vec!["ada".to_string()]);
        assert!(read_persona(root, "ada").await.unwrap().is_some());
        assert!(read_persona(root, "ghost").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn apply_rejects_traversal_slug() {
        let dir = tempfile::tempdir().unwrap();
        let payload = ProposalPayload::PersonaEdit {
            slug: "../escape".into(),
            content: "x".into(),
        };
        assert!(apply_bare(dir.path(), &payload).await.is_err());
    }

    // ---- A4 S2: model_config / secret_set ------------------------------

    #[test]
    fn payload_serde_uses_snake_case_kind_tags() {
        let model = serde_json::to_value(ProposalPayload::ModelConfig {
            default_model: Some("llama3.2".into()),
            fallback_models: None,
        })
        .unwrap();
        assert_eq!(model["kind"], "model_config");
        assert_eq!(model["default_model"], "llama3.2");

        let secret = serde_json::to_value(ProposalPayload::SecretSet {
            provider_id: "gemini".into(),
            env_key: "GEMINI_API_KEY".into(),
        })
        .unwrap();
        assert_eq!(secret["kind"], "secret_set");
        // The whole point: a serialized secret_set payload has NO value field.
        assert!(secret.get("value").is_none());

        let parsed: ProposalPayload = serde_json::from_value(secret).unwrap();
        assert!(matches!(parsed, ProposalPayload::SecretSet { .. }));
    }

    #[test]
    fn env_key_grammar_is_strict() {
        assert!(is_valid_env_key("GEMINI_API_KEY"));
        assert!(is_valid_env_key("KEY"));
        assert!(!is_valid_env_key("AB")); // too short
        assert!(!is_valid_env_key("1KEY")); // must start uppercase
        assert!(!is_valid_env_key("_KEY"));
        assert!(!is_valid_env_key("gemini_api_key")); // lowercase
        assert!(!is_valid_env_key("KEY WITH SPACE"));
        assert!(!is_valid_env_key("../ESCAPE"));
        assert!(!is_valid_env_key(&"K".repeat(65)));
    }

    async fn resources_with_store() -> (tempfile::NamedTempFile, ApplyResources) {
        let f = tempfile::NamedTempFile::new().unwrap();
        let store = crate::config_store::ConfigStore::new(f.path().to_str().unwrap().to_owned());
        store.init_schema().await.unwrap();
        (
            f,
            ApplyResources {
                config_store: Some(store),
                actor: Some("alice".into()),
                ..Default::default()
            },
        )
    }

    #[tokio::test]
    async fn model_config_apply_requires_a_config_store() {
        let dir = tempfile::tempdir().unwrap();
        let payload = ProposalPayload::ModelConfig {
            default_model: Some("llama3.2".into()),
            fallback_models: None,
        };
        let err = apply_bare(dir.path(), &payload).await.unwrap_err();
        assert!(err.to_string().contains("no config store"));
    }

    #[tokio::test]
    async fn model_config_apply_persists_with_origin_web_and_reports_restart_for_fallbacks() {
        let dir = tempfile::tempdir().unwrap();
        let (_f, res) = resources_with_store().await;
        // "llama3.2" routes to ollama — no API-key guard, no live provider
        // in `res`, so the apply persists without constructing anything.
        let payload = ProposalPayload::ModelConfig {
            default_model: Some("llama3.2".into()),
            fallback_models: Some(vec!["mistral".into()]),
        };
        let msg = apply(dir.path(), "prop-x", &payload, &res).await.unwrap();
        assert!(msg.contains("next restart"), "fallbacks are startup-loaded: {msg}");

        let store = res.config_store.as_ref().unwrap();
        let selected = store.latest(KEY_MODEL_SELECTED).await.unwrap().unwrap();
        assert_eq!(
            crate::config_store::model_from_value_json(&selected).as_deref(),
            Some("llama3.2")
        );
        let fallbacks = store.latest(KEY_MODEL_FALLBACKS).await.unwrap().unwrap();
        assert_eq!(
            crate::config_store::fallback_models_from_value_json(&fallbacks),
            Some(vec!["mistral".to_string()])
        );
        let history = store.history(KEY_MODEL_SELECTED, 1).await.unwrap();
        assert_eq!(history[0].origin, "web");
        assert_eq!(history[0].actor.as_deref(), Some("alice"));
    }

    // `std::env` is process-global — serialize the one test that touches a
    // provider key and save/restore its prior value (same discipline as
    // `secret.rs`'s ENV_LOCK).
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[tokio::test]
    async fn model_config_apply_guards_empty_and_disconnected() {
        let dir = tempfile::tempdir().unwrap();
        let (_f, res) = resources_with_store().await;

        let empty = ProposalPayload::ModelConfig {
            default_model: None,
            fallback_models: None,
        };
        assert!(apply(dir.path(), "p", &empty, &res).await.is_err());

        let blank = ProposalPayload::ModelConfig {
            default_model: Some("   ".into()),
            fallback_models: None,
        };
        assert!(apply(dir.path(), "p", &blank, &res).await.is_err());

        let blank_fallback = ProposalPayload::ModelConfig {
            default_model: None,
            fallback_models: Some(vec!["ok-model".into(), " ".into()]),
        };
        assert!(apply(dir.path(), "p", &blank_fallback, &res).await.is_err());

        // Disconnected api-key provider: a typed bail, NEVER the provider
        // constructor's `ANTHROPIC_API_KEY required` panic.
        let saved = {
            let _guard = ENV_LOCK.lock().unwrap();
            let saved = std::env::var("ANTHROPIC_API_KEY").ok();
            std::env::remove_var("ANTHROPIC_API_KEY");
            saved
        };
        let claude = ProposalPayload::ModelConfig {
            default_model: Some("claude-sonnet-4-5".into()),
            fallback_models: None,
        };
        let result = apply(dir.path(), "p", &claude, &res).await;
        {
            let _guard = ENV_LOCK.lock().unwrap();
            if let Some(prev) = saved {
                std::env::set_var("ANTHROPIC_API_KEY", prev);
            }
        }
        let err = result.unwrap_err();
        assert!(err.to_string().contains("not connected"), "{err}");
        // Nothing was persisted for the refused switch.
        let store = res.config_store.as_ref().unwrap();
        assert!(store.latest(KEY_MODEL_SELECTED).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn secret_set_apply_rejects_unknown_provider_and_key_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let (_f, mut res) = resources_with_store().await;
        res.secrets_dir = Some(dir.path().to_path_buf());

        let unknown = ProposalPayload::SecretSet {
            provider_id: "notaprovider".into(),
            env_key: "GEMINI_API_KEY".into(),
        };
        assert!(apply(dir.path(), "p", &unknown, &res).await.is_err());

        let mismatch = ProposalPayload::SecretSet {
            provider_id: "gemini".into(),
            env_key: "OPENAI_API_KEY".into(),
        };
        assert!(apply(dir.path(), "p", &mismatch, &res).await.is_err());

        let malformed = ProposalPayload::SecretSet {
            provider_id: "gemini".into(),
            env_key: "../escape".into(),
        };
        assert!(apply(dir.path(), "p", &malformed, &res).await.is_err());
    }

    #[tokio::test]
    async fn secret_set_apply_fails_expired_when_the_value_is_gone() {
        let dir = tempfile::tempdir().unwrap();
        let (_f, mut res) = resources_with_store().await;
        res.secrets_dir = Some(dir.path().to_path_buf());
        let payload = ProposalPayload::SecretSet {
            provider_id: "gemini".into(),
            env_key: "GEMINI_API_KEY".into(),
        };
        // Nothing was ever put in the pending map — same as a daemon restart.
        let err = apply(dir.path(), "prop-lost", &payload, &res)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("re-submit from the web"));
    }

    #[tokio::test]
    async fn secret_set_apply_writes_0600_consumes_the_value_and_audits_a_boolean() {
        let personas = tempfile::tempdir().unwrap();
        let secrets = tempfile::tempdir().unwrap();
        let (_f, mut res) = resources_with_store().await;
        res.secrets_dir = Some(secrets.path().to_path_buf());
        res.pending_secrets
            .put("prop-ok", SecretValue::new("sk-test-123"))
            .await;

        let payload = ProposalPayload::SecretSet {
            provider_id: "gemini".into(),
            env_key: "GEMINI_API_KEY".into(),
        };
        let msg = apply(personas.path(), "prop-ok", &payload, &res)
            .await
            .unwrap();
        assert!(msg.contains("GEMINI_API_KEY"));
        assert!(!msg.contains("sk-test-123"), "value must never be echoed");

        let file = secrets.path().join("GEMINI_API_KEY");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "sk-test-123");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&file).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        // Consumed exactly once.
        assert!(res.pending_secrets.take("prop-ok").await.is_none());

        // Audit row is a boolean marker, never the value.
        let store = res.config_store.as_ref().unwrap();
        let audit = store
            .latest("secret.set:GEMINI_API_KEY")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(audit, r#"{"set":true}"#);
        let history = store.history("secret.set:GEMINI_API_KEY", 1).await.unwrap();
        assert_eq!(history[0].origin, "web");
    }

    #[tokio::test]
    async fn secret_set_apply_fails_without_a_secrets_dir() {
        let dir = tempfile::tempdir().unwrap();
        let (_f, res) = resources_with_store().await;
        res.pending_secrets
            .put("prop-nodir", SecretValue::new("sk-test"))
            .await;
        let payload = ProposalPayload::SecretSet {
            provider_id: "gemini".into(),
            env_key: "GEMINI_API_KEY".into(),
        };
        let err = apply(dir.path(), "prop-nodir", &payload, &res)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("BASTION_SECRETS_DIR"));
        // The dir check runs before the take: the value stays pending, in
        // memory only, and dies with the process.
        assert!(res.pending_secrets.take("prop-nodir").await.is_some());
    }
}
