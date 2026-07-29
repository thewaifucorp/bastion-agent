//! Bastion Agent — oferecer login e gestão de assinaturas (BAAUTH-01..05).
//!
//! Core delivers a neutral mechanism ([`bastion_runtime::provider_auth`]):
//! given a connector's [`bastion_runtime::provider_auth::ProviderCredentialRefresher`],
//! [`bastion_runtime::provider_auth::ProviderCredentialLifecycle`] coordinates
//! refresh/backoff/single-flight and [`crate::provider_credential_state::SqliteCredentialStateStore`]
//! persists its state. What Core deliberately does NOT do is conduct an OAuth
//! flow, map an owner to a credential, or make reauthentication/revocation
//! legible across daemon/TUI/CLI — that product-level service is this module
//! (BAAUTH-03: one implementation, three surfaces, exactly the pattern
//! `agent::schedule_command` already establishes for `/schedule`).
//!
//! ## Division of labor
//!
//! - [`SubscriptionLoginFlow`] is the connector-owned half this module does
//!   NOT implement: conducting the interactive login (device code, PKCE,
//!   whatever the connector's [`bastion_types::provider_catalog::ProviderAuthFlow`]
//!   declares) and durably storing whatever raw material its own
//!   `ProviderCredentialRefresher` will need. This service only calls it.
//! - [`SubscriptionAuthService`] owns the product-level bookkeeping: which
//!   `(owner, provider, profile)` triples exist and what an operator called
//!   them ([`SubscriptionProfileRecord`], no secret), and drives connect/list/
//!   status/disconnect against a per-provider [`ConnectorRegistration`].
//!
//! ## No connector is registered yet
//!
//! This service is connector-agnostic by construction — it depends only on
//! Core's `provider_auth` contracts (already in this repo's pinned
//! `bastion-core` rev) and a local, non-secret label table. The Codex/ChatGPT
//! connector itself (`bastion-providers::codex`) lives on a separate,
//! not-yet-merged `bastion-core` branch — wiring it in is a `SubscriptionLoginFlow`
//! adapter over its already-built device-code functions plus one
//! `ConnectorRegistration` entry in the composition root (`main.rs`), added
//! once this repo's `bastion-providers` pin includes it. Until then,
//! `connectors` is empty and `connect` fails with a clear "unknown provider"
//! message rather than silently doing nothing.
//!
//! ## Secret discipline (BAAUTH-01/02)
//!
//! No method here ever sees or returns raw credential material.
//! [`SubscriptionLoginFlow::wait_for_approval`] returns only a completion
//! marker — the connector implementation stores real material into ITS OWN
//! token store as a side effect, invisible to this module. `connect` proves
//! the credential works by calling `ProviderCredentialLifecycle::refresh`
//! (which resolves a [`bastion_types::provider_auth::ResolvedProviderCredential`]
//! internally) but never reads `expose_secret()` on the result — the record
//! this module returns and persists carries only `state`/`observed_at`.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use bastion_providers::Provider;
use bastion_runtime::provider_auth::ProviderCredentialLifecycle;
use bastion_types::provider_auth::{
    ProviderAuthError, ProviderAuthRef, ProviderAuthState, ResolvedProviderCredential,
};
use rusqlite::Connection;
use tokio::task::spawn_blocking;

pub const DEFAULT_PROFILE_ID: &str = "default";

/// What a connector's interactive login shows the operator BEFORE polling —
/// rendered verbatim by the command layer. A device-flow `user_code` is not
/// bearer material (it authorizes nothing without the operator's own
/// browser-side approval), so surfacing it here is safe by the same logic
/// every OAuth device-code UX relies on; a connector must never put anything
/// stronger than that in this field.
pub struct LoginPrompt {
    pub instructions: String,
}

/// One connector's interactive-login half — the counterpart to
/// [`bastion_runtime::provider_auth::ProviderCredentialRefresher`] (which
/// renews an ALREADY-established credential; this trait establishes the
/// first one). Implemented once per connector.
#[async_trait]
pub trait SubscriptionLoginFlow: Send + Sync {
    /// Begin login for `reference`. Must not block waiting for approval —
    /// that is [`Self::wait_for_approval`]'s job, so the caller can render
    /// the prompt before polling starts.
    async fn start(&self, reference: &ProviderAuthRef) -> Result<LoginPrompt, ProviderAuthError>;

