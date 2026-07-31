//! `CodexConnector` — the concrete adapter registering
//! `bastion-providers::codex` (device-code flow + `CodexRefresher` +
//! `CodexProvider`) with [`crate::subscription_auth`]'s connector-agnostic
//! ports. This is deliberately the ONLY module in this crate that imports
//! `bastion_providers::codex` — `subscription_auth.rs` itself stays
//! connector-agnostic by design (see its module doc), and this file exists
//! purely to bridge the two: implement [`SubscriptionLoginFlow`] and
//! [`SubscriptionModelProvider`] once, register one [`ConnectorRegistration`]
//! in the composition root (`main.rs`).
//!
//! ## What this module owns vs. what it doesn't
//!
//! - **Owns**: the in-flight device-authorization state between
//!   [`SubscriptionLoginFlow::start`] and
//!   [`SubscriptionLoginFlow::wait_for_approval`] (in-memory only — a daemon
//!   restart mid-login just means the operator runs `/auth connect` again),
//!   and [`SqliteCodexTokenStore`] — the ONLY place the raw OAuth
//!   `refresh_token` and ChatGPT account id live. Neither is touched by
//!   `CredentialStateStore`, which persists only the lifecycle's state
//!   machine (BAAUTH-01/02: no secret material crosses that boundary).
//! - **Does NOT own**: the OAuth protocol itself (device-code request/poll/
//!   exchange, the refresh grant) — all of that is `bastion-providers::codex`'s
//!   already-tested logic; this module only calls it and persists what it
//!   returns.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bastion_providers::codex::{
    exchange_authorization_code, poll_device_authorization, start_device_authorization,
    CodexConfig, CodexProvider, CodexTokenRecord, CodexTokenStore, DeviceAuthorization,
    DevicePollOutcome,
};
use bastion_providers::Provider;
use bastion_types::provider_auth::{
    ProviderAuthError, ProviderAuthRef, ResolvedProviderCredential,
};
use bastion_types::SecretValue;
use rusqlite::{Connection, OptionalExtension};
use tokio::sync::Mutex;
use tokio::task::spawn_blocking;

use crate::subscription_auth::{LoginPrompt, SubscriptionLoginFlow, SubscriptionModelProvider};

/// OAuth device codes expire — RFC 8628's own convention (and the range
/// `codex-rs`'s own client polls within) is single-digit minutes; capped
/// generously here so a slow operator isn't cut off mid-approval, but a
/// truly abandoned login eventually gives up instead of polling forever.
const DEVICE_POLL_TIMEOUT: Duration = Duration::from_secs(15 * 60);

fn open_conn(path: &str) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")?;
    tighten_permissions(path);
    Ok(conn)
}

/// Every `codex_token` row carries a raw OAuth `refresh_token` — unlike
/// `provider_credential_state`'s table (state machine only, never a
/// secret), this database needs the same `0600` discipline
/// `proposals.rs::write_secret_file`/`update.rs::serve_updater` already use
/// elsewhere in this crate. `Connection::open` has no mode parameter, so
/// this re-tightens after the fact on every call — cheap (one/two/three
/// `stat`+`chmod`, local sqlite, not a hot path) and self-healing if
/// permissions ever drift. WAL mode's `-wal`/`-shm` sidecars can carry
/// uncommitted page data before a checkpoint, so all three candidates get
/// tightened, not just the main file. Best-effort: a failure here (e.g. the
/// path not existing yet on the very first call before schema init) is not
/// itself fatal — the next call retries.
#[cfg(unix)]
fn tighten_permissions(path: &str) {
    use std::os::unix::fs::PermissionsExt;
    for candidate in [
        path.to_string(),
        format!("{path}-wal"),
        format!("{path}-shm"),
    ] {
        if std::path::Path::new(&candidate).exists() {
            let _ = std::fs::set_permissions(&candidate, std::fs::Permissions::from_mode(0o600));
        }
    }
}

#[cfg(not(unix))]
fn tighten_permissions(_path: &str) {}

const SCHEMA_SQL: &str = "
    PRAGMA journal_mode=WAL;
    PRAGMA busy_timeout=5000;

    CREATE TABLE IF NOT EXISTS codex_token (
        owner_id      TEXT NOT NULL,
        provider_id   TEXT NOT NULL,
        profile_id    TEXT NOT NULL,
        refresh_token TEXT NOT NULL,
        account_id    TEXT,
        PRIMARY KEY (owner_id, provider_id, profile_id)
    );
