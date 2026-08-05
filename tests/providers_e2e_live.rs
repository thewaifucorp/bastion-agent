//! Gate 3/5 of `bastion-agent#32` (`agent/release-v0.3.0`) — a real, opt-in
//! end-to-end proof of the subscription-provider flow: connect a real
//! Codex/ChatGPT account, run one turn of inference through it, simulate a
//! daemon restart, then check status and disconnect — all through the exact
//! same command surfaces an operator uses (`src/agent/auth_command.rs`,
//! `src/agent/model_status_command.rs`), never a shortcut around them.
//!
//! Not run by default (`cargo test`): requires a real ChatGPT/Codex
//! subscription, a human to complete the device-code browser approval within
//! 15 minutes, and spends real inference tokens. Run manually:
//!
//! ```text
//! cargo test --test providers_e2e_live -- --ignored --nocapture
//! ```
//!
//! When it runs, watch stderr for a line starting with "Visit ... and enter
//! this code: ..." shortly after start — that is the device-code prompt
//! (`CodexConnector::start`, `src/codex_connector.rs`). Complete it in a
//! browser signed into the ChatGPT account you want to test with; the test
//! blocks on `wait_for_approval` until you do (or 15 minutes elapse).
//!
//! Model id defaults to `gpt-5.6-sol`, verified against Codex CLI 0.146.0 on
//! 2026-08-03. Override with `CODEX_E2E_MODEL` when the vendor catalog moves.
//!
//! Fixture note: this integration test binary cannot reuse fixtures from
//! other files under `tests/` (each compiles as its own crate) — the small
//! `build_service` helper below duplicates exactly the wiring
//! `main.rs::daemon_loop` does for the Codex connector, not a design choice
//! specific to this test.

use std::collections::HashMap;
use std::sync::Arc;

use bastion::agent::{auth_command, model_status_command};
use bastion::codex_connector::{CodexConnector, SqliteCodexTokenStore};
use bastion::provider_credential_state::SqliteCredentialStateStore;
use bastion::subscription_auth::{ConnectorRegistration, SubscriptionAuthService};
use bastion_providers::codex::{CodexConfig, CodexRefresher, CodexTokenStore, CODEX_PROVIDER_ID};
use bastion_runtime::agent::backend::{BackendProfile, ConversationBackend};
use bastion_runtime::provider_auth::{
    Clock, ExponentialBackoff, ProviderCredentialLifecycle, ProviderCredentialRefresher,
    SystemClock,
};
use tempfile::NamedTempFile;

/// Builds the same stack `main.rs` wires for the Codex connector, pointed at
/// `db_path`. Called twice in the test below with the SAME `db_path` to
/// simulate a daemon restart: a fresh in-memory `SubscriptionAuthService`
/// backed by the persisted sqlite state, exactly like a real process
/// restart would produce.
async fn build_service(db_path: &str) -> anyhow::Result<Arc<SubscriptionAuthService>> {
    let state_store = Arc::new(SqliteCredentialStateStore::new(db_path));
    state_store.init_schema().await?;
    let token_store = Arc::new(SqliteCodexTokenStore::new(db_path));
    token_store.init_schema().await?;

    let config = CodexConfig::default();
    let connector = Arc::new(CodexConnector::new(
        config.clone(),
        token_store.clone() as Arc<dyn CodexTokenStore>,
    ));
    let refresher = Arc::new(CodexRefresher::new(
        config,
        token_store as Arc<dyn CodexTokenStore>,
    ));
    let lifecycle = Arc::new(ProviderCredentialLifecycle::new(
        state_store,
        refresher as Arc<dyn ProviderCredentialRefresher>,
        Arc::new(SystemClock),
        Arc::new(ExponentialBackoff::new_default()),
    ));

    let mut connectors = HashMap::new();
    connectors.insert(
        CODEX_PROVIDER_ID.to_string(),
        ConnectorRegistration {
            flow: connector.clone() as Arc<dyn bastion::subscription_auth::SubscriptionLoginFlow>,
            lifecycle,
            provider_factory: connector
                as Arc<dyn bastion::subscription_auth::SubscriptionModelProvider>,
        },
    );

    let service = Arc::new(SubscriptionAuthService::new(db_path, connectors));
    service.init_schema().await?;
    Ok(service)
}

const OWNER: &str = "providers-e2e-live-owner";
const PROFILE: &str = "e2e-live";
const MARKER: &str = "BASTION-PROVIDERS-E2E-OK";

