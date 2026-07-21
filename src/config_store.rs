//! Unified runtime config-override store (A4-U slice S1): ONE write path for
//! every runtime configuration mutation, regardless of origin (console
//! command, web proposal apply, channel), with an append-only audit trail and
//! live SSE propagation.
//!
//! Before this module, config mutations were fragmented: `/model` wrote
//! `.bastion/model-selection.json`, `/backend` wrote
//! `.bastion/backend-selection.json`, and nothing else ever heard about the
//! change. Now both funnel through [`ConfigStore::apply`], which appends a
//! row to the `config_overrides` SQLite table (same DB the proposal store
//! uses) and broadcasts a `config.applied` event on the daemon's `/events`
//! stream. `bastion.toml` stays the declarative base; the latest row per key
//! overlays it exactly like the legacy `.json` files did.
//!
//! The table is append-only on purpose: the CURRENT value of a key is simply
//! the latest row for that key, and every previous row is the audit history
//! (`who changed what, from where, when`). "Clearing" an override (e.g.
//! `/model reset`) is therefore itself an append — an empty value the
//! startup loader treats as "no override", never a DELETE.
//!
//! Keys v1: [`KEY_MODEL_SELECTED`] and [`KEY_BACKEND_SELECTED`]. `value_json`
//! holds the exact JSON shape the legacy files held (`{"model": "..."}` and
//! the `BackendSelection` object), so migration is a verbatim import.
//!
//! Seam for S2: proposal apply (`proposals::apply` / the console `/proposal`
//! cockpit) does not write through this store yet — when proposal kinds grow
//! beyond persona edits into model/backend/channel config, their apply step
//! should call [`ConfigStore::apply`] with origin `"web"`.

use std::path::Path;

use rusqlite::Connection;
use serde::Serialize;
use tokio::sync::broadcast;
use tokio::task::spawn_blocking;

/// Runtime-selected provider/model (`/model`). Value shape:
/// `{"model": "<id>"}` — an empty `model` means "override cleared".
pub const KEY_MODEL_SELECTED: &str = "model.selected";

/// Runtime-selected conversation backend (`/backend`). Value shape: the
/// `crate::config::BackendSelection` JSON object.
pub const KEY_BACKEND_SELECTED: &str = "backend.selected";

const SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS config_overrides (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    key        TEXT NOT NULL,
    value_json TEXT NOT NULL,
    origin     TEXT NOT NULL,
    actor      TEXT,
    applied_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_config_overrides_key
    ON config_overrides(key, id);
";

/// One audit row. `applied_at` is unix seconds; `origin` is one of
/// `console | web | channel | migration`.
#[derive(Debug, Clone, Serialize)]
pub struct ConfigOverride {
    pub key: String,
    pub value_json: String,
    pub origin: String,
    pub actor: Option<String>,
    pub applied_at: i64,
}

/// Handle to the `config_overrides` table. Cheap to clone (a path string
/// plus an optional broadcast sender) — mirrors `SqliteProposalStore`'s
/// connection-per-call pattern over the same session DB file.
#[derive(Clone)]
pub struct ConfigStore {
    db_path: String,
    /// The daemon's `/events` SSE broadcast channel. `None` for the brief
    /// startup window before `main()` creates the channel (migration and the
    /// startup reads run then — nothing is subscribed yet anyway) and in
    /// unit tests that don't care about propagation.
    events_tx: Option<broadcast::Sender<String>>,
}

fn now_secs() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn open_conn(path: &str) -> anyhow::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")?;
    Ok(conn)
}

fn row_to_override(row: &rusqlite::Row) -> rusqlite::Result<ConfigOverride> {
    Ok(ConfigOverride {
        key: row.get(0)?,
        value_json: row.get(1)?,
        origin: row.get(2)?,
        actor: row.get(3)?,
        applied_at: row.get(4)?,
    })
}

impl ConfigStore {
    pub fn new(db_path: impl Into<String>) -> Self {
        Self {
            db_path: db_path.into(),
            events_tx: None,
        }
    }

    /// Attach the daemon's SSE broadcast channel: every successful
    /// [`apply`](Self::apply) from this handle on emits a `config.applied`
    /// event on it.
    pub fn with_events(mut self, events_tx: broadcast::Sender<String>) -> Self {
        self.events_tx = Some(events_tx);
        self
    }