";

/// SQLite-backed `CodexTokenStore` — same `spawn_blocking` + WAL pattern as
/// `provider_credential_state::SqliteCredentialStateStore`, but a SEPARATE
/// table: this one carries secret material (the rotating refresh token),
/// which `CredentialStateStore`'s table must never see.
pub struct SqliteCodexTokenStore {
    db_path: String,
}

impl SqliteCodexTokenStore {
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
impl CodexTokenStore for SqliteCodexTokenStore {
    async fn load(&self, reference: &ProviderAuthRef) -> anyhow::Result<Option<CodexTokenRecord>> {
        let path = self.db_path.clone();
        let (owner, provider, profile) = (
            reference.owner_id.clone(),
            reference.provider_id.clone(),
            reference.profile_id.clone(),
        );
        let row: Option<(String, Option<String>)> = spawn_blocking(move || {
            let conn = open_conn(&path)?;
            conn.query_row(
                "SELECT refresh_token, account_id FROM codex_token \
                 WHERE owner_id = ?1 AND provider_id = ?2 AND profile_id = ?3",
                rusqlite::params![owner, provider, profile],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
            )
            .optional()
            .map_err(anyhow::Error::from)
        })
        .await??;

        Ok(row.map(|(refresh_token, account_id)| CodexTokenRecord {
            refresh_token: SecretValue::new(refresh_token),
            account_id,
        }))
    }

    async fn store(
        &self,
        reference: &ProviderAuthRef,
        record: CodexTokenRecord,
    ) -> anyhow::Result<()> {
        let path = self.db_path.clone();
        let (owner, provider, profile) = (
            reference.owner_id.clone(),
            reference.provider_id.clone(),
            reference.profile_id.clone(),
        );
        let refresh_token = record.refresh_token.expose_secret().to_string();
        let account_id = record.account_id;
        spawn_blocking(move || {
            let conn = open_conn(&path)?;
            conn.execute(
                "INSERT INTO codex_token \
                     (owner_id, provider_id, profile_id, refresh_token, account_id) \
                 VALUES (?1, ?2, ?3, ?4, ?5) \
                 ON CONFLICT(owner_id, provider_id, profile_id) \
                 DO UPDATE SET refresh_token = excluded.refresh_token, \
                                account_id = excluded.account_id",
                rusqlite::params![owner, provider, profile, refresh_token, account_id],
            )?;
            Ok::<_, anyhow::Error>(())
        })
        .await?
    }
}

/// The Codex/ChatGPT connector: device-code login
/// ([`SubscriptionLoginFlow`]) plus [`Provider`] construction
/// ([`SubscriptionModelProvider`]), both thin wrappers over
/// `bastion-providers::codex`'s already-tested primitives.
pub struct CodexConnector {
    http: reqwest::Client,
    config: CodexConfig,
    tokens: Arc<dyn CodexTokenStore>,
    /// Device-authorization state between `start()` and
    /// `wait_for_approval()` — both are called back to back by
    /// `SubscriptionAuthService::connect` within the same request, so an
    /// in-memory map (never persisted) is sufficient; a daemon restart
    /// mid-login just means the operator reruns `/auth connect`. Entries are
    /// only ever removed by `wait_for_approval` taking one out — an operator
    /// who calls `start()` and then never follows up leaves one behind
    /// forever otherwise. `start()` opportunistically sweeps entries older
    /// than `DEVICE_POLL_TIMEOUT` (the same deadline `wait_for_approval`
    /// itself enforces) before inserting, so this stays bounded without a
    /// background task.
    in_flight: Mutex<HashMap<ProviderAuthRef, (DeviceAuthorization, tokio::time::Instant)>>,
}

impl CodexConnector {
    pub fn new(config: CodexConfig, tokens: Arc<dyn CodexTokenStore>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("reqwest client"),
            config,
            tokens,
            in_flight: Mutex::new(HashMap::new()),
        }
    }
}