#[tokio::test]
#[ignore = "requires a real ChatGPT/Codex subscription + a human to complete the browser \
            device-code approval within 15 minutes; spends real inference tokens. Run manually \
            with --ignored --nocapture"]
async fn codex_connect_infer_restart_status_disconnect_live() {
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_env_filter("info")
        .try_init();

    let model_id = std::env::var("CODEX_E2E_MODEL").unwrap_or_else(|_| "gpt-5.6-sol".to_string());
    let f = NamedTempFile::new().unwrap();
    let db_path = f.path().to_str().unwrap().to_string();

    // --- connect ---------------------------------------------------------
    eprintln!(
        "\n=== /auth connect codex {PROFILE} — watch for the device-code prompt below and \
         complete it in a browser within 15 minutes ===\n"
    );
    let service = build_service(&db_path).await.expect("build_service");
    let connect_reply = auth_command::handle(
        &service,
        Some(&format!("connect codex {PROFILE} E2E live test")),
        OWNER,
    )
    .await
    .expect("/auth connect must succeed against a real completed device-code login");
    eprintln!("connect reply: {connect_reply}");
    assert!(
        connect_reply.contains(&format!("connected {CODEX_PROVIDER_ID}/{PROFILE}"))
            && connect_reply.contains("ready"),
        "expected a 'connected .../... — ready' reply, got: {connect_reply}"
    );

    // --- inference ---------------------------------------------------------
    let provider = service
        .resolve_provider(OWNER, CODEX_PROVIDER_ID, PROFILE, &model_id)
        .await
        .expect("resolve_provider must build a live Codex-backed provider");
    let response = provider
        .complete_simple(&format!(
            "Reply with exactly this and nothing else: {MARKER}"
        ))
        .await
        .expect("a real inference call through the subscription-backed provider must succeed");
    eprintln!("inference response: {response:?}");
    assert!(
        response.contains(MARKER),
        "expected the marker word in the live Codex response, got: {response:?}"
    );

    // /model status, through the real command glue (BAUX-01/BACOMP): proves
    // the ExecutionOwner surfaces correctly once a subscription model is
    // actually selected, not just in the fake-clock unit tests.
    let backend_profile = BackendProfile {
        conversation: ConversationBackend::Model,
        ..Default::default()
    };
    let now = SystemClock.now_nanos();
    let status_text = model_status_command::handle(
        &backend_profile,
        &model_id,
        CODEX_PROVIDER_ID,
        OWNER,
        Some(&service),
        now,
    )
    .await;
    eprintln!("/model status: {status_text}");
    assert!(
        status_text.contains("Bastion"),
        "a Model-conversation backend must report ExecutionOwner::Bastion, got: {status_text}"
    );

    // --- restart -------------------------------------------------------
    // Drop the in-memory service entirely and rebuild against the SAME
    // db_path — the same trick a real daemon restart performs, and the same
    // one M4.2's persistence tests use.
    drop(service);
    eprintln!("\n=== simulated restart: rebuilding SubscriptionAuthService from disk ===\n");
    let restarted = build_service(&db_path)
        .await
        .expect("build_service after restart");

    // --- /auth status ----------------------------------------------------
    let status_reply = auth_command::handle(&restarted, Some(&format!("status {PROFILE}")), OWNER)
        .await
        .expect("/auth status must succeed after a simulated restart");
    eprintln!("status reply after restart: {status_reply}");
    assert!(
        status_reply.contains("ready"),
        "the credential lifecycle state must survive a restart (persisted via \
         CredentialStateStore), got: {status_reply}"
    );

    // --- /auth disconnect --------------------------------------------------
    let disconnect_reply = auth_command::handle(
        &restarted,
        Some(&format!("disconnect {CODEX_PROVIDER_ID}/{PROFILE}")),
        OWNER,
    )
    .await
    .expect("/auth disconnect must succeed");
    eprintln!("disconnect reply: {disconnect_reply}");
    assert!(
        disconnect_reply.contains(&format!("disconnected {CODEX_PROVIDER_ID}/{PROFILE}")),
        "got: {disconnect_reply}"
    );

    let list_reply = auth_command::handle(&restarted, Some("list"), OWNER)
        .await
        .expect("/auth list must succeed");
    assert_eq!(
        list_reply, "no subscription profiles connected.",
        "disconnect must actually remove the profile, got: {list_reply}"
    );
}
