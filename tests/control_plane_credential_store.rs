//! Integration-level round trip for `control_plane::credential`, exercised
//! through the public crate API (belt-and-suspenders over the inline
//! `#[cfg(test)]` unit tests in `src/control_plane/credential.rs`, which only
//! run within the crate itself). Mirrors
//! `adaptive::schedule`'s existing `add_list_revoke_round_trip_and_owner_isolation`
//! integration-test shape (US — External Control Plane and SDK, Phase 1).

use bastion::control_plane::credential::{RevokeError, SqliteCredentialStore};
use bastion::control_plane::scope::{require_scope, Scope, ScopeSet};
use tempfile::NamedTempFile;

async fn make_store() -> (NamedTempFile, SqliteCredentialStore) {
    let f = NamedTempFile::new().expect("tempfile");
    let path = f.path().to_str().expect("utf8 path").to_owned();
    let store = SqliteCredentialStore::new(path);
    store.init_schema().await.expect("init_schema");
    (f, store)
}

/// End-to-end story a future `/v1/*` auth middleware will actually run:
/// issue a scoped credential, authenticate a presented token against the
/// store, then gate an operation on the resolved credential's scopes —
/// crossing `credential` and `scope` together, which the inline unit tests
/// in each module (necessarily) don't.
#[tokio::test]
async fn issued_credential_authenticates_and_enforces_its_scopes() {
    let (_f, store) = make_store().await;

    let scopes = ScopeSet::new([Scope::TasksRead, Scope::TasksControl]);
    let (_id, token) = store
        .issue("alice", Some("paperclip"), scopes, "paperclip-prod")
        .await
        .expect("issue");

    let cred = store
        .authenticate(&token)
        .await
        .expect("authenticate")
        .expect("token resolves");

    // Granted scopes pass.
    assert!(require_scope(&cred.scopes, Scope::TasksRead).is_ok());
    assert!(require_scope(&cred.scopes, Scope::TasksControl).is_ok());

    // Ungranted scopes are denied — an issued token is never treated as an
    // implicit grant of everything.
    assert!(require_scope(&cred.scopes, Scope::TasksCreate).is_err());
    assert!(require_scope(&cred.scopes, Scope::WebhooksManage).is_err());
}

/// Two credentials issued for different owners never cross-authenticate or
/// cross-list, even when issued back-to-back against the same store/file.
#[tokio::test]
async fn credentials_are_isolated_across_owners() {
    let (_f, store) = make_store().await;

    let (_alice_id, alice_token) = store
        .issue(
            "alice",
            None,
            ScopeSet::new([Scope::TasksRead]),
            "alice-cred",
        )
        .await
        .expect("issue alice");
    let (_bob_id, bob_token) = store
        .issue("bob", None, ScopeSet::new([Scope::TasksCreate]), "bob-cred")
        .await
        .expect("issue bob");

    let alice_cred = store
        .authenticate(&alice_token)
        .await
        .expect("authenticate alice")
        .expect("alice token resolves");
    assert_eq!(alice_cred.owner_id, "alice");
    assert!(require_scope(&alice_cred.scopes, Scope::TasksCreate).is_err());

    let bob_cred = store
        .authenticate(&bob_token)
        .await
        .expect("authenticate bob")
        .expect("bob token resolves");
    assert_eq!(bob_cred.owner_id, "bob");
    assert!(require_scope(&bob_cred.scopes, Scope::TasksRead).is_err());

    let alice_list = store.list_for_owner("alice").await.expect("list alice");
    assert_eq!(alice_list.len(), 1);
    assert_eq!(alice_list[0].label, "alice-cred");

    let bob_list = store.list_for_owner("bob").await.expect("list bob");
    assert_eq!(bob_list.len(), 1);
    assert_eq!(bob_list[0].label, "bob-cred");
}

/// A full issue -> use -> revoke -> denied lifecycle, the shape a Phase 2
/// "revoke a leaked credential" operator flow will actually run.
#[tokio::test]
async fn revoked_credential_is_denied_end_to_end() {
    let (_f, store) = make_store().await;

    let (id, token) = store
        .issue(
            "alice",
            None,
            ScopeSet::new([Scope::TasksRead]),
            "short-lived",
        )
        .await
        .expect("issue");

    assert!(store.authenticate(&token).await.expect("auth").is_some());

    store.revoke("alice", &id).await.expect("revoke");

    assert!(
        store
            .authenticate(&token)
            .await
            .expect("authenticate after revoke must not error")
            .is_none(),
        "a revoked credential must never resolve, regardless of scopes it once held"
    );
}

/// Every doc comment in this module claims `control_plane_credentials` lives
/// on "the shared session-db file" alongside other stores'
/// tables (`sessions`, `schedules`, ...). This is the only test that actually
/// verifies that claim against a file that already has other tables on it,
/// rather than a store-under-test's own pristine tempfile — the gap a plain
/// tempfile round trip cannot catch (e.g. a table/index name collision, or
/// `init_schema`'s `CREATE TABLE IF NOT EXISTS` misbehaving on a non-empty
/// database).
#[tokio::test]
async fn credential_store_coexists_with_other_tables_on_a_shared_db_file() {
    let f = NamedTempFile::new().expect("tempfile");
    let path = f.path().to_str().expect("utf8 path").to_owned();

    // Simulate the file already having other stores' tables on it, as it
    // would in a real deployment (session store, adaptive::schedule's
    // `schedules` table) before the credential store's schema is added.
    {
        let conn = rusqlite::Connection::open(&path).expect("open");
        conn.execute_batch(
            "CREATE TABLE sessions (id TEXT PRIMARY KEY, owner_id TEXT NOT NULL);
             CREATE TABLE schedules (id TEXT PRIMARY KEY, owner_id TEXT NOT NULL);
             INSERT INTO sessions (id, owner_id) VALUES ('s1', 'alice');",
        )
        .expect("seed other tables");
    }

    let store = SqliteCredentialStore::new(path.clone());
    store
        .init_schema()
        .await
        .expect("init_schema must succeed on a non-empty, pre-populated db file");

    // init_schema is idempotent even with pre-existing unrelated tables present.
    store.init_schema().await.expect("second init_schema");

    let (_id, token) = store
        .issue(
            "alice",
            None,
            ScopeSet::new([Scope::TasksRead]),
            "shared-file-test",
        )
        .await
        .expect("issue");
    assert!(store.authenticate(&token).await.expect("auth").is_some());

    // The pre-existing tables/rows are untouched.
    let conn = rusqlite::Connection::open(&path).expect("reopen");
    let sessions_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
        .expect("sessions table still queryable");
    assert_eq!(sessions_count, 1);
}

/// Pins the two `RevokeError` variants across the public crate API (the
/// inline unit test in `credential.rs` covers the same contract from inside
/// the crate; this is the outside-the-crate view a Phase 2 HTTP handler
/// would actually match on).
#[tokio::test]
async fn revoke_error_is_downcastable_from_outside_the_crate() {
    let f = NamedTempFile::new().expect("tempfile");
    let path = f.path().to_str().expect("utf8 path").to_owned();
    let store = SqliteCredentialStore::new(path);
    store.init_schema().await.expect("init_schema");

    let err = store
        .revoke("alice", "does-not-exist")
        .await
        .expect_err("must error");
    assert_eq!(
        err.downcast_ref::<RevokeError>(),
        Some(&RevokeError::NotFound)
    );
}
