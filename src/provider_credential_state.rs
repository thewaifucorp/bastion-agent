//! Host-owned persistence for `bastion_runtime::provider_auth`'s
//! `CredentialStateStore` port (Bastion Agent — persistir estado de
//! credencial de assinatura).
//!
//! Core 2 (`bastion-core` PR #7) delivers the PORT — `ProviderCredentialLifecycle`
//! coordinates refresh/backoff/single-flight, but persistence is deliberately
//! a host concern (its own module doc: "Nothing here writes a file, opens a
//! database, or touches a secret store"). Without an implementation the
//! lifecycle runs in memory only: cooldown, the consecutive-failure counter,
//! `ReauthRequired` and `Revoked` are all lost on restart — a revoked
//! credential goes back to being attempted, and a backoff that had grown
//! resets to its first step. This module closes that gap.
//!
//! ## Pattern (copied from `control_plane::credential`/`webhook_subscription`,
//! `adaptive::schedule`)
//! `tokio::task::spawn_blocking` around a synchronous `rusqlite::Connection`
//! (never held across an `.await`), `PRAGMA journal_mode=WAL; busy_timeout`,
//! `CREATE TABLE IF NOT EXISTS` schema init.
//!
//! ## `compare_and_swap`
//! The whole record (`CredentialLifecycleRecord`, already `Serialize`/
//! `Deserialize`) is stored as one opaque JSON TEXT column, same discipline
//! `kind_json`/`missed_json` use in `adaptive::schedule`. The record's own
//! type tree has no `HashMap`/`HashSet` anywhere (verified against
//! `bastion_types::provider_auth`/`bastion_runtime::provider_auth` at the
//! pinned commit), so two `serde_json::to_string` calls on equal values
//! always produce byte-identical output — safe to compare by exact string
//! equality in a `WHERE` clause, no need to decompose into columns.
//!
//! - `expected: None` (first write) is a conditional `INSERT OR IGNORE`: it
//!   succeeds only when no row exists yet for this `(owner_id, provider_id,
//!   profile_id)` — the primary key conflict is what makes a racing second
//!   "first write" a no-op rather than an overwrite.
//! - `expected: Some(record)` is an `UPDATE ... WHERE record_json = ?`: it
//!   succeeds only when the stored JSON is byte-identical to `expected`'s
//!   serialization. A concurrent writer that already moved the state (or
//!   deleted the row) makes this match zero rows.
//!
//! Either way, exactly ONE atomic SQLite statement executes per call — a
//! crash before it runs leaves the prior row untouched, and SQLite's own
//! implicit per-statement transaction means there is no way to observe a
//! torn write, satisfying BPLIFE-03 (fault injection recovers the last valid
//! record, never a partial one) without needing an explicit `BEGIN`/`COMMIT`.
//!
//! ## Scope and secrets
//! Every method is scoped by the full `ProviderAuthRef` (`owner_id` +
//! `provider_id` + `profile_id` together, in the primary key) — never
//! `profile_id` alone, since two owners may reuse the same profile name
//! (BPLIFE-04: two owners with the same `profile_id` have independent
//! records; `delete` of one never touches the other). No secret material
//! ever reaches this table: `CredentialLifecycleRecord` carries only
//! `{ state, attempt, observed_at }` — the credential itself lives in the
//! product's secret backend, reached through `ProviderCredentialRefresher`,
//! a completely separate port this module never touches.

use async_trait::async_trait;
use bastion_runtime::provider_auth::{CredentialLifecycleRecord, CredentialStateStore};
use bastion_types::provider_auth::ProviderAuthRef;
use rusqlite::{Connection, OptionalExtension};
use tokio::task::spawn_blocking;

fn open_conn(path: &str) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")?;
    Ok(conn)
}

const SCHEMA_SQL: &str = "
    PRAGMA journal_mode=WAL;
    PRAGMA busy_timeout=5000;

    CREATE TABLE IF NOT EXISTS provider_credential_state (
        owner_id     TEXT NOT NULL,
        provider_id  TEXT NOT NULL,
        profile_id   TEXT NOT NULL,
        record_json  TEXT NOT NULL,
        PRIMARY KEY (owner_id, provider_id, profile_id)
    );
";

/// SQLite-backed `CredentialStateStore` (Bastion Agent — persistir estado de
/// credencial de assinatura). See the module doc for the `compare_and_swap`
/// design.
pub struct SqliteCredentialStateStore {
    db_path: String,
}

impl SqliteCredentialStateStore {
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
}