    /// Block (polling internally, on whatever cadence/timeout the connector
    /// defines) until the operator approves, is denied, or it times out. On
    /// `Ok(())` the connector has ALREADY durably stored whatever material
    /// its `ProviderCredentialRefresher::refresh` will need — this method
    /// returns no material itself (BAAUTH-01/02).
    async fn wait_for_approval(&self, reference: &ProviderAuthRef)
        -> Result<(), ProviderAuthError>;
}

/// The other connector-owned half: turning a resolved credential into an
/// actual inference [`Provider`] — the BACOMP-01 seam. Kept as its own trait
/// (rather than a method on [`SubscriptionLoginFlow`]) because building a
/// provider needs `model_id` and, for connectors whose inference call needs
/// more than the bearer material `ResolvedProviderCredential` carries (e.g.
/// Codex's `ChatGPT-Account-Id` header), a fresh read of the connector's own
/// token storage keyed by the same `reference` — async, so this cannot be a
/// plain closure field.
#[async_trait]
pub trait SubscriptionModelProvider: Send + Sync {
    /// Build a `Provider` for `model_id`, authenticated with `credential`.
    /// `reference` is passed through (not just `credential`) so an
    /// implementation may re-read its OWN connector-specific storage (e.g.
    /// an account id) keyed the same way — never anything this module's own
    /// tables carry, which stay secret-free and connector-agnostic.
    async fn build(
        &self,
        reference: &ProviderAuthRef,
        model_id: &str,
        credential: ResolvedProviderCredential,
    ) -> anyhow::Result<Box<dyn Provider>>;
}

/// One connector's registration: its login flow, the lifecycle wrapping its
/// refresher, and its provider factory. One [`ProviderCredentialLifecycle`]
/// per connector — the Core type is constructed with exactly one
/// [`bastion_runtime::provider_auth::ProviderCredentialRefresher`], so two
/// connectors sharing a lifecycle instance is not an option; they DO share
/// the same underlying [`crate::provider_credential_state::SqliteCredentialStateStore`]
/// table (already scoped by the full `ProviderAuthRef`, provider included).
#[derive(Clone)]
pub struct ConnectorRegistration {
    pub flow: Arc<dyn SubscriptionLoginFlow>,
    pub lifecycle: Arc<ProviderCredentialLifecycle>,
    pub provider_factory: Arc<dyn SubscriptionModelProvider>,
}

/// One `(owner, provider, profile)` triple as `auth list`/`auth status`
/// render it. No secret, ever — `state`/`observed_at` are `None` exactly
/// when the profile is labeled but its connector isn't registered (never
/// fabricated as "unknown" meaning something more specific).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscriptionProfileRecord {
    pub owner_id: String,
    pub provider_id: String,
    pub profile_id: String,
    pub label: String,
    pub state: Option<ProviderAuthState>,
    pub observed_at: Option<i64>,
}

fn open_conn(path: &str) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")?;
    Ok(conn)
}

const SCHEMA_SQL: &str = "
    PRAGMA journal_mode=WAL;
    PRAGMA busy_timeout=5000;

    CREATE TABLE IF NOT EXISTS subscription_profile_label (
        owner_id     TEXT NOT NULL,
        provider_id  TEXT NOT NULL,
        profile_id   TEXT NOT NULL,
        label        TEXT NOT NULL,
        PRIMARY KEY (owner_id, provider_id, profile_id)
    );
";

/// Host-owned, non-secret bookkeeping: which profiles exist and what an
/// operator called them. No lifecycle state lives here (read live from
/// `ProviderCredentialLifecycle` instead, which is the source of truth) and
/// no secret material ever does — this table exists purely so `auth
/// list`/`auth status` have labels to enumerate.
struct SqliteProfileLabelStore {
    db_path: String,
}

impl SqliteProfileLabelStore {
    fn new(db_path: impl Into<String>) -> Self {
        Self {
            db_path: db_path.into(),
        }
    }

