//! Sqlite-backed persistence for `ExtensionHost`'s installed set — same
//! `spawn_blocking` + WAL pattern already established 3x in this crate
//! (`SqliteCredentialStateStore`, `codex_connector::SqliteCodexTokenStore`).
//!
//! `ExtensionHost` itself stays pure in-memory (`host.rs`'s own doc:
//! "product code, deliberately outside the kernel" — no I/O dependency).
//! This store never becomes a second source of truth for WHETHER something
//! is active: it only records enough to reconstruct the same
//! `Arc<dyn ExtensionInstance>` and re-`install()` it through
//! `ExtensionHost`'s own already-atomic path at boot — never a second
//! activation mechanism. `extension_command.rs::reload_persisted` is the
//! only reader that turns rows back into live extensions.

use bastion_extension_protocol::ExtensionManifest;
use rusqlite::Connection;
use tokio::task::spawn_blocking;

fn open_conn(path: &str) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")?;
    Ok(conn)
}

const SCHEMA_SQL: &str = "
    PRAGMA journal_mode=WAL;
    PRAGMA busy_timeout=5000;

    CREATE TABLE IF NOT EXISTS installed_extension (
        id            TEXT NOT NULL PRIMARY KEY,
        owner         TEXT NOT NULL,
        kind          TEXT NOT NULL,
        manifest_json TEXT NOT NULL
    );
";

/// Which reconstruction path a persisted row needs at boot — mirrors the
/// only two `Arc<dyn ExtensionInstance>` constructors
/// `extension_command.rs` actually wires today
/// (`install_one_extension`'s declarative branch, `install_git_capability`).
/// Deliberately not every `ExtensionKind` — only what's real; a manifest
/// whose kind isn't one of these two never reaches `ExtensionHost::install`
/// in the first place (see `install_one_extension`'s skip branches), so
/// there is nothing else to persist yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconstructKind {
    Declarative,
    GitCapability,
}

impl ReconstructKind {
    fn as_str(&self) -> &'static str {
        match self {
            ReconstructKind::Declarative => "declarative",
            ReconstructKind::GitCapability => "git_capability",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "declarative" => Some(Self::Declarative),
            "git_capability" => Some(Self::GitCapability),
            _ => None,
        }
    }
}

/// One row, already parsed back into the shape `reload_persisted` needs.
pub struct PersistedExtension {
    pub owner: String,
    pub manifest: ExtensionManifest,
    pub kind: ReconstructKind,
}

/// Sqlite-backed record of what's installed. `save`/`remove` are called at
/// the exact call sites where `ExtensionHost::install`/`revoke` already
/// succeed — never speculatively, never as a second guess at what's active.
pub struct SqliteExtensionStore {
    db_path: String,
}

impl SqliteExtensionStore {
    pub fn new(db_path: impl Into<String>) -> Self {
        Self {
            db_path: db_path.into(),
        }
    }

    pub async fn init_schema(&self) -> anyhow::Result<()> {
        let path = self.db_path.clone();
        spawn_blocking(move || {
            let conn = open_conn(&path)?;
            conn.execute_batch(SCHEMA_SQL)?;
            Ok::<_, anyhow::Error>(())
        })
        .await?
    }

    /// Record one extension as installed. `INSERT ... ON CONFLICT DO
    /// UPDATE`: re-installing an id (the same id can only be installed once
    /// per `ExtensionHost::install`'s own `AlreadyInstalled` guard, but this
    /// store doesn't re-derive that invariant — it just reflects whatever
    /// the caller already got past that guard for) overwrites the row
    /// cleanly instead of erroring on the primary key.
    pub async fn save(
        &self,
        owner: &str,
        manifest: &ExtensionManifest,
        kind: ReconstructKind,
    ) -> anyhow::Result<()> {
        let path = self.db_path.clone();
        let id = manifest.id.clone();
        let owner = owner.to_string();
        let manifest_json = serde_json::to_string(manifest)?;
        let kind_str = kind.as_str();
        spawn_blocking(move || {
            let conn = open_conn(&path)?;
            conn.execute(
                "INSERT INTO installed_extension (id, owner, kind, manifest_json) \
                 VALUES (?1, ?2, ?3, ?4) \
                 ON CONFLICT(id) DO UPDATE SET owner = excluded.owner, \
                     kind = excluded.kind, manifest_json = excluded.manifest_json",
                rusqlite::params![id, owner, kind_str, manifest_json],
            )?;
            Ok::<_, anyhow::Error>(())
        })
        .await?
    }

    /// Drop the persisted record. Removing an id that was never persisted
    /// (or already removed) is not an error — same "clearing is idempotent"
    /// discipline the rest of this crate's stores already follow.
    pub async fn remove(&self, id: &str) -> anyhow::Result<()> {
        let path = self.db_path.clone();
        let id = id.to_string();
        spawn_blocking(move || {
            let conn = open_conn(&path)?;
            conn.execute(
                "DELETE FROM installed_extension WHERE id = ?1",
                rusqlite::params![id],
            )?;
            Ok::<_, anyhow::Error>(())
        })
        .await?
    }

    /// Every persisted row. A row whose `kind` this build no longer
    /// recognizes, or whose `manifest_json` no longer deserializes, is
    /// logged and skipped by the caller (`reload_persisted`) rather than
    /// failing the whole load — one corrupt record must never block boot.
    pub async fn load_all(&self) -> anyhow::Result<Vec<(String, String, String)>> {
        let path = self.db_path.clone();
        spawn_blocking(move || {
            let conn = open_conn(&path)?;
            let mut stmt =
                conn.prepare("SELECT owner, kind, manifest_json FROM installed_extension")?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok::<_, anyhow::Error>(rows)
        })
        .await?
    }

