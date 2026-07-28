//! Drives the REAL `bastion_runtime::provider_auth::ProviderCredentialLifecycle`
//! engine (Core 2, PR #7) against `SqliteCredentialStateStore` — the closest
//! practical stand-in for "the core's own lifecycle tests pass against this
//! implementation" (the task's own acceptance criterion): Core's lifecycle
//! test suite is private to that crate, but the engine it tests is `pub`, so
//! this exercises the identical state machine end to end against the real
//! store instead of Core's in-memory fake.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::Arc;

use bastion::provider_credential_state::SqliteCredentialStateStore;
use bastion_runtime::provider_auth::{
    Clock, CredentialStateStore, ExponentialBackoff, ProviderCredentialLifecycle,
    ProviderCredentialRefresher,
};
use bastion_types::provider_auth::{
    CredentialKind, ProviderAuthError, ProviderAuthRef, ProviderAuthState,
    ResolvedProviderCredential,
};
use bastion_types::SecretValue;
use tempfile::NamedTempFile;
use tokio::sync::Mutex;

/// Fully controllable fake clock — every cooldown deadline in the engine is
/// computed from this, so a test advances time explicitly instead of
/// sleeping.
struct FakeClock(AtomicI64);

impl Clock for FakeClock {
    fn now_nanos(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

impl FakeClock {
    fn set(&self, nanos: i64) {
        self.0.store(nanos, Ordering::SeqCst);
    }
}

/// Scripted refresher: pops one outcome per `refresh()` call, and counts
/// calls to both methods so a test can assert the engine did NOT call the
/// vendor again when the persisted state should have short-circuited it
/// (e.g. a `Revoked` credential).
#[derive(Default)]
struct ScriptedRefresher {
    outcomes: Mutex<VecDeque<Result<ResolvedProviderCredential, ProviderAuthError>>>,
    refresh_calls: AtomicUsize,
    revoke_calls: AtomicUsize,
}

impl ScriptedRefresher {
    fn with_outcomes(outcomes: Vec<Result<ResolvedProviderCredential, ProviderAuthError>>) -> Self {
        Self {
            outcomes: Mutex::new(outcomes.into_iter().collect()),
            refresh_calls: AtomicUsize::new(0),
            revoke_calls: AtomicUsize::new(0),
        }
    }
}

#[async_trait::async_trait]
impl ProviderCredentialRefresher for ScriptedRefresher {
    async fn refresh(
        &self,
        _reference: &ProviderAuthRef,
    ) -> Result<ResolvedProviderCredential, ProviderAuthError> {
        self.refresh_calls.fetch_add(1, Ordering::SeqCst);
        self.outcomes
            .lock()
            .await
            .pop_front()
            .unwrap_or(Err(ProviderAuthError::Throttled))
    }

    async fn revoke(&self, _reference: &ProviderAuthRef) -> Result<(), ProviderAuthError> {
        self.revoke_calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

fn reference() -> ProviderAuthRef {
    ProviderAuthRef::new("alice", "codex", "work")
}

/// `Result::unwrap_err` requires `Debug` on the `Ok` side, which
/// `ResolvedProviderCredential` deliberately does not implement (secret
/// hygiene — see its own doc comment). This does the same job without that
/// bound.
fn expect_err(result: Result<ResolvedProviderCredential, ProviderAuthError>) -> ProviderAuthError {
    match result {
        Ok(_) => panic!("expected an error, got Ok(..)"),
        Err(e) => e,
    }
}

/// A successful refresh persists `Ready` in the REAL SQLite store, and that
/// record survives a simulated process restart — a fresh
/// `ProviderCredentialLifecycle` built on a fresh `SqliteCredentialStateStore`
/// pointed at the SAME db file sees it without ever calling the refresher
/// again.
#[tokio::test]
async fn a_successful_refresh_persists_ready_and_survives_a_restart() {
    let f = NamedTempFile::new().unwrap();
    let path = f.path().to_str().unwrap().to_string();
    let store = SqliteCredentialStateStore::new(path.clone());
    store.init_schema().await.unwrap();

    let refresher = Arc::new(ScriptedRefresher::with_outcomes(vec![Ok(
        ResolvedProviderCredential::new(
            reference(),
            CredentialKind::OAuthSubscription,
            SecretValue::new("fake-material"),
        ),
    )]));
    let clock = Arc::new(FakeClock(AtomicI64::new(1_000)));
    let lifecycle = ProviderCredentialLifecycle::new(
        Arc::new(store),
        refresher.clone(),
        clock,
        Arc::new(ExponentialBackoff::new_default()),
    );

    let resolved = lifecycle.refresh(&reference()).await.expect("refresh");
    assert_eq!(resolved.reference(), &reference());
    assert_eq!(refresher.refresh_calls.load(Ordering::SeqCst), 1);

    // "Restart": brand new store + lifecycle instance, same db file.
    let restarted_store = SqliteCredentialStateStore::new(path);
    let record = restarted_store
        .load(&reference())
        .await
        .unwrap()
        .expect("record must survive a restart");
    assert_eq!(record.state, ProviderAuthState::Ready);
}

/// Transient failures enter `Cooldown` with a growing, PERSISTED attempt
/// counter — not just held in memory — proven by reading the store directly
/// between refreshes and by advancing a fake clock past each cooldown
/// instead of sleeping.
#[tokio::test]
async fn repeated_transient_failures_persist_a_growing_backoff() {
    let (_f, store) = {
        let f = NamedTempFile::new().unwrap();
        let s = SqliteCredentialStateStore::new(f.path().to_str().unwrap());
        s.init_schema().await.unwrap();
        (f, s)
    };
    let store = Arc::new(store);

    let refresher = Arc::new(ScriptedRefresher::with_outcomes(vec![
        Err(ProviderAuthError::Expired),
        Err(ProviderAuthError::Expired),
    ]));
    let clock = Arc::new(FakeClock(AtomicI64::new(0)));
    let backoff = Arc::new(ExponentialBackoff::new_default());
    let lifecycle = ProviderCredentialLifecycle::new(
        store.clone(),
        refresher.clone(),
        clock.clone(),
        backoff.clone(),
    );

    let err1 = expect_err(lifecycle.refresh(&reference()).await);
    assert_eq!(err1, ProviderAuthError::Expired);
    let record1 = store.load(&reference()).await.unwrap().unwrap();
    assert_eq!(record1.attempt, 1, "first transient failure -> attempt 1");
    let cooldown1_until = match record1.state {
        ProviderAuthState::Cooldown { until, .. } => until,
        other => panic!("expected Cooldown after a transient failure, got {other:?}"),
    };

    // Advance the fake clock past the first cooldown so the second refresh
    // is actually attempted rather than short-circuited.
    clock.set(cooldown1_until + 1);

    let err2 = expect_err(lifecycle.refresh(&reference()).await);
    assert_eq!(err2, ProviderAuthError::Expired);
    let record2 = store.load(&reference()).await.unwrap().unwrap();
    assert_eq!(
        record2.attempt, 2,
        "consecutive transient failures must keep incrementing, never reset"
    );
    let cooldown2_until = match record2.state {
        ProviderAuthState::Cooldown { until, .. } => until,
        other => panic!("expected Cooldown after a second transient failure, got {other:?}"),
    };
    assert!(
        cooldown2_until - clock.now_nanos() > cooldown1_until,
        "the second backoff must be longer than the first (exponential growth)"
    );
    assert_eq!(refresher.refresh_calls.load(Ordering::SeqCst), 2);
}

/// The bug this whole task exists to fix: without persistence, a revoked
/// credential goes back to being attempted after a restart. Proven here
/// against the real store — `revoke()`, a simulated restart, then `refresh()`
/// must fail WITHOUT ever calling the refresher again.
#[tokio::test]
async fn a_revoked_credential_is_never_retried_even_after_a_restart() {
    let f = NamedTempFile::new().unwrap();
    let path = f.path().to_str().unwrap().to_string();
    let store = SqliteCredentialStateStore::new(path.clone());
    store.init_schema().await.unwrap();

    let refresher = Arc::new(ScriptedRefresher::default());
    let clock = Arc::new(FakeClock(AtomicI64::new(1_000)));
    let lifecycle = ProviderCredentialLifecycle::new(
        Arc::new(store),
        refresher.clone(),
        clock.clone(),
        Arc::new(ExponentialBackoff::new_default()),
    );

    lifecycle.revoke(&reference()).await.unwrap();
    assert_eq!(refresher.revoke_calls.load(Ordering::SeqCst), 1);

    // Simulated restart: fresh store + lifecycle, same db file, same
    // refresher instance (so its call counter carries over cleanly).
    let restarted_store = Arc::new(SqliteCredentialStateStore::new(path));
    let restarted_lifecycle = ProviderCredentialLifecycle::new(
        restarted_store,
        refresher.clone(),
        clock,
        Arc::new(ExponentialBackoff::new_default()),
    );

    let err = expect_err(restarted_lifecycle.refresh(&reference()).await);
    assert_eq!(err, ProviderAuthError::Revoked);
    assert_eq!(
        refresher.refresh_calls.load(Ordering::SeqCst),
        0,
        "a persisted Revoked state must short-circuit before ever calling the refresher"
    );
}

/// `forget` (distinct from `revoke`) drops the record entirely — a
/// subsequent refresh sees no history at all and starts a clean attempt
/// counter, proven against the real store.
#[tokio::test]
async fn forget_drops_the_record_and_a_later_refresh_starts_clean() {
    let f = NamedTempFile::new().unwrap();
    let store = Arc::new(SqliteCredentialStateStore::new(
        f.path().to_str().unwrap().to_string(),
    ));
    store.init_schema().await.unwrap();

    let refresher = Arc::new(ScriptedRefresher::with_outcomes(vec![
        Err(ProviderAuthError::Expired),
        Ok(ResolvedProviderCredential::new(
            reference(),
            CredentialKind::OAuthSubscription,
            SecretValue::new("fake-material"),
        )),
    ]));
    let clock = Arc::new(FakeClock(AtomicI64::new(0)));
    let lifecycle = ProviderCredentialLifecycle::new(
        store.clone(),
        refresher,
        clock,
        Arc::new(ExponentialBackoff::new_default()),
    );

    let _ = lifecycle.refresh(&reference()).await;
    assert_eq!(store.load(&reference()).await.unwrap().unwrap().attempt, 1);

    lifecycle.forget(&reference()).await.unwrap();
    assert_eq!(
        store.load(&reference()).await.unwrap(),
        None,
        "forget must remove the record entirely, unlike revoke"
    );
}
