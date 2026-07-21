//! Control Plane integration credentials (US — External Control Plane and
//! SDK, Phase 1: "auth scopes").
//!
//! `SqliteCredentialStore` is a new, agent-owned store — same conventions as
//! `adaptive::schedule::SqliteScheduleStore` (its doc comment literally says
//! "copied from `bastion-runtime`'s `SqliteTaskStore`"): `spawn_blocking`
//! around a synchronous `rusqlite::Connection` (never held across `.await`),
//! `PRAGMA journal_mode=WAL; busy_timeout`, idempotent `CREATE TABLE IF NOT
//! EXISTS`, an owner-scoped IDOR guard (`WHERE id=?1 AND owner_id=?2`,
//! bailing — never silently no-opping — on zero rows changed), and
//! `serde_json` TEXT columns for structured fields.
//!
//! This is deliberately **not** `OwnerMap` (the existing
//! `channel::OwnerMap`/`BASTION_BOOTSTRAP_TOKEN` model): that's a flat
//! `token -> owner_id` map with no scope concept, read from `bastion.toml`/
//! env at startup. A Control Plane credential is scoped (see
//! [`super::scope`]), optionally project-tagged, individually issuable and
//! revocable at runtime, and persisted — it generalizes MCP's
//! `TokenPermissions`/`authenticate_token` (`mcp/server.rs`) rather than
//! reusing the webhook channel's model.
//!
//! Phase 1 note: this store is **not yet wired into `main.rs`** or any live
//! HTTP route — nothing calls `init_schema`/`issue`/`authenticate` outside
//! tests yet. Wiring it up is a later phase's job, once `/v1/*` routes exist
//! to actually read/write it (see the plan doc). Until then this module is a
//! pure, additive library surface.

use base64::Engine;
use rand::RngCore;
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::task::spawn_blocking;

use super::scope::ScopeSet;

/// Prefix on every issued plaintext token, purely for operator identifiability
/// (grep-ability in logs/configs, same convention as Stripe/GitHub API keys —
/// never a security boundary by itself). "bcp" = Bastion Control Plane.
const TOKEN_PREFIX: &str = "bcp_";

/// Random entropy per issued token. 32 bytes matches the CSPRNG sizing already
/// used for `BASTION_BOOTSTRAP_TOKEN`/`APP_JWT_SECRET` generation in
/// `installer.sh`'s `random_secret` (`openssl rand -hex 32`).
const TOKEN_ENTROPY_BYTES: usize = 32;

fn open_conn(path: &str) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")?;
    Ok(conn)
}

const SCHEMA_SQL: &str = "
    PRAGMA journal_mode=WAL;
    PRAGMA busy_timeout=5000;

    CREATE TABLE IF NOT EXISTS control_plane_credentials (
        id           TEXT    PRIMARY KEY,
        owner_id     TEXT    NOT NULL,
        project      TEXT,
        label        TEXT    NOT NULL,
        scopes_json  TEXT    NOT NULL,
        token_hash   TEXT    NOT NULL,
        created_at   INTEGER NOT NULL,
        revoked_at   INTEGER
    );
    CREATE INDEX IF NOT EXISTS idx_cp_cred_owner ON control_plane_credentials(owner_id);
    CREATE UNIQUE INDEX IF NOT EXISTS idx_cp_cred_hash ON control_plane_credentials(token_hash);
";

const READ_COLUMNS: &str = "id, owner_id, project, label, scopes_json, created_at, revoked_at";

/// Wall-clock now as nanoseconds-since-epoch — mirrors
/// `adaptive::schedule::now_nanos`'s convention for this repo's timestamp
/// columns.
fn now_nanos() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as i64
}

/// Generate a new plaintext token: `bcp_` + 32 CSPRNG bytes, URL-safe base64
/// (no padding, so it is safe unescaped in a header/URL). Returned to the
/// caller exactly once by [`SqliteCredentialStore::issue`] — never logged,
/// never persisted; only its hash is stored (see [`hash_token`]).
fn generate_token() -> String {
    let mut bytes = [0u8; TOKEN_ENTROPY_BYTES];
    rand::thread_rng().fill_bytes(&mut bytes);
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    format!("{TOKEN_PREFIX}{encoded}")
}