    async fn init_schema(&self) -> anyhow::Result<()> {
        let path = self.db_path.clone();
        spawn_blocking(move || {
            let conn = open_conn(&path)?;
            conn.execute_batch(SCHEMA_SQL)?;
            Ok::<_, anyhow::Error>(())
        })
        .await?
    }

    async fn upsert(
        &self,
        owner_id: &str,
        provider_id: &str,
        profile_id: &str,
        label: &str,
    ) -> anyhow::Result<()> {
        let path = self.db_path.clone();
        let (owner_id, provider_id, profile_id, label) = (
            owner_id.to_string(),
            provider_id.to_string(),
            profile_id.to_string(),
            label.to_string(),
        );
        spawn_blocking(move || {
            let conn = open_conn(&path)?;
            conn.execute(
                "INSERT INTO subscription_profile_label \
                     (owner_id, provider_id, profile_id, label) \
                 VALUES (?1, ?2, ?3, ?4) \
                 ON CONFLICT(owner_id, provider_id, profile_id) DO UPDATE SET label = excluded.label",
                rusqlite::params![owner_id, provider_id, profile_id, label],
            )?;
            Ok::<_, anyhow::Error>(())
        })
        .await?
    }

    async fn delete(
        &self,
        owner_id: &str,
        provider_id: &str,
        profile_id: &str,
    ) -> anyhow::Result<()> {
        let path = self.db_path.clone();
        let (owner_id, provider_id, profile_id) = (
            owner_id.to_string(),
            provider_id.to_string(),
            profile_id.to_string(),
        );
        spawn_blocking(move || {
            let conn = open_conn(&path)?;
            conn.execute(
                "DELETE FROM subscription_profile_label \
                 WHERE owner_id = ?1 AND provider_id = ?2 AND profile_id = ?3",
                rusqlite::params![owner_id, provider_id, profile_id],
            )?;
            Ok::<_, anyhow::Error>(())
        })
        .await?
    }