    pub async fn init_schema(&self) -> anyhow::Result<()> {
        let path = self.db_path.clone();
        spawn_blocking(move || {
            open_conn(&path)?.execute_batch(SCHEMA_SQL)?;
            Ok::<_, anyhow::Error>(())
        })
        .await?
    }

    /// THE single write path: append one override row, then broadcast
    /// `config.applied` (fire-and-forget — a full or subscriber-less channel
    /// never fails the write that already committed).
    pub async fn apply(
        &self,
        key: &str,
        value_json: &str,
        origin: &str,
        actor: Option<&str>,
    ) -> anyhow::Result<()> {
        let path = self.db_path.clone();
        let applied_at = now_secs();
        let row = (
            key.to_string(),
            value_json.to_string(),
            origin.to_string(),
            actor.map(str::to_string),
        );
        spawn_blocking(move || {
            open_conn(&path)?.execute(
                "INSERT INTO config_overrides (key, value_json, origin, actor, applied_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![row.0, row.1, row.2, row.3, applied_at],
            )?;
            Ok::<_, anyhow::Error>(())
        })
        .await??;
        tracing::info!(
            event = "config_override_applied",
            key = %key,
            origin = %origin,
        );
        if let Some(tx) = &self.events_tx {
            let _ = tx.send(
                serde_json::json!({
                    "type": "config.applied",
                    "key": key,
                    "origin": origin,
                    "applied_at": applied_at,
                })
                .to_string(),
            );
        }
        Ok(())
    }

    /// Current value of a key = latest row for that key.
    pub async fn latest(&self, key: &str) -> anyhow::Result<Option<String>> {
        let path = self.db_path.clone();
        let key = key.to_string();
        let value = spawn_blocking(move || {
            let conn = open_conn(&path)?;
            let mut stmt = conn.prepare(
                "SELECT value_json FROM config_overrides
                 WHERE key = ?1 ORDER BY id DESC LIMIT 1",
            )?;
            let mut rows = stmt.query_map([key], |row| row.get::<_, String>(0))?;
            let value = rows.next().transpose()?;
            Ok::<_, anyhow::Error>(value)
        })
        .await??;
        Ok(value)
    }

    /// Latest row per key — the effective overlay `GET /config/overrides`
    /// reports.
    pub async fn all_latest(&self) -> anyhow::Result<Vec<ConfigOverride>> {
        let path = self.db_path.clone();
        let rows = spawn_blocking(move || {
            let conn = open_conn(&path)?;
            let mut stmt = conn.prepare(
                "SELECT key, value_json, origin, actor, applied_at FROM config_overrides
                 WHERE id IN (SELECT MAX(id) FROM config_overrides GROUP BY key)
                 ORDER BY key",
            )?;
            let rows = stmt
                .query_map([], row_to_override)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok::<_, anyhow::Error>(rows)
        })
        .await??;
        Ok(rows)
    }

    /// Audit history for one key, newest first.
    pub async fn history(&self, key: &str, limit: u32) -> anyhow::Result<Vec<ConfigOverride>> {
        let path = self.db_path.clone();
        let key = key.to_string();
        let rows = spawn_blocking(move || {
            let conn = open_conn(&path)?;
            let mut stmt = conn.prepare(
                "SELECT key, value_json, origin, actor, applied_at FROM config_overrides
                 WHERE key = ?1 ORDER BY id DESC LIMIT ?2",
            )?;
            let rows = stmt
                .query_map(rusqlite::params![key, limit], row_to_override)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok::<_, anyhow::Error>(rows)
        })
        .await??;
        Ok(rows)
    }