    /// [`Self::load_all`] parsed into [`PersistedExtension`]s, skipping (and
    /// logging) any row that fails to parse instead of erroring the whole
    /// load.
    pub async fn load_all_parsed(&self) -> anyhow::Result<Vec<PersistedExtension>> {
        let raw = self.load_all().await?;
        let mut out = Vec::with_capacity(raw.len());
        for (owner, kind_str, manifest_json) in raw {
            let Some(kind) = ReconstructKind::parse(&kind_str) else {
                tracing::warn!(
                    event = "extension_persistence_unknown_kind",
                    kind = %kind_str,
                );
                continue;
            };
            let manifest: ExtensionManifest = match serde_json::from_str(&manifest_json) {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!(event = "extension_persistence_bad_manifest", error = %e);
                    continue;
                }
            };
            out.push(PersistedExtension {
                owner,
                manifest,
                kind,
            });
        }
        Ok(out)
    }

    /// Whether anything is persisted for this id — used only by tests today,
    /// but a small, honest read primitive rather than exposing `load_all`
    /// for a single lookup.
    #[cfg(test)]
    async fn contains(&self, id: &str) -> anyhow::Result<bool> {
        use rusqlite::OptionalExtension;
        let path = self.db_path.clone();
        let id = id.to_string();
        spawn_blocking(move || {
            let conn = open_conn(&path)?;
            let found: Option<i64> = conn
                .query_row(
                    "SELECT 1 FROM installed_extension WHERE id = ?1",
                    rusqlite::params![id],
                    |row| row.get(0),
                )
                .optional()?;
            Ok::<_, anyhow::Error>(found.is_some())
        })
        .await?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bastion_extension_protocol::{Entrypoint, ExtensionKind, PermissionSet};
    use semver::Version;
    use tempfile::NamedTempFile;

    fn manifest(id: &str) -> ExtensionManifest {
        ExtensionManifest {
            id: id.to_string(),
            version: Version::new(1, 0, 0),
            kind: ExtensionKind::Declarative,
            compat: semver::VersionReq::parse("*").unwrap(),
            provides: vec![],
            requires: vec![],
            permissions: PermissionSet::none(),
            secrets: vec![],
            entrypoint: Entrypoint::Declarative {
                artifact_path: "noop.json".into(),
            },
            migrations: vec![],
            signature: None,
        }
    }

    async fn store() -> (NamedTempFile, SqliteExtensionStore) {
        let f = NamedTempFile::new().unwrap();
        let s = SqliteExtensionStore::new(f.path().to_str().unwrap());
        s.init_schema().await.unwrap();
        (f, s)
    }

    #[tokio::test]
    async fn save_then_load_round_trips_manifest_and_kind() {
        let (_f, s) = store().await;
        let m = manifest("acme/widget");
        s.save("alice", &m, ReconstructKind::Declarative)
            .await
            .unwrap();

        let loaded = s.load_all_parsed().await.unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].owner, "alice");
        assert_eq!(loaded[0].manifest.id, "acme/widget");
        assert_eq!(loaded[0].kind, ReconstructKind::Declarative);
    }

    #[tokio::test]
    async fn save_twice_for_the_same_id_overwrites_not_duplicates() {
        let (_f, s) = store().await;
        s.save(
            "alice",
            &manifest("acme/widget"),
            ReconstructKind::Declarative,
        )
        .await
        .unwrap();
        s.save(
            "bob",
            &manifest("acme/widget"),
            ReconstructKind::GitCapability,
        )
        .await
        .unwrap();

        let loaded = s.load_all_parsed().await.unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].owner, "bob");
        assert_eq!(loaded[0].kind, ReconstructKind::GitCapability);
    }

    #[tokio::test]
    async fn remove_drops_the_row_and_is_idempotent() {
        let (_f, s) = store().await;
        s.save(
            "alice",
            &manifest("acme/widget"),
            ReconstructKind::Declarative,
        )
        .await
        .unwrap();
        assert!(s.contains("acme/widget").await.unwrap());

        s.remove("acme/widget").await.unwrap();
        assert!(!s.contains("acme/widget").await.unwrap());

        // Removing again (already gone) must not error.
        s.remove("acme/widget").await.unwrap();
    }

    #[tokio::test]
    async fn remove_of_unknown_id_is_a_harmless_no_op() {
        let (_f, s) = store().await;
        s.remove("never/installed").await.unwrap();
    }

    #[tokio::test]
    async fn two_extensions_persist_independently() {
        let (_f, s) = store().await;
        s.save(
            "alice",
            &manifest("acme/widget"),
            ReconstructKind::Declarative,
        )
        .await
        .unwrap();
        s.save(
            "alice",
            &manifest("bastion/git-capability"),
            ReconstructKind::GitCapability,
        )
        .await
        .unwrap();

        let loaded = s.load_all_parsed().await.unwrap();
        assert_eq!(loaded.len(), 2);
    }

    #[tokio::test]
    async fn a_row_survives_across_store_instances_pointed_at_the_same_db() {
        let f = NamedTempFile::new().unwrap();
        let path = f.path().to_str().unwrap().to_string();

        let first = SqliteExtensionStore::new(path.clone());
        first.init_schema().await.unwrap();
        first
            .save(
                "alice",
                &manifest("acme/widget"),
                ReconstructKind::Declarative,
            )
            .await
            .unwrap();
        drop(first);

        let second = SqliteExtensionStore::new(path);
        let loaded = second.load_all_parsed().await.unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].manifest.id, "acme/widget");
    }
}