/// SHA-256 hex digest of a presented/generated token.
///
/// No `constant_time_eq` is needed here (unlike `mcp/server.rs`'s
/// `authenticate_token`, which scans an in-memory `HashMap` of *plaintext*
/// configured tokens and must compare candidate bytes without an early exit).
/// Here the presented token is hashed and looked up by an exact SQL equality
/// match against a stored hash — the raw secret's bytes are never compared to
/// anything, so there is no early-exit timing channel to close.
fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    let digest = hasher.finalize();
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Why [`SqliteCredentialStore::revoke`] found nothing to revoke. Kept
/// distinct from a generic `anyhow::Error` (downcast via
/// `downcast_ref::<RevokeError>()`, the same pattern `bastion-core` uses for
/// `BastionError::PrivacyEgressBlocked`) so a future HTTP handler can map
/// [`Self::NotFound`] to `404` and [`Self::AlreadyRevoked`] to an idempotent
/// `200`/`204` — collapsing both into one opaque error would force every
/// caller to treat a harmless double-revoke the same as a real IDOR/missing-id
/// failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RevokeError {
    #[error("no credential with that id exists for this owner (wrong owner or missing id)")]
    NotFound,
    #[error("credential is already revoked")]
    AlreadyRevoked,
}

/// A resolved, authenticated credential — the generalized replacement for
/// `mcp/server.rs`'s `TokenPermissions`, returned by
/// [`SqliteCredentialStore::authenticate`]. Carries no token/hash material.
#[derive(Debug, Clone, PartialEq)]
pub struct AuthenticatedCredential {
    pub credential_id: String,
    pub owner_id: String,
    /// Project-namespace tag, carried only at this layer — `bastion-core`'s
    /// `TaskCase`/`SqliteTaskStore` have no `project` concept and are not
    /// modified to add one (see the plan doc's "project isolation" decision).
    /// Phase 1 stores this field; no query anywhere filters by it yet.
    pub project: Option<String>,
    pub scopes: ScopeSet,
}

/// A credential's metadata as returned by [`SqliteCredentialStore::list_for_owner`].
/// Never carries the token or its hash.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CredentialSummary {
    pub id: String,
    pub owner_id: String,
    pub project: Option<String>,
    pub label: String,
    pub scopes: ScopeSet,
    pub created_at: i64,
    pub revoked_at: Option<i64>,
}

struct RawCredentialRow {
    id: String,
    owner_id: String,
    project: Option<String>,
    label: String,
    scopes_json: String,
    created_at: i64,
    revoked_at: Option<i64>,
}

fn read_credential_row(row: &rusqlite::Row) -> rusqlite::Result<RawCredentialRow> {
    Ok(RawCredentialRow {
        id: row.get(0)?,
        owner_id: row.get(1)?,
        project: row.get(2)?,
        label: row.get(3)?,
        scopes_json: row.get(4)?,
        created_at: row.get(5)?,
        revoked_at: row.get(6)?,
    })
}

fn raw_to_summary(raw: RawCredentialRow) -> anyhow::Result<CredentialSummary> {
    Ok(CredentialSummary {
        id: raw.id,
        owner_id: raw.owner_id,
        project: raw.project,
        label: raw.label,
        scopes: serde_json::from_str(&raw.scopes_json)?,
        created_at: raw.created_at,
        revoked_at: raw.revoked_at,
    })
}

/// SQLite-backed store for Control Plane integration credentials. Owns its
/// own `control_plane_credentials` table on the shared session-db file —
/// never the task tables (`bastion-runtime`'s `SqliteTaskStore`) or the
/// `schedules` table.
#[derive(Clone)]
pub struct SqliteCredentialStore {
    db_path: String,
}