#[async_trait]
impl CredentialStateStore for SqliteCredentialStateStore {
    async fn load(
        &self,
        reference: &ProviderAuthRef,
    ) -> anyhow::Result<Option<CredentialLifecycleRecord>> {
        let path = self.db_path.clone();
        let owner = reference.owner_id.clone();
        let provider = reference.provider_id.clone();
        let profile = reference.profile_id.clone();

        let record_json: Option<String> = spawn_blocking(move || {
            let conn = open_conn(&path)?;
            conn.query_row(
                "SELECT record_json FROM provider_credential_state \
                 WHERE owner_id = ?1 AND provider_id = ?2 AND profile_id = ?3",
                rusqlite::params![owner, provider, profile],
                |row| row.get(0),
            )
            .optional()
            .map_err(anyhow::Error::from)
        })
        .await??;

        match record_json {
            Some(json) => Ok(Some(serde_json::from_str(&json)?)),
            None => Ok(None),
        }
    }

    async fn compare_and_swap(
        &self,
        reference: &ProviderAuthRef,
        expected: Option<&CredentialLifecycleRecord>,
        next: &CredentialLifecycleRecord,
    ) -> anyhow::Result<bool> {
        let path = self.db_path.clone();
        let owner = reference.owner_id.clone();
        let provider = reference.provider_id.clone();
        let profile = reference.profile_id.clone();
        let next_json = serde_json::to_string(next)?;
        let expected_json = expected.map(serde_json::to_string).transpose()?;

        spawn_blocking(move || {
            let conn = open_conn(&path)?;
            let changed = match expected_json {
                // First write: succeed only if no row exists yet. The
                // primary key conflict IS the atomicity — a racing second
                // "first write" is a no-op, never a silent overwrite.
                None => conn.execute(
                    "INSERT OR IGNORE INTO provider_credential_state \
                     (owner_id, provider_id, profile_id, record_json) \
                     VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![owner, provider, profile, next_json],
                )?,
                // Subsequent write: succeed only if the stored record is
                // byte-identical to what the caller believes is current.
                // Matches zero rows if another writer already moved the
                // state, or deleted it — both are "not expected", not errors.
                Some(expected_json) => conn.execute(
                    "UPDATE provider_credential_state SET record_json = ?4 \
                     WHERE owner_id = ?1 AND provider_id = ?2 AND profile_id = ?3 \
                     AND record_json = ?5",
                    rusqlite::params![owner, provider, profile, next_json, expected_json],
                )?,
            };
            Ok::<_, anyhow::Error>(changed == 1)
        })
        .await?
    }