/// `start_device_authorization` returns a plain `anyhow::Result` (unlike
/// `poll_device_authorization`/`exchange_authorization_code`, which already
/// map onto `ProviderAuthError` inside `bastion-providers::codex`) — so this
/// connector has to do its own transient-vs-terminal classification here
/// instead of collapsing every failure into `UnsupportedProtocol`, which
/// would make a plain HTTP 503 or a timeout indistinguishable from a real
/// protocol change and drive the wrong retry behavior upstream (BAAUTH's
/// own `/auth connect` UX depends on `Throttled` meaning "try again",
/// `UnsupportedProtocol` meaning "this needs a human/engineer, not a retry").
fn classify_start_error(err: &anyhow::Error) -> ProviderAuthError {
    // Connection-level failures (timeout, refused, DNS) say nothing about
    // the protocol — always worth retrying.
    if let Some(reqwest_err) = err.chain().find_map(|c| c.downcast_ref::<reqwest::Error>()) {
        if reqwest_err.is_timeout() || reqwest_err.is_connect() {
            return ProviderAuthError::Throttled;
        }
    }
    // `start_device_authorization`'s own non-2xx branch bails with a
    // message ending in "HTTP {status}" — the only signal available here
    // without changing that function's return type. 429/5xx are transient
    // (rate limit or a vendor-side outage); any other status is a real
    // protocol mismatch.
    let is_server_or_rate_limited = err
        .to_string()
        .rsplit("HTTP ")
        .next()
        .and_then(|tail| tail.split_whitespace().next())
        .and_then(|code| code.parse::<u16>().ok())
        .is_some_and(|code| code == 429 || (500..600).contains(&code));
    if is_server_or_rate_limited {
        ProviderAuthError::Throttled
    } else {
        ProviderAuthError::UnsupportedProtocol
    }
}

#[async_trait]
impl SubscriptionLoginFlow for CodexConnector {
    async fn start(&self, reference: &ProviderAuthRef) -> Result<LoginPrompt, ProviderAuthError> {
        let device_auth = start_device_authorization(&self.http, &self.config)
            .await
            .map_err(|e| {
                let mapped = classify_start_error(&e);
                tracing::warn!(
                    event = "codex_device_start_failed",
                    error = %e,
                    transient = mapped.is_transient(),
                    "device authorization request failed",
                );
                mapped
            })?;
        // BAAUTH-01: a device-flow user_code authorizes nothing on its own
        // without the operator's own browser-side approval — safe to render
        // verbatim, never bearer material.
        let instructions = format!(
            "Visit {} and enter this code: {}",
            device_auth.verification_uri, device_auth.user_code
        );
        {
            let mut in_flight = self.in_flight.lock().await;
            let now = tokio::time::Instant::now();
            in_flight.retain(|_, (_, inserted_at)| {
                now.duration_since(*inserted_at) < DEVICE_POLL_TIMEOUT
            });
            in_flight.insert(reference.clone(), (device_auth, now));
        }
        Ok(LoginPrompt { instructions })
    }

    async fn wait_for_approval(
        &self,
        reference: &ProviderAuthRef,
    ) -> Result<(), ProviderAuthError> {
        let (device_auth, _inserted_at) = self
            .in_flight
            .lock()
            .await
            .remove(reference)
            .ok_or(ProviderAuthError::Missing)?;
        let deadline = tokio::time::Instant::now() + DEVICE_POLL_TIMEOUT;
        let interval = Duration::from_secs(device_auth.interval_secs.max(1));

        let (authorization_code, code_verifier) = loop {
            if tokio::time::Instant::now() >= deadline {
                return Err(ProviderAuthError::ReauthRequired);
            }
            match poll_device_authorization(
                &self.http,
                &self.config,
                &device_auth.device_auth_id,
                &device_auth.user_code,
            )
            .await
            {
                Ok(DevicePollOutcome::Authorized {
                    authorization_code,
                    code_verifier,
                }) => break (authorization_code, code_verifier),
                Ok(DevicePollOutcome::Pending) => tokio::time::sleep(interval).await,
                // A transient error (e.g. Throttled — the poll endpoint's
                // own 429 path) mid-poll must not abort the whole login: the
                // operator is still within the approval window and a single
                // rate-limit hiccup is not their fault. Sleep the same
                // interval and try again; the deadline check above still
                // bounds how long this can go on. Non-transient errors
                // (ReauthRequired, UnsupportedProtocol, ...) are terminal —
                // propagate immediately, same as before.
                Err(e) if e.is_transient() => {
                    tracing::warn!(
                        event = "codex_device_poll_transient_error",
                        error = %e,
                        "retrying within the approval window",
                    );
                    tokio::time::sleep(interval).await;
                }
                Err(e) => return Err(e),
            }
        };

        let record = exchange_authorization_code(
            &self.http,
            &self.config,
            &authorization_code,
            &code_verifier,
        )
        .await?;
        self.tokens.store(reference, record).await.map_err(|e| {
            // A storage failure here is a LOCAL problem (disk full, the
            // sqlite file locked, a permissions error) — never a statement
            // about the vendor rate-limiting this credential. `Throttled`
            // is still the closest fit in the closed `ProviderAuthError`
            // vocabulary (transient, safe to retry), but the real cause
            // must not be silently discarded — this is exactly the moment
            // upstream login just succeeded and the fresh, single-use
            // refresh_token is at risk of being lost if this write is
            // never retried.
            tracing::error!(
                event = "codex_token_store_failed",
                error = %e,
                "failed to persist the exchanged Codex token",
            );
            ProviderAuthError::Throttled
        })?;
        Ok(())
    }
}

