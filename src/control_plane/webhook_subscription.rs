//! Webhook subscription storage (US — External Control Plane and SDK,
//! Phase 4: "Webhooks").
//!
//! `SqliteWebhookSubscriptionStore` mirrors `credential::SqliteCredentialStore`'s
//! conventions (own table on the shared session-db file, `spawn_blocking` +
//! sync `rusqlite`, owner-scoped IDOR guards) — see that module's doc comment
//! for the pattern this one is a structural copy of.
//!
//! `target_url` is validated with `adaptive::browser::validate_public_url`
//! (the SAME SSRF guard `HttpFetchBackend` runs before every page fetch,
//! US-204) exactly once, at subscription-creation time — never at delivery
//! time. This is a narrower guarantee than `browser.rs`'s per-fetch check
//! (see that module's doc comment on the DNS-rebinding residual gap): a
//! webhook subscription's URL is fixed and reused for many future
//! deliveries, so an attacker would need to control DNS for the
//! ALREADY-REGISTERED hostname to exploit rebinding after registration,
//! rather than controlling a single dynamically-chosen navigation target.
//! Documented as a known, accepted residual risk in
//! `docs/en/control-plane-security.md` — re-validating on every delivery was
//! considered and deferred (it would also make delivery untestable against a
//! local mock server without a `cfg(test)` escape hatch, which is worse).

use base64::Engine;
use rand::RngCore;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tokio::task::spawn_blocking;

fn open_conn(path: &str) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")?;
    Ok(conn)
}

const SCHEMA_SQL: &str = "
    PRAGMA journal_mode=WAL;
    PRAGMA busy_timeout=5000;

    CREATE TABLE IF NOT EXISTS webhook_subscriptions (
        id           TEXT    PRIMARY KEY,
        owner_id     TEXT    NOT NULL,
        target_url   TEXT    NOT NULL,
        event_types  TEXT    NOT NULL,
        secret       TEXT    NOT NULL,
        created_at   INTEGER NOT NULL,
        revoked_at   INTEGER
    );
    CREATE INDEX IF NOT EXISTS idx_webhook_sub_owner ON webhook_subscriptions(owner_id);
    CREATE INDEX IF NOT EXISTS idx_webhook_sub_active
        ON webhook_subscriptions(owner_id, revoked_at);
";

const READ_COLUMNS: &str = "id, owner_id, target_url, event_types, created_at, revoked_at";

fn now_nanos() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as i64
}

/// 32 random bytes, base64url — same shape/rationale as
/// `credential::generate_token`, used here as the HMAC signing secret
/// (`webhook_delivery::sign_payload`) rather than a bearer credential.
fn generate_secret() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn uuid_like_id() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A subscription's metadata, as returned by `list_for_owner`/`issue`. Never
/// carries the signing secret — matches `credential::CredentialSummary`'s
/// "the secret is shown exactly once, at creation" contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubscriptionSummary {
    pub id: String,
    pub owner_id: String,
    pub target_url: String,
    pub event_types: Vec<String>,
    pub created_at: i64,
    pub revoked_at: Option<i64>,
}

/// A subscription as loaded for DELIVERY purposes — the only caller that
/// legitimately needs the secret back out of storage
/// (`webhook_delivery::run_delivery_loop`, to sign each outbound payload).
#[derive(Debug, Clone, PartialEq)]
pub struct SubscriptionForDelivery {
    pub id: String,
    pub owner_id: String,
    pub target_url: String,
    pub event_types: Vec<String>,
    pub secret: String,
}

struct RawRow {
    id: String,
    owner_id: String,
    target_url: String,
    event_types_json: String,
    created_at: i64,
    revoked_at: Option<i64>,
}

fn read_row(row: &rusqlite::Row) -> rusqlite::Result<RawRow> {
    Ok(RawRow {
        id: row.get(0)?,
        owner_id: row.get(1)?,
        target_url: row.get(2)?,
        event_types_json: row.get(3)?,
        created_at: row.get(4)?,
        revoked_at: row.get(5)?,
    })
}

fn raw_to_summary(raw: RawRow) -> anyhow::Result<SubscriptionSummary> {
    Ok(SubscriptionSummary {
        id: raw.id,
        owner_id: raw.owner_id,
        target_url: raw.target_url,
        event_types: serde_json::from_str(&raw.event_types_json)?,
        created_at: raw.created_at,
        revoked_at: raw.revoked_at,
    })
}

/// Same [`super::credential::RevokeError`] shape, reused here rather than
/// duplicated — both stores need the identical NotFound-vs-AlreadyRevoked
/// distinction for the identical reason.
pub type RevokeError = super::credential::RevokeError;

#[derive(Clone)]
pub struct SqliteWebhookSubscriptionStore {
    db_path: String,
}