    async fn delete(&self, reference: &ProviderAuthRef) -> anyhow::Result<()> {
        let path = self.db_path.clone();
        let owner = reference.owner_id.clone();
        let provider = reference.provider_id.clone();
        let profile = reference.profile_id.clone();

        spawn_blocking(move || {
            let conn = open_conn(&path)?;
            conn.execute(
                "DELETE FROM provider_credential_state \
                 WHERE owner_id = ?1 AND provider_id = ?2 AND profile_id = ?3",
                rusqlite::params![owner, provider, profile],
            )?;
            Ok::<_, anyhow::Error>(())
        })
        .await?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bastion_runtime::provider_auth::CredentialLifecycleRecord;
    use bastion_types::provider_auth::ProviderAuthState;
    use std::sync::Arc;
    use tempfile::NamedTempFile;

    async fn make_store() -> (NamedTempFile, SqliteCredentialStateStore) {
        let f = NamedTempFile::new().expect("tempfile");
        let store = SqliteCredentialStateStore::new(f.path().to_str().unwrap());
        store.init_schema().await.expect("init schema");
        (f, store)
    }

    fn ready(observed_at: i64) -> CredentialLifecycleRecord {
        CredentialLifecycleRecord {
            state: ProviderAuthState::Ready,
            attempt: 0,
            observed_at,
        }
    }

    fn reference(owner: &str, profile: &str) -> ProviderAuthRef {
        ProviderAuthRef::new(owner, "anthropic", profile)
    }

    #[tokio::test]
    async fn load_on_a_never_written_reference_returns_none() {
        let (_f, store) = make_store().await;
        assert_eq!(store.load(&reference("alice", "work")).await.unwrap(), None);
    }

    #[tokio::test]
    async fn first_write_with_expected_none_succeeds_exactly_once() {
        let (_f, store) = make_store().await;
        let reference = reference("alice", "work");
        let record = ready(100);

        assert!(store
            .compare_and_swap(&reference, None, &record)
            .await
            .unwrap());
        assert_eq!(store.load(&reference).await.unwrap(), Some(record));
    }

    /// The trait's own contract: a losing CAS is `Ok(false)`, never an
    /// error, and the stored record is left exactly as it was.
    #[tokio::test]
    async fn cas_with_the_wrong_expected_returns_false_and_does_not_write() {
        let (_f, store) = make_store().await;
        let reference = reference("alice", "work");
        let original = ready(100);
        store
            .compare_and_swap(&reference, None, &original)
            .await
            .unwrap();

        let wrong_expected = CredentialLifecycleRecord {
            attempt: 99, // does not match what is actually stored
            ..original.clone()
        };
        let attempted_next = ready(200);
        let ok = store
            .compare_and_swap(&reference, Some(&wrong_expected), &attempted_next)
            .await
            .unwrap();

        assert!(!ok, "a mismatched expected must be rejected, never applied");
        assert_eq!(
            store.load(&reference).await.unwrap(),
            Some(original),
            "the record must be untouched after a losing CAS"
        );
    }

    #[tokio::test]
    async fn cas_with_the_correct_expected_updates_the_record() {
        let (_f, store) = make_store().await;
        let reference = reference("alice", "work");
        let original = ready(100);
        store
            .compare_and_swap(&reference, None, &original)
            .await
            .unwrap();

        let next = CredentialLifecycleRecord {
            state: ProviderAuthState::Revoked,
            attempt: 0,
            observed_at: 200,
        };
        let ok = store
            .compare_and_swap(&reference, Some(&original), &next)
            .await
            .unwrap();

        assert!(ok);
        assert_eq!(store.load(&reference).await.unwrap(), Some(next));
    }

    /// N concurrent CAS calls racing on the SAME `expected` — exactly one
    /// must win (BPLIFE-03's atomicity requirement, exercised for real
    /// against SQLite rather than asserted about the code).
    #[tokio::test]
    async fn n_concurrent_cas_calls_with_the_same_expected_exactly_one_wins() {
        let (_f, store) = make_store().await;
        let store = Arc::new(store);
        let reference = reference("alice", "work");
        let original = ready(100);
        store
            .compare_and_swap(&reference, None, &original)
            .await
            .unwrap();

        let mut handles = Vec::new();
        for i in 0..16u32 {
            let store = store.clone();
            let reference = reference.clone();
            let original = original.clone();
            handles.push(tokio::spawn(async move {
                let next = CredentialLifecycleRecord {
                    attempt: i,
                    observed_at: 200 + i as i64,
                    ..original.clone()
                };
                store
                    .compare_and_swap(&reference, Some(&original), &next)
                    .await
                    .unwrap()
            }));
        }

        let mut wins = 0;
        for h in handles {
            if h.await.unwrap() {
                wins += 1;
            }
        }
        assert_eq!(
            wins, 1,
            "exactly one racer must win a CAS on the same expected"
        );
    }

    /// BPLIFE-04: two owners reusing the same `profile_id` never collide —
    /// keying by `profile_id` alone would be wrong by construction.
    #[tokio::test]
    async fn two_owners_with_the_same_profile_id_have_independent_records() {
        let (_f, store) = make_store().await;
        let alice = reference("alice", "work");
        let bob = reference("bob", "work"); // same provider_id + profile_id, different owner

        store
            .compare_and_swap(&alice, None, &ready(100))
            .await
            .unwrap();
        store
            .compare_and_swap(&bob, None, &ready(200))
            .await
            .unwrap();

        assert_eq!(store.load(&alice).await.unwrap(), Some(ready(100)));
        assert_eq!(store.load(&bob).await.unwrap(), Some(ready(200)));

        store.delete(&alice).await.unwrap();
        assert_eq!(
            store.load(&alice).await.unwrap(),
            None,
            "alice's record must be gone"
        );
        assert_eq!(
            store.load(&bob).await.unwrap(),
            Some(ready(200)),
            "deleting alice's record must not touch bob's"
        );
    }

    #[tokio::test]
    async fn delete_on_an_absent_reference_is_a_harmless_no_op() {
        let (_f, store) = make_store().await;
        store.delete(&reference("alice", "work")).await.unwrap();
    }

    /// A record survives a process restart — a fresh store instance pointed
    /// at the SAME db file sees exactly what the previous instance wrote.
    #[tokio::test]
    async fn a_record_survives_across_store_instances_pointed_at_the_same_db() {
        let f = NamedTempFile::new().unwrap();
        let path = f.path().to_str().unwrap().to_string();

        let first = SqliteCredentialStateStore::new(path.clone());
        first.init_schema().await.unwrap();
        let reference = reference("alice", "work");
        first
            .compare_and_swap(&reference, None, &ready(100))
            .await
            .unwrap();
        drop(first);

        let second = SqliteCredentialStateStore::new(path);
        assert_eq!(second.load(&reference).await.unwrap(), Some(ready(100)));
    }
}
