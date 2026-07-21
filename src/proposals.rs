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
//! v1 supports one kind: `persona_edit` (write `personas/<slug>/SOUL.md`).
//! Channel/MCP/model config edits stay backlog — they mutate bastion.toml
//! and need a restart story first.

use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tokio::task::spawn_blocking;

pub const MAX_CONTENT_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalStatus {
    Pending,
    Approved,
    Rejected,
}

/// v1: only persona edits. Kept as a tagged enum so channel/MCP kinds can
/// join without a schema change (payload is opaque JSON in the table).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProposalPayload {
    PersonaEdit { slug: String, content: String },
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

/// Apply an APPROVED proposal to disk. Returns a human line for the console.
/// Persona edits back up the previous SOUL.md beside itself before writing.
pub async fn apply(root: &Path, payload: &ProposalPayload) -> anyhow::Result<String> {
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
    }
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

    #[tokio::test]
    async fn apply_writes_soul_and_backs_up_previous() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let payload = edit("ada");
        let msg = apply(root, &payload).await.unwrap();
        assert!(msg.contains("personas/ada/SOUL.md written"));
        let soul = root.join("personas/ada/SOUL.md");
        assert!(soul.is_file());

        // second apply backs up the first
        apply(root, &payload).await.unwrap();
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
        assert!(apply(dir.path(), &payload).await.is_err());
    }
}