    /// One-time startup migration of a legacy selection file
    /// (`.bastion/model-selection.json` / `backend-selection.json`): if the
    /// file exists AND the store has no row for `key`, import its content
    /// verbatim (normalized JSON) with origin `migration`, then rename the
    /// file with an `.imported` suffix so it never re-imports and the legacy
    /// writers are clearly retired. Returns `true` when an import happened.
    ///
    /// A corrupt legacy file is skipped with a warning instead of failing
    /// startup — the exact tolerance the legacy `load_*_selection` loaders
    /// had (they returned `None` on any parse error).
    pub async fn migrate_legacy_file(&self, key: &str, legacy_path: &Path) -> anyhow::Result<bool> {
        let raw = match tokio::fs::read_to_string(legacy_path).await {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e.into()),
        };
        if self.latest(key).await?.is_some() {
            // The store is already authoritative for this key; the stale
            // legacy file is ignored forever (it would never be re-imported).
            return Ok(false);
        }
        let value: serde_json::Value = match serde_json::from_str(&raw) {
            Ok(value) => value,
            Err(e) => {
                tracing::warn!(
                    event = "config_migration_skipped",
                    key = %key,
                    path = %legacy_path.display(),
                    error = %e,
                    "legacy selection file is not valid JSON — skipping import",
                );
                return Ok(false);
            }
        };
        let value_json = value.to_string();
        self.apply(key, &value_json, "migration", None).await?;
        let mut imported = legacy_path.as_os_str().to_owned();
        imported.push(".imported");
        tokio::fs::rename(legacy_path, std::path::PathBuf::from(imported)).await?;
        tracing::info!(
            event = "config_migration_imported",
            key = %key,
            path = %legacy_path.display(),
        );
        Ok(true)
    }
}

/// Serialize a `/model` choice into the `model.selected` value shape —
/// byte-compatible with the legacy `model-selection.json` content. An empty
/// `model` is the "override cleared" sentinel `/model reset` appends.
pub fn model_value_json(model: &str) -> String {
    serde_json::json!({ "model": model }).to_string()
}

/// Parse a `model.selected` value; empty/whitespace models (the cleared
/// sentinel) come back as `None`, matching the legacy loader's filter.
pub fn model_from_value_json(raw: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    let model = value.get("model")?.as_str()?;
    (!model.trim().is_empty()).then(|| model.to_string())
}