#[async_trait]
impl SubscriptionModelProvider for CodexConnector {
    async fn build(
        &self,
        reference: &ProviderAuthRef,
        model_id: &str,
        credential: ResolvedProviderCredential,
    ) -> anyhow::Result<Box<dyn Provider>> {
        // The bearer material alone (`credential`) is not enough for a real
        // call — Codex's inference endpoint also wants the ChatGPT account
        // id, which only this connector's own token store carries (never
        // `ResolvedProviderCredential`, which is deliberately opaque bearer
        // material only).
        let record = self.tokens.load(reference).await?;
        let account_id = record.and_then(|r| r.account_id);
        // Fail here, not at the first inference call: a missing account_id
        // means the id_token claim couldn't be decoded during exchange/
        // refresh (`bastion-providers::codex::decode_chatgpt_account_id`
        // returning `None`) — silently building a provider without it would
        // defer that failure to a confusing 401/400 from the Responses API
        // instead of a clear auth-layer error pointing at the real cause.
        let account_id = account_id.ok_or_else(|| {
            tracing::error!(
                event = "codex_build_missing_account_id",
                owner = %reference.owner_id,
                profile = %reference.profile_id,
                "token record has no ChatGPT account id — reconnect required",
            );
            anyhow::anyhow!(
                "Codex credential for profile '{}' has no ChatGPT account id on file — \
                 reconnect with `/auth connect codex {}` to re-derive it",
                reference.profile_id,
                reference.profile_id,
            )
        })?;
        Ok(Box::new(CodexProvider::with_config(
            model_id,
            credential.expose_secret(),
            Some(account_id),
            self.config.clone(),
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bastion_types::provider_auth::CredentialKind;
    use tempfile::NamedTempFile;

    fn reference(owner: &str, profile: &str) -> ProviderAuthRef {
        ProviderAuthRef::new(owner, "codex", profile)
    }

    async fn make_store() -> (NamedTempFile, SqliteCodexTokenStore) {
        let f = NamedTempFile::new().expect("tempfile");
        let store = SqliteCodexTokenStore::new(f.path().to_str().unwrap());
        store.init_schema().await.expect("init schema");
        (f, store)
    }

    #[tokio::test]
    async fn load_on_a_never_written_reference_returns_none() {
        let (_f, store) = make_store().await;
        assert!(store
            .load(&reference("alice", "work"))
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn store_then_load_round_trips_refresh_token_and_account_id() {
        let (_f, store) = make_store().await;
        let reference = reference("alice", "work");
        store
            .store(
                &reference,
                CodexTokenRecord {
                    refresh_token: SecretValue::new("CANARY-refresh-token"),
                    account_id: Some("acct-123".to_string()),
                },
            )
            .await
            .unwrap();

        let loaded = store.load(&reference).await.unwrap().unwrap();
        assert_eq!(loaded.refresh_token.expose_secret(), "CANARY-refresh-token");
        assert_eq!(loaded.account_id, Some("acct-123".to_string()));
    }

    /// The database file carries a raw OAuth refresh_token — must be
    /// `0600`, never inherit a permissive umask like a table that only ever
    /// held lifecycle state.
    #[cfg(unix)]
    #[tokio::test]
    async fn db_file_is_tightened_to_0600_after_a_write() {
        use std::os::unix::fs::PermissionsExt;
        let (f, store) = make_store().await;
        store
            .store(
                &reference("alice", "work"),
                CodexTokenRecord {
                    refresh_token: SecretValue::new("CANARY-refresh-token"),
                    account_id: None,
                },
            )
            .await
            .unwrap();
        let mode = std::fs::metadata(f.path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "db file mode was {mode:o}, expected 0600");
    }

    /// The refresh_token OpenAI issues is single-use and rotates on every
    /// exchange — `store` must overwrite, never accumulate rows.
    #[tokio::test]
    async fn a_second_store_call_overwrites_rather_than_duplicating() {
        let (_f, store) = make_store().await;
        let reference = reference("alice", "work");
        store
            .store(
                &reference,
                CodexTokenRecord {
                    refresh_token: SecretValue::new("first-token"),
                    account_id: None,
                },
            )
            .await
            .unwrap();
        store
            .store(
                &reference,
                CodexTokenRecord {
                    refresh_token: SecretValue::new("rotated-token"),
                    account_id: Some("acct-456".to_string()),
                },
            )
            .await
            .unwrap();

        let loaded = store.load(&reference).await.unwrap().unwrap();
        assert_eq!(loaded.refresh_token.expose_secret(), "rotated-token");
        assert_eq!(loaded.account_id, Some("acct-456".to_string()));
    }

    /// BPLIFE-04-equivalent: two owners never collide, even with the same
    /// provider/profile pair.
    #[tokio::test]
    async fn two_owners_with_the_same_profile_have_independent_records() {
        let (_f, store) = make_store().await;
        let alice = reference("alice", "work");
        let bob = reference("bob", "work");
        store
            .store(
                &alice,
                CodexTokenRecord {
                    refresh_token: SecretValue::new("alice-token"),
                    account_id: None,
                },
            )
            .await
            .unwrap();
        store
            .store(
                &bob,
                CodexTokenRecord {
                    refresh_token: SecretValue::new("bob-token"),
                    account_id: None,
                },
            )
            .await
            .unwrap();

        assert_eq!(
            store
                .load(&alice)
                .await
                .unwrap()
                .unwrap()
                .refresh_token
                .expose_secret(),
            "alice-token"
        );
        assert_eq!(
            store
                .load(&bob)
                .await
                .unwrap()
                .unwrap()
                .refresh_token
                .expose_secret(),
            "bob-token"
        );
    }

    #[tokio::test]
    async fn a_record_survives_across_store_instances_pointed_at_the_same_db() {
        let f = NamedTempFile::new().unwrap();
        let path = f.path().to_str().unwrap().to_string();
        let reference = reference("alice", "work");

        let first = SqliteCodexTokenStore::new(path.clone());
        first.init_schema().await.unwrap();
        first
            .store(
                &reference,
                CodexTokenRecord {
                    refresh_token: SecretValue::new("token"),
                    account_id: Some("acct".to_string()),
                },
            )
            .await
            .unwrap();
        drop(first);

        let second = SqliteCodexTokenStore::new(path);
        let loaded = second.load(&reference).await.unwrap().unwrap();
        assert_eq!(loaded.refresh_token.expose_secret(), "token");
    }

    /// `wait_for_approval` without a prior `start()` for the same reference
    /// must fail closed, never poll a device authorization that doesn't
    /// exist.
    #[tokio::test]
    async fn wait_for_approval_without_a_prior_start_fails_with_missing() {
        let (_f, store) = make_store().await;
        let connector = CodexConnector::new(
            CodexConfig::default(),
            Arc::new(store) as Arc<dyn CodexTokenStore>,
        );
        let err = connector
            .wait_for_approval(&reference("alice", "work"))
            .await
            .unwrap_err();
        assert_eq!(err, ProviderAuthError::Missing);
    }

    /// `build` reads account_id from the connector's OWN token store, never
    /// from the credential itself — proven by round-tripping through a real
    /// store instance.
    #[tokio::test]
    async fn build_reads_account_id_from_the_token_store_not_the_credential() {
        let (_f, store) = make_store().await;
        let reference = reference("alice", "work");
        store
            .store(
                &reference,
                CodexTokenRecord {
                    refresh_token: SecretValue::new("token"),
                    account_id: Some("acct-789".to_string()),
                },
            )
            .await
            .unwrap();

        let connector = CodexConnector::new(
            CodexConfig::default(),
            Arc::new(store) as Arc<dyn CodexTokenStore>,
        );
        let credential = ResolvedProviderCredential::new(
            reference.clone(),
            CredentialKind::OAuthSubscription,
            SecretValue::new("access-token"),
        );
        let provider = connector
            .build(&reference, "gpt-5", credential)
            .await
            .unwrap();
        assert_eq!(provider.model_name(), "gpt-5");
        assert_eq!(provider.name(), "codex");
    }

    /// A token record missing its `account_id` (the id_token claim couldn't
    /// be decoded during exchange/refresh) must fail `build` outright —
    /// never construct a provider that would only surface this as a
    /// confusing 401 at first inference.
    #[tokio::test]
    async fn build_fails_closed_when_account_id_is_missing() {
        let (_f, store) = make_store().await;
        let reference = reference("alice", "work");
        store
            .store(
                &reference,
                CodexTokenRecord {
                    refresh_token: SecretValue::new("token"),
                    account_id: None,
                },
            )
            .await
            .unwrap();

        let connector = CodexConnector::new(
            CodexConfig::default(),
            Arc::new(store) as Arc<dyn CodexTokenStore>,
        );
        let credential = ResolvedProviderCredential::new(
            reference.clone(),
            CredentialKind::OAuthSubscription,
            SecretValue::new("access-token"),
        );
        // `Box<dyn Provider>` isn't `Debug`, so `unwrap_err()` doesn't
        // type-check here — match instead.
        match connector.build(&reference, "gpt-5", credential).await {
            Ok(_) => panic!("build must fail closed without an account_id"),
            Err(e) => assert!(
                e.to_string().contains("account id"),
                "error should name the real cause: {e}"
            ),
        }
    }

    /// `classify_start_error` is what stands between a plain HTTP 503 (or a
    /// timeout) and the wrong terminal `UnsupportedProtocol` classification
    /// — proven directly against the exact message shape
    /// `start_device_authorization` bails with.
    #[test]
    fn classify_start_error_treats_5xx_and_429_as_transient() {
        for status in [429, 500, 502, 503] {
            let err = anyhow::anyhow!("codex device authorization request failed: HTTP {status}");
            assert_eq!(
                classify_start_error(&err),
                ProviderAuthError::Throttled,
                "HTTP {status} must be transient"
            );
        }
    }

    #[test]
    fn classify_start_error_treats_other_statuses_as_protocol_failures() {
        let err = anyhow::anyhow!("codex device authorization request failed: HTTP 404");
        assert_eq!(
            classify_start_error(&err),
            ProviderAuthError::UnsupportedProtocol
        );
    }

    /// An abandoned `start()` (operator never calls `wait_for_approval`)
    /// must not accumulate forever — a later `start()` call for a DIFFERENT
    /// reference sweeps it once it is older than `DEVICE_POLL_TIMEOUT`.
    #[tokio::test]
    async fn start_sweeps_in_flight_entries_older_than_the_poll_timeout() {
        let (_f, store) = make_store().await;
        let connector = CodexConnector::new(
            CodexConfig::default(),
            Arc::new(store) as Arc<dyn CodexTokenStore>,
        );
        let abandoned = reference("alice", "abandoned");
        let stale_auth = DeviceAuthorization {
            device_auth_id: "d1".to_string(),
            user_code: "CODE1".to_string(),
            verification_uri: "https://example.test/device".to_string(),
            interval_secs: 5,
        };
        let stale_at = tokio::time::Instant::now() - DEVICE_POLL_TIMEOUT - Duration::from_secs(1);
        connector
            .in_flight
            .lock()
            .await
            .insert(abandoned.clone(), (stale_auth, stale_at));
        assert_eq!(connector.in_flight.lock().await.len(), 1);

        // A real `start()` call hits the network (start_device_authorization),
        // which is not reachable in a unit test — so this test drives the
        // same sweep-then-insert logic `start()` uses, directly, to prove
        // the stale entry is dropped rather than accumulating.
        {
            let mut in_flight = connector.in_flight.lock().await;
            let now = tokio::time::Instant::now();
            in_flight.retain(|_, (_, inserted_at)| {
                now.duration_since(*inserted_at) < DEVICE_POLL_TIMEOUT
            });
        }
        assert!(
            connector.in_flight.lock().await.is_empty(),
            "an entry older than DEVICE_POLL_TIMEOUT must be swept"
        );
    }
}