    async fn list_for_owner(
        &self,
        owner_id: &str,
    ) -> anyhow::Result<Vec<(String, String, String)>> {
        let path = self.db_path.clone();
        let owner_id = owner_id.to_string();
        spawn_blocking(move || {
            let conn = open_conn(&path)?;
            let mut stmt = conn.prepare(
                "SELECT provider_id, profile_id, label FROM subscription_profile_label \
                 WHERE owner_id = ?1 ORDER BY provider_id, profile_id",
            )?;
            let rows = stmt
                .query_map(rusqlite::params![owner_id], |row| {
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

    /// Profile ids are unique per `(owner, provider)`, not globally — two
    /// providers may reuse the same profile name for the same owner. Returns
    /// every provider that has a profile with this exact id.
    async fn find_by_profile(
        &self,
        owner_id: &str,
        profile_id: &str,
    ) -> anyhow::Result<Vec<(String, String)>> {
        let path = self.db_path.clone();
        let (owner_id, profile_id) = (owner_id.to_string(), profile_id.to_string());
        spawn_blocking(move || {
            let conn = open_conn(&path)?;
            let mut stmt = conn.prepare(
                "SELECT provider_id, label FROM subscription_profile_label \
                 WHERE owner_id = ?1 AND profile_id = ?2 ORDER BY provider_id",
            )?;
            let rows = stmt
                .query_map(rusqlite::params![owner_id, profile_id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok::<_, anyhow::Error>(rows)
        })
        .await?
    }
}

/// The product-level subscription login/auth service (BAAUTH-01..05). Build
/// once in the composition root, share via `Arc` across CLI, console/TUI and
/// channel dispatch — see the module doc.
pub struct SubscriptionAuthService {
    labels: SqliteProfileLabelStore,
    connectors: HashMap<String, ConnectorRegistration>,
}

impl SubscriptionAuthService {
    pub fn new(
        db_path: impl Into<String>,
        connectors: HashMap<String, ConnectorRegistration>,
    ) -> Self {
        Self {
            labels: SqliteProfileLabelStore::new(db_path),
            connectors,
        }
    }

    pub async fn init_schema(&self) -> anyhow::Result<()> {
        self.labels.init_schema().await
    }

    fn available_providers(&self) -> String {
        if self.connectors.is_empty() {
            return "none registered yet".to_string();
        }
        let mut names: Vec<&str> = self.connectors.keys().map(String::as_str).collect();
        names.sort_unstable();
        names.join(", ")
    }

    /// BAAUTH-01: conducts the connector's declared login flow, then proves
    /// the resulting credential actually resolves (`lifecycle.refresh`, which
    /// is also what first seeds `CredentialStateStore` to `Ready` — no state
    /// exists for a reference until a refresh has run at least once) before
    /// the profile is considered connected. A cancelled/denied/failed flow
    /// never reaches the label write, so it leaves nothing behind to clean up.
    pub async fn connect(
        &self,
        owner_id: &str,
        provider_id: &str,
        profile_id: &str,
        label: &str,
    ) -> anyhow::Result<SubscriptionProfileRecord> {
        let registration = self.connectors.get(provider_id).ok_or_else(|| {
            anyhow::anyhow!(
                "unknown provider '{provider_id}'. Available: {}",
                self.available_providers()
            )
        })?;
        let reference = ProviderAuthRef::new(owner_id, provider_id, profile_id);

        let prompt = registration
            .flow
            .start(&reference)
            .await
            .map_err(anyhow::Error::from)?;
        tracing::info!(
            event = "subscription_login_prompt",
            owner = %owner_id,
            provider = %provider_id,
            profile = %profile_id,
            instructions = %prompt.instructions,
        );

        registration
            .flow
            .wait_for_approval(&reference)
            .await
            .map_err(anyhow::Error::from)?;

        registration
            .lifecycle
            .refresh(&reference)
            .await
            .map_err(anyhow::Error::from)?;

        self.labels
            .upsert(owner_id, provider_id, profile_id, label)
            .await?;
        self.read_one(owner_id, provider_id, profile_id, label)
            .await
    }

    pub async fn list(&self, owner_id: &str) -> anyhow::Result<Vec<SubscriptionProfileRecord>> {
        let rows = self.labels.list_for_owner(owner_id).await?;
        let mut out = Vec::with_capacity(rows.len());
        for (provider_id, profile_id, label) in rows {
            out.push(
                self.read_one(owner_id, &provider_id, &profile_id, &label)
                    .await?,
            );
        }
        Ok(out)
    }

    /// Every provider with a profile named `profile_id` for this owner — a
    /// bare profile id is not globally unique (two providers may reuse the
    /// same name), so this may return more than one match.
    pub async fn status(
        &self,
        owner_id: &str,
        profile_id: &str,
    ) -> anyhow::Result<Vec<SubscriptionProfileRecord>> {
        let rows = self.labels.find_by_profile(owner_id, profile_id).await?;
        let mut out = Vec::with_capacity(rows.len());
        for (provider_id, label) in rows {
            out.push(
                self.read_one(owner_id, &provider_id, profile_id, &label)
                    .await?,
            );
        }
        Ok(out)
    }

    /// BAAUTH-05: revokes upstream when the connector is registered and
    /// supports it, then removes only the requested reference — never
    /// another profile, another owner's, or (when the vendor revoke call
    /// itself fails) skips the local removal because of it. Local state
    /// still moves and the label is still dropped; a failed upstream
    /// revocation is logged, not propagated as this call's error, mirroring
    /// `ProviderCredentialLifecycle::revoke`'s own contract that local
    /// intent wins over vendor support.
    pub async fn disconnect(
        &self,
        owner_id: &str,
        provider_id: &str,
        profile_id: &str,
    ) -> anyhow::Result<()> {
        let reference = ProviderAuthRef::new(owner_id, provider_id, profile_id);
        if let Some(registration) = self.connectors.get(provider_id) {
            if let Err(e) = registration.lifecycle.revoke(&reference).await {
                tracing::warn!(
                    event = "subscription_disconnect_upstream_revoke_failed",
                    owner = %owner_id,
                    provider = %provider_id,
                    profile = %profile_id,
                    error = %e,
                );
            }
            registration.lifecycle.forget(&reference).await?;
        }
        self.labels.delete(owner_id, provider_id, profile_id).await
    }

    /// Whether `provider_id` names a registered connector — the check
    /// `/model <provider_id>/<model_id>[@profile]` parsing uses to decide
    /// whether a `/`-containing model string is a subscription selection
    /// rather than an ordinary provider-prefixed model id (OpenRouter's
    /// `vendor/model` slugs, most notably) — never guessed from the string
    /// shape alone.
    pub fn is_registered(&self, provider_id: &str) -> bool {
        self.connectors.contains_key(provider_id)
    }

    /// BACOMP-01: build a native-loop [`Provider`] authenticated by this
    /// owner's `(provider_id, profile_id)` subscription. Refreshes through
    /// the connector's lifecycle first — the SAME call `connect` uses to
    /// prove a credential works — so a revoked/expired/cooldown credential
    /// surfaces as the lifecycle's own typed `ProviderAuthError` (mapped to
    /// `anyhow`) and ends here: BACOMP-03 is satisfied by construction,
    /// since this method has no other reference to try and returns instead
    /// of ever selecting a different provider/profile/owner.
    pub async fn resolve_provider(
        &self,
        owner_id: &str,
        provider_id: &str,
        profile_id: &str,
        model_id: &str,
    ) -> anyhow::Result<Box<dyn Provider>> {
        let registration = self.connectors.get(provider_id).ok_or_else(|| {
            anyhow::anyhow!(
                "unknown provider '{provider_id}'. Available: {}",
                self.available_providers()
            )
        })?;
        let reference = ProviderAuthRef::new(owner_id, provider_id, profile_id);
        let credential = registration
            .lifecycle
            .refresh(&reference)
            .await
            .map_err(anyhow::Error::from)?;
        registration
            .provider_factory
            .build(&reference, model_id, credential)
            .await
    }

    async fn read_one(
        &self,
        owner_id: &str,
        provider_id: &str,
        profile_id: &str,
        label: &str,
    ) -> anyhow::Result<SubscriptionProfileRecord> {
        let reference = ProviderAuthRef::new(owner_id, provider_id, profile_id);
        let (state, observed_at) = match self.connectors.get(provider_id) {
            Some(registration) => match registration.lifecycle.state(&reference).await? {
                Some(record) => (Some(record.state), Some(record.observed_at)),
                None => (None, None),
            },
            None => (None, None),
        };
        Ok(SubscriptionProfileRecord {
            owner_id: owner_id.to_string(),
            provider_id: provider_id.to_string(),
            profile_id: profile_id.to_string(),
            label: label.to_string(),
            state,
            observed_at,
        })
    }
}

/// Test doubles shared with `agent::auth_command`'s own tests — `pub(crate)`
/// rather than nested in `mod tests` so both call sites can build a
/// `SubscriptionAuthService` without a live connector.
#[cfg(test)]
pub(crate) mod fakes {
    use super::*;
    use bastion_runtime::provider_auth::{
        ExponentialBackoff, ProviderCredentialRefresher, SystemClock,
    };
    use bastion_types::provider_auth::{CredentialKind, ResolvedProviderCredential};
    use bastion_types::SecretValue;

    pub(crate) struct FakeFlow {
        pub(crate) outcome: Result<(), ProviderAuthError>,
        pub(crate) prompt: String,
    }

    #[async_trait]
    impl SubscriptionLoginFlow for FakeFlow {
        async fn start(
            &self,
            _reference: &ProviderAuthRef,
        ) -> Result<LoginPrompt, ProviderAuthError> {
            Ok(LoginPrompt {
                instructions: self.prompt.clone(),
            })
        }

        async fn wait_for_approval(
            &self,
            _reference: &ProviderAuthRef,
        ) -> Result<(), ProviderAuthError> {
            self.outcome
        }
    }

    pub(crate) struct FakeRefresher {
        pub(crate) material: &'static str,
        /// `Err` makes every `refresh()` call fail with this reason instead
        /// of resolving a credential — used to test the "refresh itself
        /// fails" path (BACOMP-03), distinct from `FakeFlow`'s
        /// `outcome` (which only affects the interactive `connect` flow).
        pub(crate) refresh_outcome: Result<(), ProviderAuthError>,
    }

    #[async_trait]
    impl ProviderCredentialRefresher for FakeRefresher {
        async fn refresh(
            &self,
            reference: &ProviderAuthRef,
        ) -> Result<ResolvedProviderCredential, ProviderAuthError> {
            self.refresh_outcome?;
            Ok(ResolvedProviderCredential::new(
                reference.clone(),
                CredentialKind::OAuthSubscription,
                SecretValue::new(self.material),
            ))
        }

        async fn revoke(&self, _reference: &ProviderAuthRef) -> Result<(), ProviderAuthError> {
            Ok(())
        }
    }

    /// Minimal stand-in `Provider` — never actually called by these tests,
    /// only returned, so every inference method is deliberately
    /// `unimplemented!()` (mirrors `agent::command::tests::StubProvider`).
    struct FakeModelProvider {
        model: String,
    }

    #[async_trait]
    impl Provider for FakeModelProvider {
        async fn complete(
            &self,
            _: &[bastion_types::Message],
            _: &bastion_types::CallConfig,
        ) -> anyhow::Result<bastion_types::LlmResponse> {
            unimplemented!()
        }
        async fn complete_simple(&self, _: &str) -> anyhow::Result<String> {
            unimplemented!()
        }
        fn context_limit(&self) -> usize {
            8192
        }
        fn model_name(&self) -> &str {
            &self.model
        }
        fn name(&self) -> &'static str {
            "fake"
        }
    }

    pub(crate) struct FakeProviderFactory;

    #[async_trait]
    impl SubscriptionModelProvider for FakeProviderFactory {
        async fn build(
            &self,
            _reference: &ProviderAuthRef,
            model_id: &str,
            _credential: ResolvedProviderCredential,
        ) -> anyhow::Result<Box<dyn Provider>> {
            Ok(Box::new(FakeModelProvider {
                model: model_id.to_string(),
            }))
        }
    }

    /// A service with exactly one registered connector, backed by a real
    /// `SqliteCredentialStateStore` at `db_path` and a scripted flow outcome.
    /// The refresher itself always succeeds — use
    /// [`service_with_failing_refresh`] to script a refresh failure instead.
    pub(crate) async fn service_with(
        db_path: &str,
        provider_id: &str,
        flow_outcome: Result<(), ProviderAuthError>,
    ) -> SubscriptionAuthService {
        service_with_refresh_outcome(db_path, provider_id, flow_outcome, Ok(())).await
    }

    /// Like [`service_with`], but the connector's `ProviderCredentialRefresher`
    /// itself fails every call with `refresh_outcome`'s reason — for testing
    /// `resolve_provider`'s BACOMP-03 path, where `connect`'s flow never runs.
    pub(crate) async fn service_with_failing_refresh(
        db_path: &str,
        provider_id: &str,
        refresh_outcome: ProviderAuthError,
    ) -> SubscriptionAuthService {
        service_with_refresh_outcome(db_path, provider_id, Ok(()), Err(refresh_outcome)).await
    }

    async fn service_with_refresh_outcome(
        db_path: &str,
        provider_id: &str,
        flow_outcome: Result<(), ProviderAuthError>,
        refresh_outcome: Result<(), ProviderAuthError>,
    ) -> SubscriptionAuthService {
        let store =
            Arc::new(crate::provider_credential_state::SqliteCredentialStateStore::new(db_path));
        store.init_schema().await.unwrap();
        let lifecycle = Arc::new(ProviderCredentialLifecycle::new(
            store,
            Arc::new(FakeRefresher {
                material: "CANARY-do-not-leak",
                refresh_outcome,
            }),
            Arc::new(SystemClock),
            Arc::new(ExponentialBackoff::new_default()),
        ));
        let flow = Arc::new(FakeFlow {
            outcome: flow_outcome,
            prompt: "visit https://example.test/device and enter ABCD-1234".to_string(),
        });
        let mut connectors = HashMap::new();
        connectors.insert(
            provider_id.to_string(),
            ConnectorRegistration {
                flow,
                lifecycle,
                provider_factory: Arc::new(FakeProviderFactory),
            },
        );
        let service = SubscriptionAuthService::new(db_path, connectors);
        service.init_schema().await.unwrap();
        service
    }
}

#[cfg(test)]
mod tests {
    use super::fakes::service_with;
    use super::*;
    use bastion_types::provider_auth::ProviderAuthError;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn connect_succeeds_and_lists_the_new_profile() {
        let f = NamedTempFile::new().unwrap();
        let service = service_with(f.path().to_str().unwrap(), "codex", Ok(())).await;

        let record = service
            .connect("alice", "codex", "default", "Work")
            .await
            .unwrap();
        assert_eq!(record.state, Some(ProviderAuthState::Ready));

        let listed = service.list("alice").await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].label, "Work");
        assert_eq!(listed[0].state, Some(ProviderAuthState::Ready));
    }

    /// BAAUTH acceptance bar: a cancelled/denied consent must not leave a
    /// half-connected profile an operator would have to notice and clean up.
    #[tokio::test]
    async fn a_cancelled_consent_leaves_no_profile_behind() {
        let f = NamedTempFile::new().unwrap();
        let service = service_with(
            f.path().to_str().unwrap(),
            "codex",
            Err(ProviderAuthError::ReauthRequired),
        )
        .await;

        assert!(service
            .connect("alice", "codex", "default", "Work")
            .await
            .is_err());
        assert!(
            service.list("alice").await.unwrap().is_empty(),
            "a cancelled consent must not leave a labeled profile behind"
        );
    }

    #[tokio::test]
    async fn connect_to_an_unregistered_provider_fails_clearly() {
        let f = NamedTempFile::new().unwrap();
        let service = SubscriptionAuthService::new(f.path().to_str().unwrap(), HashMap::new());
        service.init_schema().await.unwrap();

        let err = service
            .connect("alice", "codex", "default", "Work")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("unknown provider"));
    }

    #[tokio::test]
    async fn is_registered_reflects_the_connector_map() {
        let f = NamedTempFile::new().unwrap();
        let service = service_with(f.path().to_str().unwrap(), "codex", Ok(())).await;
        assert!(service.is_registered("codex"));
        assert!(!service.is_registered("copilot"));
    }

    /// BACOMP-01: resolve_provider refreshes first (proving the credential
    /// works, same as connect) and hands the connector's factory the exact
    /// model id requested.
    #[tokio::test]
    async fn resolve_provider_builds_a_provider_for_the_requested_model() {
        let f = NamedTempFile::new().unwrap();
        let service = service_with(f.path().to_str().unwrap(), "codex", Ok(())).await;
        service
            .connect("alice", "codex", "work", "Work")
            .await
            .unwrap();

        let provider = service
            .resolve_provider("alice", "codex", "work", "gpt-5")
            .await
            .unwrap();
        assert_eq!(provider.model_name(), "gpt-5");
        assert_eq!(provider.name(), "fake");
    }

    /// BACOMP-03: a credential that cannot refresh (revoked/expired/reauth)
    /// ends here with a typed error — resolve_provider never falls back to
    /// a different reference.
    #[tokio::test]
    async fn resolve_provider_surfaces_a_refresh_failure_instead_of_building_anything() {
        use super::fakes::service_with_failing_refresh;

        let f = NamedTempFile::new().unwrap();
        let service = service_with_failing_refresh(
            f.path().to_str().unwrap(),
            "codex",
            ProviderAuthError::ReauthRequired,
        )
        .await;

        // `Box<dyn Provider>` has no `Debug`, so `unwrap_err()` (which
        // requires it) doesn't apply — match explicitly instead.
        match service
            .resolve_provider("alice", "codex", "default", "gpt-5")
            .await
        {
            Err(err) => assert!(err.to_string().contains("re-authentication")),
            Ok(_) => panic!("a refresh failure must not build a provider"),
        }
    }

    #[tokio::test]
    async fn resolve_provider_on_an_unregistered_provider_fails_clearly() {
        let f = NamedTempFile::new().unwrap();
        let service = SubscriptionAuthService::new(f.path().to_str().unwrap(), HashMap::new());
        service.init_schema().await.unwrap();

        match service
            .resolve_provider("alice", "codex", "default", "gpt-5")
            .await
        {
            Err(err) => assert!(err.to_string().contains("unknown provider")),
            Ok(_) => panic!("an unregistered provider must not build anything"),
        }
    }

    #[tokio::test]
    async fn status_finds_a_profile_by_bare_profile_id() {
        let f = NamedTempFile::new().unwrap();
        let service = service_with(f.path().to_str().unwrap(), "codex", Ok(())).await;
        service
            .connect("alice", "codex", "work", "Work account")
            .await
            .unwrap();

        let matches = service.status("alice", "work").await.unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].provider_id, "codex");
    }

    #[tokio::test]
    async fn status_on_an_unknown_profile_is_empty_not_an_error() {
        let f = NamedTempFile::new().unwrap();
        let service = service_with(f.path().to_str().unwrap(), "codex", Ok(())).await;
        assert!(service.status("alice", "nope").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn disconnect_removes_only_the_requested_profile() {
        let f = NamedTempFile::new().unwrap();
        let service = service_with(f.path().to_str().unwrap(), "codex", Ok(())).await;
        service
            .connect("alice", "codex", "work", "Work")
            .await
            .unwrap();
        service
            .connect("alice", "codex", "personal", "Personal")
            .await
            .unwrap();

        service.disconnect("alice", "codex", "work").await.unwrap();

        let remaining = service.list("alice").await.unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].profile_id, "personal");
    }

    #[tokio::test]
    async fn disconnect_on_a_never_connected_profile_is_a_harmless_no_op() {
        let f = NamedTempFile::new().unwrap();
        let service = service_with(f.path().to_str().unwrap(), "codex", Ok(())).await;
        service.disconnect("alice", "codex", "ghost").await.unwrap();
    }

    /// Two owners never see or affect each other's profiles.
    #[tokio::test]
    async fn two_owners_have_independent_profile_lists() {
        let f = NamedTempFile::new().unwrap();
        let service = service_with(f.path().to_str().unwrap(), "codex", Ok(())).await;
        service
            .connect("alice", "codex", "default", "Alice's")
            .await
            .unwrap();
        service
            .connect("mallory", "codex", "default", "Mallory's")
            .await
            .unwrap();

        assert_eq!(service.list("alice").await.unwrap().len(), 1);
        assert_eq!(service.list("mallory").await.unwrap().len(), 1);

        service
            .disconnect("alice", "codex", "default")
            .await
            .unwrap();
        assert!(service.list("alice").await.unwrap().is_empty());
        assert_eq!(
            service.list("mallory").await.unwrap().len(),
            1,
            "disconnecting alice's profile must not touch mallory's"
        );
    }

    /// BAAUTH-01/02: nothing this service returns or persists ever carries
    /// the underlying secret material — proven with a canary value only the
    /// fake refresher's resolved credential holds.
    #[tokio::test]
    async fn no_record_ever_leaks_the_canary_material() {
        let f = NamedTempFile::new().unwrap();
        let service = service_with(f.path().to_str().unwrap(), "codex", Ok(())).await;

        let record = service
            .connect("alice", "codex", "default", "Work")
            .await
            .unwrap();
        assert!(!format!("{record:?}").contains("CANARY-do-not-leak"));

        let listed = service.list("alice").await.unwrap();
        assert!(!format!("{listed:?}").contains("CANARY-do-not-leak"));
    }

    /// A profile survives a fresh service instance pointed at the same db
    /// file (restart) — both the label and the lifecycle state.
    #[tokio::test]
    async fn a_profile_survives_across_service_instances_pointed_at_the_same_db() {
        let f = NamedTempFile::new().unwrap();
        let path = f.path().to_str().unwrap().to_string();

        let first = service_with(&path, "codex", Ok(())).await;
        first
            .connect("alice", "codex", "default", "Work")
            .await
            .unwrap();
        drop(first);

        let second = service_with(&path, "codex", Ok(())).await;
        let listed = second.list("alice").await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].state, Some(ProviderAuthState::Ready));
    }
}