impl SqliteCredentialStore {
    /// Build a store over the sqlite file at `db_path`. Does not touch the
    /// database — call [`Self::init_schema`] before first use.
    pub fn new(db_path: impl Into<String>) -> Self {
        Self {
            db_path: db_path.into(),
        }
    }

    /// Create the `control_plane_credentials` table/indexes if absent.
    /// Idempotent; safe to call on every startup.
    pub async fn init_schema(&self) -> anyhow::Result<()> {
        let path = self.db_path.clone();
        spawn_blocking(move || {
            let conn = open_conn(&path)?;
            conn.execute_batch(SCHEMA_SQL)?;
            Ok::<_, anyhow::Error>(())
        })
        .await?
    }

    /// Issue a new credential for `owner`, optionally tagged with `project`,
    /// granting `scopes`. `label` is an operator-facing name (e.g.
    /// "paperclip-prod") — purely descriptive, never used for auth.
    ///
    /// Returns `(credential_id, plaintext_token)`. The plaintext token is
    /// generated here and returned exactly once; only its SHA-256 hash is
    /// persisted (`token_hash`). There is no way to recover a lost token —
    /// the caller must issue a new one and revoke the old.
    pub async fn issue(
        &self,
        owner: &str,
        project: Option<&str>,
        scopes: ScopeSet,
        label: &str,
    ) -> anyhow::Result<(String, String)> {
        let path = self.db_path.clone();
        let id = uuid_like_id();
        let owner = owner.to_string();
        let project = project.map(str::to_owned);
        let label = label.to_string();
        let scopes_json = serde_json::to_string(&scopes)?;
        let token = generate_token();
        let token_hash = hash_token(&token);
        let created_at = now_nanos();
        let insert_id = id.clone();
        let insert_token = token.clone();
        spawn_blocking(move || {
            let conn = open_conn(&path)?;
            conn.execute(
                "INSERT INTO control_plane_credentials
                    (id, owner_id, project, label, scopes_json, token_hash, created_at, revoked_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL)",
                rusqlite::params![
                    insert_id,
                    owner,
                    project,
                    label,
                    scopes_json,
                    token_hash,
                    created_at
                ],
            )?;
            Ok::<_, anyhow::Error>(())
        })
        .await??;
        Ok((id, insert_token))
    }

    /// Resolve a presented plaintext token to its credential, or `None` if it
    /// doesn't match any active (non-revoked) credential.
    ///
    /// Fail-closed by construction: any lookup miss (unknown hash, or a
    /// matching hash whose row is revoked) returns `None`, never a partial
    /// or default-permissive credential — mirrors `authenticate_token`'s
    /// "missing/unknown token is REJECTED, never an implicit grant" contract.
    pub async fn authenticate(
        &self,
        presented_token: &str,
    ) -> anyhow::Result<Option<AuthenticatedCredential>> {
        let path = self.db_path.clone();
        let token_hash = hash_token(presented_token);
        spawn_blocking(move || {
            let conn = open_conn(&path)?;
            let mut stmt = conn.prepare(&format!(
                "SELECT {READ_COLUMNS} FROM control_plane_credentials \
                 WHERE token_hash = ?1 AND revoked_at IS NULL"
            ))?;
            let mut rows = stmt.query(rusqlite::params![token_hash])?;
            let Some(row) = rows.next()? else {
                return Ok::<_, anyhow::Error>(None);
            };
            let raw = read_credential_row(row)?;
            let scopes: ScopeSet = serde_json::from_str(&raw.scopes_json)?;
            Ok(Some(AuthenticatedCredential {
                credential_id: raw.id,
                owner_id: raw.owner_id,
                project: raw.project,
                scopes,
            }))
        })
        .await?
    }