impl SqliteWebhookSubscriptionStore {
    pub fn new(db_path: impl Into<String>) -> Self {
        Self {
            db_path: db_path.into(),
        }
    }

    /// The underlying SQLite file path — mainly useful for tests that need
    /// to seed rows directly (bypassing the SSRF-gated `issue()`); see
    /// `tests/control_plane_routes.rs`'s webhook-subscription tests.
    pub fn db_path(&self) -> &str {
        &self.db_path
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

    /// Validate `target_url` against the SSRF guard (see module doc), then
    /// persist the subscription. Returns `(id, secret)` — the secret is
    /// generated here and returned exactly once; only itself is ever
    /// persisted (no hash — the delivery loop needs the plaintext back to
    /// sign each outbound request, unlike a bearer credential which only
    /// ever needs equality-checked, so hashing doesn't apply here the way it
    /// does in `credential.rs`).
    pub async fn issue(
        &self,
        owner: &str,
        target_url: &str,
        event_types: Vec<String>,
    ) -> anyhow::Result<(String, String)> {
        crate::adaptive::browser::validate_public_url(target_url)
            .await
            .map_err(|e| anyhow::anyhow!("target_url failed SSRF validation: {e}"))?;

        let path = self.db_path.clone();
        let id = uuid_like_id();
        let owner = owner.to_string();
        let target_url = target_url.to_string();
        let event_types_json = serde_json::to_string(&event_types)?;
        let secret = generate_secret();
        let created_at = now_nanos();
        let insert_id = id.clone();
        let insert_secret = secret.clone();
        spawn_blocking(move || {
            let conn = open_conn(&path)?;
            conn.execute(
                "INSERT INTO webhook_subscriptions
                    (id, owner_id, target_url, event_types, secret, created_at, revoked_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL)",
                rusqlite::params![
                    insert_id,
                    owner,
                    target_url,
                    event_types_json,
                    insert_secret,
                    created_at
                ],
            )?;
            Ok::<_, anyhow::Error>(())
        })
        .await??;
        Ok((id, secret))
    }

    pub async fn list_for_owner(&self, owner: &str) -> anyhow::Result<Vec<SubscriptionSummary>> {
        let path = self.db_path.clone();
        let owner = owner.to_string();
        spawn_blocking(move || {
            let conn = open_conn(&path)?;
            let mut stmt = conn.prepare(&format!(
                "SELECT {READ_COLUMNS} FROM webhook_subscriptions \
                 WHERE owner_id = ?1 ORDER BY created_at DESC, id DESC"
            ))?;
            let raws = stmt
                .query_map(rusqlite::params![owner], read_row)?
                .collect::<Result<Vec<_>, _>>()?;
            raws.into_iter()
                .map(raw_to_summary)
                .collect::<anyhow::Result<Vec<SubscriptionSummary>>>()
        })
        .await?
    }

    /// Every ACTIVE subscription across ALL owners matching `event_type`
    /// (or with an empty `event_types` list — "empty means all", per the
    /// spec/DTO doc comment). NOT owner-scoped by design: the delivery loop
    /// (`webhook_delivery::run_delivery_loop`) sweeps events for every owner
    /// in one pass, the same way `schedule::SqliteScheduleStore::due` is a
    /// global, unscoped sweep, not a per-owner query.
    pub async fn active_matching(
        &self,
        owner: &str,
        event_type: &str,
    ) -> anyhow::Result<Vec<SubscriptionForDelivery>> {
        let path = self.db_path.clone();
        let owner = owner.to_string();
        let event_type = event_type.to_string();
        spawn_blocking(move || {
            let conn = open_conn(&path)?;
            let mut stmt = conn.prepare(
                "SELECT id, owner_id, target_url, event_types, secret \
                 FROM webhook_subscriptions WHERE owner_id = ?1 AND revoked_at IS NULL",
            )?;
            let rows = stmt
                .query_map(rusqlite::params![owner], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;

            let mut matched = Vec::new();
            for (id, owner_id, target_url, event_types_json, secret) in rows {
                let event_types: Vec<String> = serde_json::from_str(&event_types_json)?;
                if event_types.is_empty() || event_types.iter().any(|t| t == &event_type) {
                    matched.push(SubscriptionForDelivery {
                        id,
                        owner_id,
                        target_url,
                        event_types,
                        secret,
                    });
                }
            }
            Ok::<_, anyhow::Error>(matched)
        })
        .await?
    }

    /// Owner-scoped IDOR guard, identical discipline to
    /// `credential::SqliteCredentialStore::revoke` (same [`RevokeError`]
    /// variants, same "existence check scoped to this owner so a wrong-owner
    /// revoke never reveals whether the id exists for someone else").
    pub async fn revoke(&self, owner: &str, subscription_id: &str) -> anyhow::Result<()> {
        let path = self.db_path.clone();
        let owner = owner.to_string();
        let subscription_id = subscription_id.to_string();
        let revoked_at = now_nanos();
        spawn_blocking(move || {
            let conn = open_conn(&path)?;
            let changed = conn.execute(
                "UPDATE webhook_subscriptions SET revoked_at = ?1 \
                 WHERE id = ?2 AND owner_id = ?3 AND revoked_at IS NULL",
                rusqlite::params![revoked_at, subscription_id, owner],
            )?;
            if changed == 0 {
                use rusqlite::OptionalExtension;
                let exists: bool = conn
                    .query_row(
                        "SELECT 1 FROM webhook_subscriptions WHERE id = ?1 AND owner_id = ?2",
                        rusqlite::params![subscription_id, owner],
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    async fn make_store() -> (NamedTempFile, SqliteWebhookSubscriptionStore) {
        let f = NamedTempFile::new().expect("tempfile");
        let path = f.path().to_str().expect("utf8 path").to_owned();
        let store = SqliteWebhookSubscriptionStore::new(path);
        store.init_schema().await.expect("init_schema");
        (f, store)
    }

    #[tokio::test]
    async fn issue_rejects_a_loopback_target_url() {
        let (_f, store) = make_store().await;
        let err = store
            .issue("alice", "http://127.0.0.1:9999/hook", vec![])
            .await
            .expect_err("loopback must be rejected");
        assert!(err.to_string().contains("SSRF"));
    }

    #[tokio::test]
    async fn issue_rejects_a_private_range_target_url() {
        let (_f, store) = make_store().await;
        let err = store
            .issue("alice", "http://10.0.0.5/hook", vec![])
            .await
            .expect_err("private range must be rejected");
        assert!(err.to_string().contains("SSRF"));
    }

    #[tokio::test]
    async fn issue_rejects_a_non_http_scheme() {
        let (_f, store) = make_store().await;
        let err = store
            .issue("alice", "ftp://example.com/hook", vec![])
            .await
            .expect_err("non-http(s) scheme must be rejected");
        assert!(err.to_string().contains("SSRF"));
    }

    // No "issue succeeds against a public URL" test here — that needs a real
    // DNS resolution (`validate_public_url` resolves the host), which would
    // make this unit test network-dependent/flaky. The route-level
    // integration test and the manual E2E validation cover the success path
    // against a real public endpoint instead.

    #[tokio::test]
    async fn active_matching_filters_by_event_type_and_owner() {
        let (_f, store) = make_store().await;
        // Can't `issue()` (SSRF-gated, needs DNS) in a unit test — insert
        // directly to exercise `active_matching`'s own filtering logic.
        {
            let conn = rusqlite::Connection::open(&store.db_path).unwrap();
            conn.execute_batch(SCHEMA_SQL).unwrap();
            conn.execute(
                "INSERT INTO webhook_subscriptions VALUES \
                 ('s1','alice','https://example.com/a','[\"task.created\"]','secret1',1,NULL), \
                 ('s2','alice','https://example.com/b','[]','secret2',2,NULL), \
                 ('s3','alice','https://example.com/c','[\"task.terminal\"]','secret3',3,NULL), \
                 ('s4','bob','https://example.com/d','[\"task.created\"]','secret4',4,NULL), \
                 ('s5','alice','https://example.com/e','[\"task.created\"]','secret5',5,999)",
                [],
            )
            .unwrap();
        }

        let matches = store.active_matching("alice", "task.created").await.unwrap();
        let ids: Vec<_> = matches.iter().map(|s| s.id.as_str()).collect();
        assert!(ids.contains(&"s1"), "exact event_type match");
        assert!(ids.contains(&"s2"), "empty event_types means 'all'");
        assert!(!ids.contains(&"s3"), "different event_type must not match");
        assert!(!ids.contains(&"s4"), "different owner must not match");
        assert!(!ids.contains(&"s5"), "revoked subscription must not match");
    }

    #[tokio::test]
    async fn revoke_is_owner_scoped_and_distinguishes_not_found_from_already_revoked() {
        let (_f, store) = make_store().await;
        {
            let conn = rusqlite::Connection::open(&store.db_path).unwrap();
            conn.execute_batch(SCHEMA_SQL).unwrap();
            conn.execute(
                "INSERT INTO webhook_subscriptions VALUES \
                 ('s1','alice','https://example.com/a','[]','secret',1,NULL)",
                [],
            )
            .unwrap();
        }

        let wrong_owner = store.revoke("bob", "s1").await.unwrap_err();
        assert_eq!(
            wrong_owner.downcast_ref::<RevokeError>(),
            Some(&RevokeError::NotFound)
        );

        store.revoke("alice", "s1").await.expect("revoke succeeds");

        let already = store.revoke("alice", "s1").await.unwrap_err();
        assert_eq!(
            already.downcast_ref::<RevokeError>(),
            Some(&RevokeError::AlreadyRevoked)
        );
    }
}