/// Parse a `backend.selected` value into the same `BackendSelection` shape
/// the legacy `backend-selection.json` held.
pub fn backend_selection_from_value_json(raw: &str) -> Option<crate::config::BackendSelection> {
    serde_json::from_str(raw).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn store() -> (tempfile::NamedTempFile, ConfigStore) {
        let f = tempfile::NamedTempFile::new().unwrap();
        let s = ConfigStore::new(f.path().to_str().unwrap().to_owned());
        s.init_schema().await.unwrap();
        (f, s)
    }

    /// Test shorthand for the one write path.
    async fn put(s: &ConfigStore, key: &str, value: &str, origin: &str) {
        s.apply(key, value, origin, None).await.unwrap();
    }

    #[tokio::test]
    async fn apply_then_latest_returns_last_write() {
        let (_f, s) = store().await;
        assert!(s.latest(KEY_MODEL_SELECTED).await.unwrap().is_none());

        put(&s, KEY_MODEL_SELECTED, &model_value_json("m-a"), "console").await;
        let b = model_value_json("m-b");
        let k = KEY_MODEL_SELECTED;
        s.apply(k, &b, "console", Some("_local")).await.unwrap();

        let latest = s.latest(KEY_MODEL_SELECTED).await.unwrap().unwrap();
        assert_eq!(model_from_value_json(&latest).as_deref(), Some("m-b"));
    }

    #[tokio::test]
    async fn all_latest_reports_one_row_per_key_and_history_keeps_audit() {
        let (_f, s) = store().await;
        put(&s, KEY_MODEL_SELECTED, &model_value_json("m1"), "console").await;
        put(&s, KEY_MODEL_SELECTED, &model_value_json("m2"), "web").await;
        let backend = r#"{"conversation":"model","auth":null,"task_runtime":null}"#;
        put(&s, KEY_BACKEND_SELECTED, backend, "console").await;

        let all = s.all_latest().await.unwrap();
        assert_eq!(all.len(), 2, "one effective row per key");
        let model_row = all.iter().find(|o| o.key == KEY_MODEL_SELECTED).unwrap();
        assert_eq!(model_row.origin, "web");
        assert_eq!(
            model_from_value_json(&model_row.value_json).as_deref(),
            Some("m2")
        );

        // Append-only audit: history keeps every write, newest first.
        let history = s.history(KEY_MODEL_SELECTED, 10).await.unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].origin, "web");
        assert_eq!(history[1].origin, "console");
        assert_eq!(s.history(KEY_MODEL_SELECTED, 1).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn apply_broadcasts_config_applied_event() {
        let f = tempfile::NamedTempFile::new().unwrap();
        let (tx, mut rx) = broadcast::channel(8);
        let s = ConfigStore::new(f.path().to_str().unwrap().to_owned()).with_events(tx);
        s.init_schema().await.unwrap();

        put(&s, KEY_MODEL_SELECTED, &model_value_json("m1"), "console").await;

        let event: serde_json::Value = serde_json::from_str(&rx.recv().await.unwrap()).unwrap();
        assert_eq!(event["type"], "config.applied");
        assert_eq!(event["key"], KEY_MODEL_SELECTED);
        assert_eq!(event["origin"], "console");
        assert!(event["applied_at"].as_i64().unwrap() > 0);
    }

    #[tokio::test]
    async fn migration_imports_once_and_retires_the_legacy_file() {
        let (_f, s) = store().await;
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join("model-selection.json");
        std::fs::write(&legacy, r#"{ "model": "legacy-model" }"#).unwrap();

        assert!(s
            .migrate_legacy_file(KEY_MODEL_SELECTED, &legacy)
            .await
            .unwrap());
        let latest = s.latest(KEY_MODEL_SELECTED).await.unwrap().unwrap();
        assert_eq!(
            model_from_value_json(&latest).as_deref(),
            Some("legacy-model")
        );
        let history = s.history(KEY_MODEL_SELECTED, 10).await.unwrap();
        assert_eq!(history[0].origin, "migration");

        // The file was renamed with the .imported suffix — never re-imported.
        assert!(!legacy.exists());
        assert!(dir.path().join("model-selection.json.imported").exists());
        assert!(!s
            .migrate_legacy_file(KEY_MODEL_SELECTED, &legacy)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn migration_skips_when_store_already_has_the_key() {
        let (_f, s) = store().await;
        put(&s, KEY_MODEL_SELECTED, &model_value_json("old"), "console").await;

        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join("model-selection.json");
        std::fs::write(&legacy, r#"{ "model": "stale" }"#).unwrap();

        assert!(!s
            .migrate_legacy_file(KEY_MODEL_SELECTED, &legacy)
            .await
            .unwrap());
        let latest = s.latest(KEY_MODEL_SELECTED).await.unwrap().unwrap();
        assert_eq!(model_from_value_json(&latest).as_deref(), Some("old"));
        // The stale file stays put (ignored forever) — only an actual import renames.
        assert!(legacy.exists());
    }

    #[tokio::test]
    async fn migration_tolerates_missing_and_corrupt_files() {
        let (_f, s) = store().await;
        let dir = tempfile::tempdir().unwrap();

        // Missing file: no-op.
        assert!(!s
            .migrate_legacy_file(KEY_MODEL_SELECTED, &dir.path().join("absent.json"))
            .await
            .unwrap());

        // Corrupt file: warn + skip, never fail startup.
        let corrupt = dir.path().join("backend-selection.json");
        std::fs::write(&corrupt, "not json {").unwrap();
        assert!(!s
            .migrate_legacy_file(KEY_BACKEND_SELECTED, &corrupt)
            .await
            .unwrap());
        assert!(s.latest(KEY_BACKEND_SELECTED).await.unwrap().is_none());
        assert!(corrupt.exists(), "a skipped file is left for inspection");
    }

    #[test]
    fn model_value_json_roundtrip_and_cleared_sentinel() {
        assert_eq!(
            model_from_value_json(&model_value_json("gemini-2.5-pro")).as_deref(),
            Some("gemini-2.5-pro")
        );
        assert_eq!(model_from_value_json(&model_value_json("")), None);
        assert_eq!(model_from_value_json(r#"{"model":"   "}"#), None);
        assert_eq!(model_from_value_json("not json"), None);
    }

    #[test]
    fn backend_selection_value_json_matches_legacy_shape() {
        let selection = backend_selection_from_value_json(
            r#"{"conversation":"runtime:acpx_claude","auth":"claude-subscription","task_runtime":null}"#,
        )
        .unwrap();
        assert_eq!(selection.conversation, "runtime:acpx_claude");
        assert_eq!(selection.auth.as_deref(), Some("claude-subscription"));
        assert!(backend_selection_from_value_json("nope").is_none());
    }
}