    /// List every credential owned by `owner` (revoked included), newest
    /// first. Never returns the token or its hash — [`CredentialSummary`]
    /// carries neither field.
    pub async fn list_for_owner(&self, owner: &str) -> anyhow::Result<Vec<CredentialSummary>> {
        let path = self.db_path.clone();
        let owner = owner.to_string();
        spawn_blocking(move || {
            let conn = open_conn(&path)?;
            let mut stmt = conn.prepare(&format!(
                "SELECT {READ_COLUMNS} FROM control_plane_credentials \
                 WHERE owner_id = ?1 ORDER BY created_at DESC, id DESC"
            ))?;
            let raws = stmt
                .query_map(rusqlite::params![owner], read_credential_row)?
                .collect::<Result<Vec<_>, _>>()?;
            raws.into_iter()
                .map(raw_to_summary)
                .collect::<anyhow::Result<Vec<CredentialSummary>>>()
        })
        .await?
    }

    /// Revoke a credential owned by `owner`. Owner-scoped IDOR guard: bails
    /// on zero rows changed — never silently no-ops. The bail is a typed
    /// [`RevokeError`] ([`RevokeError::NotFound`] for a wrong owner/missing
    /// id, [`RevokeError::AlreadyRevoked`] for a credential that exists but
    /// was already revoked) so callers can tell the two apart instead of
    /// seeing one opaque failure. Revocation is permanent; there is no
    /// un-revoke, matching the "issue a new one" recovery story in
    /// [`Self::issue`].
    pub async fn revoke(&self, owner: &str, credential_id: &str) -> anyhow::Result<()> {
        let path = self.db_path.clone();
        let owner = owner.to_string();
        let credential_id = credential_id.to_string();
        let revoked_at = now_nanos();
        spawn_blocking(move || {
            let conn = open_conn(&path)?;
            let changed = conn.execute(
                "UPDATE control_plane_credentials SET revoked_at = ?1 \
                 WHERE id = ?2 AND owner_id = ?3 AND revoked_at IS NULL",
                rusqlite::params![revoked_at, credential_id, owner],
            )?;
            if changed == 0 {
                let exists: bool = conn
                    .query_row(
                        "SELECT 1 FROM control_plane_credentials WHERE id = ?1 AND owner_id = ?2",
                        rusqlite::params![credential_id, owner],
                        |_| Ok(true),
                    )
                    .optional()?
                    .unwrap_or(false);
                let err = if exists {
                    RevokeError::AlreadyRevoked
                } else {
                    RevokeError::NotFound
                };
                return Err(err.into());
            }
            Ok::<_, anyhow::Error>(())
        })
        .await?
    }
}

/// A sufficiently-unique id for a new credential row. Not a UUID library
/// dependency (none is pulled in by this crate today) — 16 random bytes,
/// hex-encoded, has the same collision-resistance property this repo already
/// relies on for `BASTION_BOOTSTRAP_TOKEN`-style secrets.
fn uuid_like_id() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::super::scope::Scope;
    use super::*;
    use tempfile::NamedTempFile;

    async fn make_store() -> (NamedTempFile, SqliteCredentialStore) {
        let f = NamedTempFile::new().expect("tempfile");
        let path = f.path().to_str().expect("utf8 path").to_owned();
        let store = SqliteCredentialStore::new(path);
        store.init_schema().await.expect("init_schema");
        (f, store)
    }

    #[tokio::test]
    async fn issue_then_authenticate_round_trip() {
        let (_f, store) = make_store().await;
        let scopes = ScopeSet::new([Scope::TasksRead, Scope::TasksCreate]);
        let (id, token) = store
            .issue("alice", Some("paperclip"), scopes.clone(), "paperclip-prod")
            .await
            .expect("issue");

        assert!(token.starts_with(TOKEN_PREFIX));

        let cred = store
            .authenticate(&token)
            .await
            .expect("authenticate")
            .expect("token should resolve");
        assert_eq!(cred.credential_id, id);
        assert_eq!(cred.owner_id, "alice");
        assert_eq!(cred.project.as_deref(), Some("paperclip"));
        assert_eq!(cred.scopes, scopes);
    }

    #[tokio::test]
    async fn authenticate_unknown_token_returns_none() {
        let (_f, store) = make_store().await;
        let result = store
            .authenticate("bcp_not-a-real-token")
            .await
            .expect("authenticate should not error on a miss");
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn revoked_credential_no_longer_authenticates() {
        let (_f, store) = make_store().await;
        let (id, token) = store
            .issue("alice", None, ScopeSet::new([Scope::TasksRead]), "test")
            .await
            .expect("issue");

        store.revoke("alice", &id).await.expect("revoke");

        let result = store
            .authenticate(&token)
            .await
            .expect("authenticate after revoke should not error");
        assert!(result.is_none(), "revoked token must not authenticate");
    }

    #[tokio::test]
    async fn revoke_is_owner_scoped_idor_guard() {
        let (_f, store) = make_store().await;
        let (id, _token) = store
            .issue("alice", None, ScopeSet::new([Scope::TasksRead]), "test")
            .await
            .expect("issue");

        // Wrong owner cannot revoke alice's credential.
        assert!(store.revoke("bob", &id).await.is_err());

        // Correct owner can.
        store.revoke("alice", &id).await.expect("revoke");

        // Revoking again (already revoked) also bails — not a silent no-op.
        assert!(store.revoke("alice", &id).await.is_err());
    }

    /// A future HTTP handler needs to tell "wrong owner / no such id" (404)
    /// apart from "exists but already revoked" (safe to treat as an
    /// idempotent no-op) — this pins the two `RevokeError` variants the
    /// store must produce for each case.
    #[tokio::test]
    async fn revoke_error_kind_distinguishes_not_found_from_already_revoked() {
        let (_f, store) = make_store().await;
        let (id, _token) = store
            .issue("alice", None, ScopeSet::new([Scope::TasksRead]), "test")
            .await
            .expect("issue");

        let missing_err = store
            .revoke("alice", "no-such-id")
            .await
            .expect_err("missing id must error");
        assert_eq!(
            missing_err.downcast_ref::<RevokeError>(),
            Some(&RevokeError::NotFound)
        );

        let wrong_owner_err = store
            .revoke("bob", &id)
            .await
            .expect_err("wrong owner must error");
        assert_eq!(
            wrong_owner_err.downcast_ref::<RevokeError>(),
            Some(&RevokeError::NotFound),
            "a wrong-owner revoke must look identical to a missing id — never confirm the id exists for another owner"
        );

        store.revoke("alice", &id).await.expect("first revoke");

        let already_err = store
            .revoke("alice", &id)
            .await
            .expect_err("second revoke must error");
        assert_eq!(
            already_err.downcast_ref::<RevokeError>(),
            Some(&RevokeError::AlreadyRevoked)
        );
    }

    #[tokio::test]
    async fn list_for_owner_is_isolated_and_omits_secrets() {
        let (_f, store) = make_store().await;
        store
            .issue(
                "alice",
                None,
                ScopeSet::new([Scope::TasksRead]),
                "alice-cred",
            )
            .await
            .expect("issue");
        store
            .issue("bob", None, ScopeSet::new([Scope::TasksRead]), "bob-cred")
            .await
            .expect("issue");

        let alice_creds = store.list_for_owner("alice").await.expect("list");
        assert_eq!(alice_creds.len(), 1);
        assert_eq!(alice_creds[0].label, "alice-cred");
        assert!(alice_creds[0].revoked_at.is_none());

        let bob_creds = store.list_for_owner("bob").await.expect("list");
        assert_eq!(bob_creds.len(), 1);
        assert_eq!(bob_creds[0].label, "bob-cred");
    }

    #[test]
    fn hash_token_is_deterministic_and_not_the_plaintext() {
        let token = "bcp_example";
        let h1 = hash_token(token);
        let h2 = hash_token(token);
        assert_eq!(h1, h2);
        assert_ne!(h1, token);
        assert_eq!(h1.len(), 64, "sha256 hex digest is 64 chars");
    }

    #[test]
    fn generate_token_has_prefix_and_sufficient_length() {
        let token = generate_token();
        assert!(token.starts_with(TOKEN_PREFIX));
        // 32 raw bytes base64url-encoded (no padding) is 43 chars, plus the prefix.
        assert!(token.len() > 40);
    }
}
