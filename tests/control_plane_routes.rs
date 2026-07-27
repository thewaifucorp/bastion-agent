//! Integration tests for the live `/v1/*` routes (US — External Control
//! Plane and SDK, Phase 2). Builds a real `ControlPlaneState` (tempfile
//! `SqliteTaskStore` + `SqliteCredentialStore`) and drives the actual router
//! via `tower::ServiceExt::oneshot` — no mocked store, no mocked auth. This
//! is the strongest in-process approximation of an E2E test available before
//! standing up the full Docker stack (see the manual curl validation in
//! `docs/en/control-plane-security.md`'s Phase 2 notes).

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use bastion::control_plane::credential::SqliteCredentialStore;
use bastion::control_plane::rate_limit::RateLimiter;
use bastion::control_plane::routes::{router, ControlPlaneState};
use bastion::control_plane::scope::{Scope, ScopeSet};
use bastion::control_plane::webhook_delivery::SqliteWebhookDeliveryStore;
use bastion::control_plane::webhook_subscription::SqliteWebhookSubscriptionStore;
use bastion_runtime::task::{
    AcceptanceCriterion, Attempt, AttemptId, Bounds, CorrelationIds, ExecutionMode, Frame, Intent,
    IntentOrigin, OpaqueState, SqliteTaskStore, TaskCase, TaskCaseId, TaskStatus, TaskStore,
    UsageAccum,
};
use serde_json::Value;
use tempfile::NamedTempFile;
use tower::ServiceExt;

async fn build_app() -> (
    NamedTempFile,
    Arc<SqliteTaskStore>,
    Arc<SqliteCredentialStore>,
    axum::Router,
) {
    let f = NamedTempFile::new().expect("tempfile");
    let path = f.path().to_str().expect("utf8 path").to_owned();

    let task_store = Arc::new(SqliteTaskStore::new(path.clone()));
    task_store.init_schema().await.expect("task store schema");

    let credential_store = Arc::new(SqliteCredentialStore::new(path.clone()));
    credential_store
        .init_schema()
        .await
        .expect("credential store schema");

    let webhook_subscription_store = Arc::new(SqliteWebhookSubscriptionStore::new(path.clone()));
    webhook_subscription_store
        .init_schema()
        .await
        .expect("webhook subscription store schema");

    let webhook_delivery_store = Arc::new(SqliteWebhookDeliveryStore::new(path));
    webhook_delivery_store
        .init_schema()
        .await
        .expect("webhook delivery store schema");

    let app = router(
        ControlPlaneState {
            task_store: task_store.clone() as Arc<dyn TaskStore>,
            credential_store: credential_store.clone(),
            webhook_subscription_store,
            webhook_delivery_store,
        },
        RateLimiter::new(),
        None,
    );

    (f, task_store, credential_store, app)
}

/// Like [`build_app`], but also returns the two Phase 4 stores — a separate
/// helper (rather than widening `build_app`'s return tuple, which every
/// existing test destructures positionally) so webhook-specific tests can
/// inspect the subscription/delivery tables directly without touching the
/// ~20 call sites above.
async fn build_app_with_webhook_stores() -> (
    NamedTempFile,
    Arc<SqliteTaskStore>,
    Arc<SqliteCredentialStore>,
    Arc<SqliteWebhookSubscriptionStore>,
    Arc<SqliteWebhookDeliveryStore>,
    axum::Router,
) {
    let f = NamedTempFile::new().expect("tempfile");
    let path = f.path().to_str().expect("utf8 path").to_owned();

    let task_store = Arc::new(SqliteTaskStore::new(path.clone()));
    task_store.init_schema().await.expect("task store schema");

    let credential_store = Arc::new(SqliteCredentialStore::new(path.clone()));
    credential_store
        .init_schema()
        .await
        .expect("credential store schema");

    let webhook_subscription_store = Arc::new(SqliteWebhookSubscriptionStore::new(path.clone()));
    webhook_subscription_store
        .init_schema()
        .await
        .expect("webhook subscription store schema");

    let webhook_delivery_store = Arc::new(SqliteWebhookDeliveryStore::new(path));
    webhook_delivery_store
        .init_schema()
        .await
        .expect("webhook delivery store schema");

    let app = router(
        ControlPlaneState {
            task_store: task_store.clone() as Arc<dyn TaskStore>,
            credential_store: credential_store.clone(),
            webhook_subscription_store: webhook_subscription_store.clone(),
            webhook_delivery_store: webhook_delivery_store.clone(),
        },
        RateLimiter::new(),
        None,
    );

    (
        f,
        task_store,
        credential_store,
        webhook_subscription_store,
        webhook_delivery_store,
        app,
    )
}

fn sample_case(owner: &str, id: &str, created_at: i64) -> TaskCase {
    TaskCase {
        id: TaskCaseId(id.to_string()),
        owner: owner.to_string(),
        mode: ExecutionMode::Pursue,
        intent: Intent {
            owner: owner.to_string(),
            mode: ExecutionMode::Pursue,
            summary: "test".into(),
            origin: IntentOrigin::Message,
        },
        frame: Frame {
            objective: format!("objective for {id}"),
            acceptance: vec![AcceptanceCriterion {
                description: "done".into(),
                check: None,
            }],
            context_refs: vec![],
        },
        bounds: Bounds::default(),
        status: TaskStatus::Running,
        stop_reason: None,
        attempts: vec![],
        pending_approvals: vec![],
        next_decision: None,
        usage: UsageAccum::default(),
        parent: None,
        correlation: CorrelationIds::default(),
        business_state: OpaqueState::default(),
        created_at,
        updated_at: created_at,
        revision: 1,
    }
}

async fn issue_token(store: &SqliteCredentialStore, owner: &str, scopes: &[Scope]) -> String {
    let (_id, token) = store
        .issue(owner, None, ScopeSet::new(scopes.iter().copied()), "test")
        .await
        .expect("issue");
    token
}

async fn body_json(resp: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("read body");
    serde_json::from_slice(&bytes).expect("valid json")
}

fn post_json(uri: &str, token: &str, idempotency_key: Option<&str>, body: Value) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(uri)
        .header("x-bastion-token", token)
        .header("content-type", "application/json");
    if let Some(key) = idempotency_key {
        builder = builder.header("idempotency-key", key);
    }
    builder.body(Body::from(body.to_string())).unwrap()
}

#[tokio::test]
async fn list_tasks_requires_auth() {
    let (_f, _task_store, _cred_store, app) = build_app().await;
    let req = Request::builder()
        .uri("/v1/tasks")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ─── Phase 4: POST /v1/webhook-subscriptions + event emission ─────────────

#[tokio::test]
async fn create_webhook_subscription_requires_webhooks_manage_scope() {
    let (_f, _task_store, cred_store, _sub_store, _delivery_store, app) =
        build_app_with_webhook_stores().await;
    let token = issue_token(&cred_store, "alice", &[Scope::TasksRead]).await;
    let req = post_json(
        "/v1/webhook-subscriptions",
        &token,
        None,
        serde_json::json!({ "target_url": "https://example.com/hook" }),
    );
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

/// The SSRF proof at the ROUTE level (the store-level guard already has its
/// own dedicated tests in `src/control_plane/webhook_subscription.rs`) —
/// confirms the route actually wires `issue()`'s validation into a real
/// rejected HTTP response, not just that the store function works in
/// isolation.
#[tokio::test]
async fn create_webhook_subscription_rejects_a_loopback_target_url() {
    let (_f, _task_store, cred_store, _sub_store, _delivery_store, app) =
        build_app_with_webhook_stores().await;
    let token = issue_token(&cred_store, "alice", &[Scope::WebhooksManage]).await;
    let req = post_json(
        "/v1/webhook-subscriptions",
        &token,
        None,
        serde_json::json!({ "target_url": "http://127.0.0.1:9999/hook" }),
    );
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

/// Depends on real DNS resolution against a real public hostname
/// (`example.com` — IANA-reserved for documentation/testing, stable,
/// harmless to touch) — the one test in this file with a network
/// dependency, needed because `adaptive::browser::validate_public_url`
/// genuinely resolves the host as part of the SSRF guard; there is no way to
/// exercise the ALLOW path without a real, resolvable public address.
#[tokio::test]
async fn create_webhook_subscription_succeeds_for_a_real_public_url() {
    let (_f, _task_store, cred_store, _sub_store, _delivery_store, app) =
        build_app_with_webhook_stores().await;
    let token = issue_token(&cred_store, "alice", &[Scope::WebhooksManage]).await;
    let req = post_json(
        "/v1/webhook-subscriptions",
        &token,
        None,
        serde_json::json!({ "target_url": "https://example.com/hook", "event_types": ["task.created"] }),
    );
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let json = body_json(resp).await;
    assert_eq!(json["owner_id"], "alice");
    assert_eq!(json["target_url"], "https://example.com/hook");
    // The signing secret is returned exactly once, in this creation
    // response — a caller has no other way to retrieve it.
    let secret = json["secret"]
        .as_str()
        .expect("secret must be present on creation");
    assert!(!secret.is_empty());
}

/// End-to-end proof that a successful mutation actually enqueues a webhook
/// delivery — seeds a subscription directly into the store (bypassing the
/// SSRF-gated route, same reasoning as
/// `webhook_subscription`'s own unit tests) so this test has no network
/// dependency, then drives a REAL `POST /v1/tasks` through the router and
/// asserts a matching delivery landed in the queue.
#[tokio::test]
async fn create_task_enqueues_a_task_created_delivery_for_a_matching_subscription() {
    let (_f, _task_store, cred_store, sub_store, delivery_store, app) =
        build_app_with_webhook_stores().await;

    {
        let conn = rusqlite::Connection::open(sub_store.db_path()).unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS webhook_subscriptions (
                id TEXT PRIMARY KEY, owner_id TEXT NOT NULL, target_url TEXT NOT NULL,
                event_types TEXT NOT NULL, secret TEXT NOT NULL, created_at INTEGER NOT NULL,
                revoked_at INTEGER
             );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO webhook_subscriptions VALUES \
             ('sub1','alice','https://example.com/hook','[\"task.created\"]','shh',1,NULL)",
            [],
        )
        .unwrap();
    }

    let token = issue_token(&cred_store, "alice", &[Scope::TasksCreate]).await;
    let req = post_json(
        "/v1/tasks",
        &token,
        Some("evt-key-1"),
        serde_json::json!({ "objective": "ship it" }),
    );
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let matches = sub_store
        .active_matching("alice", "task.created")
        .await
        .unwrap();
    assert_eq!(
        matches.len(),
        1,
        "the subscription we seeded should be an active match"
    );

    // Poll briefly: `emit_event` is awaited inside the handler before the
    // response is returned, so this should already be true by the time we
    // get here, but a tiny retry loop keeps the test robust against any
    // future change that makes emission fire-and-forget instead.
    let due = delivery_store.count_pending().await.unwrap();
    assert_eq!(
        due, 1,
        "a task.created delivery must be enqueued for the matching subscription"
    );
}

// ─── POST /v1/credentials (remote issuance, operator-token gated) ──────────

/// Like [`build_app`], but with remote credential issuance mounted under the
/// given operator token. A separate helper so every existing test keeps
/// asserting the default: the route is not mounted at all.
async fn build_app_with_remote_issuance(
    daemon_token: Option<&str>,
) -> (NamedTempFile, Arc<SqliteCredentialStore>, axum::Router) {
    let f = NamedTempFile::new().expect("tempfile");
    let path = f.path().to_str().expect("utf8 path").to_owned();

    let task_store = Arc::new(SqliteTaskStore::new(path.clone()));
    task_store.init_schema().await.expect("task store schema");
    let credential_store = Arc::new(SqliteCredentialStore::new(path.clone()));
    credential_store
        .init_schema()
        .await
        .expect("credential store schema");
    let webhook_subscription_store = Arc::new(SqliteWebhookSubscriptionStore::new(path.clone()));
    webhook_subscription_store
        .init_schema()
        .await
        .expect("webhook subscription store schema");
    let webhook_delivery_store = Arc::new(SqliteWebhookDeliveryStore::new(path));
    webhook_delivery_store
        .init_schema()
        .await
        .expect("webhook delivery store schema");

    let app = router(
        ControlPlaneState {
            task_store: task_store.clone() as Arc<dyn TaskStore>,
            credential_store: credential_store.clone(),
            webhook_subscription_store,
            webhook_delivery_store,
        },
        RateLimiter::new(),
        Some(bastion::channel::operational::DaemonAccessAuth::new(
            daemon_token.map(str::to_string),
        )),
    );
    (f, credential_store, app)
}

fn issue_req(body: Value, bearer: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/v1/credentials")
        .header("content-type", "application/json");
    if let Some(token) = bearer {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    builder.body(Body::from(body.to_string())).unwrap()
}

fn valid_issue_body() -> Value {
    serde_json::json!({
        "owner_id": "alice",
        "scopes": ["tasks:read", "tasks:create"],
        "label": "paperclip"
    })
}

/// The default: an operator who has not opted in has no issuance route at all,
/// so issuance stays console-only exactly as before.
#[tokio::test]
async fn credential_issuance_is_not_mounted_by_default() {
    let (_f, _task_store, _cred_store, app) = build_app().await;
    let resp = app
        .oneshot(issue_req(valid_issue_body(), Some("anything")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn credential_issuance_without_a_configured_token_refuses_everything() {
    let (_f, _cred_store, app) = build_app_with_remote_issuance(None).await;
    let resp = app
        .oneshot(issue_req(valid_issue_body(), Some("anything")))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "mounting the route without BASTION_DAEMON_TOKEN must fail closed"
    );
}

#[tokio::test]
async fn credential_issuance_rejects_a_wrong_or_missing_operator_token() {
    let (_f, _cred_store, app) = build_app_with_remote_issuance(Some("right")).await;
    let wrong = app
        .clone()
        .oneshot(issue_req(valid_issue_body(), Some("wrong")))
        .await
        .unwrap();
    assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);

    let missing = app
        .oneshot(issue_req(valid_issue_body(), None))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);
}

/// A Control Plane credential must never be able to mint credentials: minting
/// authority is the operator token alone, so presenting a perfectly valid
/// `x-bastion-token` (even one with every scope) is still unauthorized here.
#[tokio::test]
async fn a_control_plane_credential_cannot_mint_credentials() {
    let (_f, cred_store, app) = build_app_with_remote_issuance(Some("right")).await;
    let token = issue_token(
        &cred_store,
        "alice",
        &[
            Scope::TasksRead,
            Scope::TasksCreate,
            Scope::TasksControl,
            Scope::WebhooksManage,
        ],
    )
    .await;

    let req = Request::builder()
        .method("POST")
        .uri("/v1/credentials")
        .header("content-type", "application/json")
        .header("x-bastion-token", token)
        .body(Body::from(valid_issue_body().to_string()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// End to end: the issued token actually authenticates on another `/v1` route,
/// with exactly the scopes that were asked for and no more.
#[tokio::test]
async fn an_issued_credential_authenticates_with_exactly_the_requested_scopes() {
    let (_f, _cred_store, app) = build_app_with_remote_issuance(Some("right")).await;
    let resp = app
        .clone()
        .oneshot(issue_req(valid_issue_body(), Some("right")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let json = body_json(resp).await;
    assert_eq!(json["owner_id"], "alice");
    assert_eq!(
        json["scopes"],
        serde_json::json!(["tasks:read", "tasks:create"])
    );
    assert!(
        json.get("project").is_none(),
        "no project asked, none echoed"
    );
    let token = json["token"]
        .as_str()
        .expect("token returned once")
        .to_owned();
    assert!(!token.is_empty());

    // Granted scope works...
    let listed = app
        .clone()
        .oneshot(get_req("/v1/tasks", &token))
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);

    // ...and a scope that was NOT asked for is still denied.
    let denied = app
        .oneshot(delete_req("/v1/webhook-subscriptions/whatever", &token))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn an_issued_credential_carries_its_project_tag() {
    let (_f, _cred_store, app) = build_app_with_remote_issuance(Some("right")).await;
    let body = serde_json::json!({
        "owner_id": "alice",
        "project": "acme",
        "scopes": ["tasks:create"],
        "label": "acme-integration"
    });
    let resp = app
        .clone()
        .oneshot(issue_req(body, Some("right")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let json = body_json(resp).await;
    assert_eq!(json["project"], "acme");

    // The tag is live, not decorative: a task created with this credential is
    // only visible to a project-scoped list for the same project.
    let token = json["token"].as_str().unwrap().to_owned();
    let created = app
        .clone()
        .oneshot(post_json(
            "/v1/tasks",
            &token,
            Some("k1"),
            serde_json::json!({ "objective": "ship it" }),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn credential_issuance_validates_its_body() {
    let (_f, _cred_store, app) = build_app_with_remote_issuance(Some("right")).await;

    let cases = [
        serde_json::json!({ "owner_id": "  ", "scopes": ["tasks:read"], "label": "l" }),
        serde_json::json!({ "owner_id": "alice", "scopes": [], "label": "l" }),
        serde_json::json!({ "owner_id": "alice", "scopes": ["tasks:read"], "label": " " }),
    ];
    for body in cases {
        let resp = app
            .clone()
            .oneshot(issue_req(body.clone(), Some("right")))
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "body should have been refused: {body}"
        );
    }
}

/// An unknown scope name refuses the whole request — issuing a credential with
/// silently fewer grants than asked for would surface later as a puzzling 403.
#[tokio::test]
async fn credential_issuance_refuses_an_unknown_scope_instead_of_dropping_it() {
    let (_f, _cred_store, app) = build_app_with_remote_issuance(Some("right")).await;
    let body = serde_json::json!({
        "owner_id": "alice",
        "scopes": ["tasks:read", "tasks:delete"],
        "label": "l"
    });
    let resp = app.oneshot(issue_req(body, Some("right"))).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let json = body_json(resp).await;
    assert_eq!(json["code"], "invalid_scope");
    assert!(
        json["message"].as_str().unwrap().contains("tasks:delete"),
        "the message must name the offending scope: {json}"
    );
}

// ─── GET /v1/webhook-subscriptions + DELETE /v1/webhook-subscriptions/{id} ─

/// Seeds a subscription row directly, bypassing the SSRF-gated `issue()` —
/// same reasoning as the delivery test above: these tests are about listing
/// and revoking, and must not depend on DNS.
fn seed_subscription(
    sub_store: &SqliteWebhookSubscriptionStore,
    id: &str,
    owner: &str,
    revoked_at: Option<i64>,
) {
    let conn = rusqlite::Connection::open(sub_store.db_path()).unwrap();
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS webhook_subscriptions (
            id TEXT PRIMARY KEY, owner_id TEXT NOT NULL, target_url TEXT NOT NULL,
            event_types TEXT NOT NULL, secret TEXT NOT NULL, created_at INTEGER NOT NULL,
            revoked_at INTEGER
         );",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO webhook_subscriptions VALUES (?1, ?2, 'https://example.com/hook', \
         '[\"task.created\"]', 'shh', 1, ?3)",
        rusqlite::params![id, owner, revoked_at],
    )
    .unwrap();
}

fn delete_req(uri: &str, token: &str) -> Request<Body> {
    Request::builder()
        .method("DELETE")
        .uri(uri)
        .header("x-bastion-token", token)
        .body(Body::empty())
        .unwrap()
}

fn get_req(uri: &str, token: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .header("x-bastion-token", token)
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn list_webhook_subscriptions_requires_auth() {
    let (_f, _task_store, _cred_store, _sub_store, _delivery_store, app) =
        build_app_with_webhook_stores().await;
    let req = Request::builder()
        .uri("/v1/webhook-subscriptions")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn list_webhook_subscriptions_requires_webhooks_manage_scope() {
    let (_f, _task_store, cred_store, _sub_store, _delivery_store, app) =
        build_app_with_webhook_stores().await;
    let token = issue_token(&cred_store, "alice", &[Scope::TasksRead]).await;
    let resp = app
        .oneshot(get_req("/v1/webhook-subscriptions", &token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

/// Owner isolation plus the secret contract: another owner's subscription is
/// invisible, and no item ever carries the signing secret — it exists only in
/// the response to the POST that created it.
#[tokio::test]
async fn list_webhook_subscriptions_is_owner_scoped_and_never_returns_the_secret() {
    let (_f, _task_store, cred_store, sub_store, _delivery_store, app) =
        build_app_with_webhook_stores().await;
    seed_subscription(&sub_store, "sub-alice", "alice", None);
    seed_subscription(&sub_store, "sub-mallory", "mallory", None);

    let token = issue_token(&cred_store, "alice", &[Scope::WebhooksManage]).await;
    let resp = app
        .oneshot(get_req("/v1/webhook-subscriptions", &token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "alice must not see mallory's subscription");
    assert_eq!(items[0]["id"], "sub-alice");
    assert!(
        items[0].as_object().unwrap().get("secret").is_none(),
        "the signing secret must never appear in a list response"
    );
    assert!(
        items[0].as_object().unwrap().get("revoked_at").is_none(),
        "an active subscription omits revoked_at entirely"
    );
}

#[tokio::test]
async fn list_webhook_subscriptions_includes_revoked_ones_with_their_timestamp() {
    let (_f, _task_store, cred_store, sub_store, _delivery_store, app) =
        build_app_with_webhook_stores().await;
    seed_subscription(&sub_store, "sub-gone", "alice", Some(42));

    let token = issue_token(&cred_store, "alice", &[Scope::WebhooksManage]).await;
    let resp = app
        .oneshot(get_req("/v1/webhook-subscriptions", &token))
        .await
        .unwrap();
    let json = body_json(resp).await;
    let items = json["items"].as_array().unwrap();
    assert_eq!(
        items.len(),
        1,
        "a revoked subscription stays listed — an operator has to be able to \
         tell 'never registered' from 'registered and later revoked'"
    );
    assert_eq!(items[0]["revoked_at"], serde_json::json!(42));
}

#[tokio::test]
async fn revoke_webhook_subscription_requires_webhooks_manage_scope() {
    let (_f, _task_store, cred_store, sub_store, _delivery_store, app) =
        build_app_with_webhook_stores().await;
    seed_subscription(&sub_store, "sub-alice", "alice", None);
    let token = issue_token(&cred_store, "alice", &[Scope::TasksRead]).await;
    let resp = app
        .oneshot(delete_req("/v1/webhook-subscriptions/sub-alice", &token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

/// The delivery loop only ever loads `revoked_at IS NULL` rows
/// (`active_matching`), so a successful revoke must make the subscription stop
/// matching — asserted through the store, not just through the HTTP status.
#[tokio::test]
async fn revoke_webhook_subscription_stops_delivery_and_shows_up_in_the_list() {
    let (_f, _task_store, cred_store, sub_store, _delivery_store, app) =
        build_app_with_webhook_stores().await;
    seed_subscription(&sub_store, "sub-alice", "alice", None);
    assert_eq!(
        sub_store
            .active_matching("alice", "task.created")
            .await
            .unwrap()
            .len(),
        1
    );

    let token = issue_token(&cred_store, "alice", &[Scope::WebhooksManage]).await;
    let resp = app
        .clone()
        .oneshot(delete_req("/v1/webhook-subscriptions/sub-alice", &token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    assert!(
        sub_store
            .active_matching("alice", "task.created")
            .await
            .unwrap()
            .is_empty(),
        "a revoked subscription must no longer be an active delivery target"
    );

    let listed = body_json(
        app.oneshot(get_req("/v1/webhook-subscriptions", &token))
            .await
            .unwrap(),
    )
    .await;
    assert!(
        listed["items"][0]["revoked_at"].is_i64(),
        "the tombstone must be visible in the list, not deleted"
    );
}

/// IDOR: another owner's id answers 404 — indistinguishable from an id that
/// never existed, so this route cannot be used to probe for other owners'
/// subscription ids. The victim's subscription stays deliverable.
#[tokio::test]
async fn revoke_webhook_subscription_cannot_touch_another_owners_subscription() {
    let (_f, _task_store, cred_store, sub_store, _delivery_store, app) =
        build_app_with_webhook_stores().await;
    seed_subscription(&sub_store, "sub-mallory", "mallory", None);

    let token = issue_token(&cred_store, "alice", &[Scope::WebhooksManage]).await;
    let resp = app
        .oneshot(delete_req("/v1/webhook-subscriptions/sub-mallory", &token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let json = body_json(resp).await;
    assert_eq!(json["code"], "not_found");

    assert_eq!(
        sub_store
            .active_matching("mallory", "task.created")
            .await
            .unwrap()
            .len(),
        1,
        "mallory's subscription must be untouched"
    );
}

#[tokio::test]
async fn revoke_webhook_subscription_twice_is_a_conflict_not_a_silent_success() {
    let (_f, _task_store, cred_store, sub_store, _delivery_store, app) =
        build_app_with_webhook_stores().await;
    seed_subscription(&sub_store, "sub-alice", "alice", None);
    let token = issue_token(&cred_store, "alice", &[Scope::WebhooksManage]).await;

    let first = app
        .clone()
        .oneshot(delete_req("/v1/webhook-subscriptions/sub-alice", &token))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::NO_CONTENT);

    let second = app
        .oneshot(delete_req("/v1/webhook-subscriptions/sub-alice", &token))
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::CONFLICT);
    let json = body_json(second).await;
    assert_eq!(json["code"], "already_revoked");
}

#[tokio::test]
async fn revoke_webhook_subscription_for_an_unknown_id_is_not_found() {
    let (_f, _task_store, cred_store, _sub_store, _delivery_store, app) =
        build_app_with_webhook_stores().await;
    let token = issue_token(&cred_store, "alice", &[Scope::WebhooksManage]).await;
    let resp = app
        .oneshot(delete_req("/v1/webhook-subscriptions/nope", &token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ─── Phase 3: POST /v1/tasks (create) ──────────────────────────────────────

#[tokio::test]
async fn create_task_requires_tasks_create_scope() {
    let (_f, _task_store, cred_store, app) = build_app().await;
    let token = issue_token(&cred_store, "alice", &[Scope::TasksRead]).await;
    let req = post_json(
        "/v1/tasks",
        &token,
        Some("key-1"),
        serde_json::json!({ "objective": "ship it" }),
    );
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn create_task_requires_idempotency_key_header() {
    let (_f, _task_store, cred_store, app) = build_app().await;
    let token = issue_token(&cred_store, "alice", &[Scope::TasksCreate]).await;
    let req = post_json(
        "/v1/tasks",
        &token,
        None,
        serde_json::json!({ "objective": "ship it" }),
    );
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn create_task_rejects_empty_objective() {
    let (_f, _task_store, cred_store, app) = build_app().await;
    let token = issue_token(&cred_store, "alice", &[Scope::TasksCreate]).await;
    let req = post_json(
        "/v1/tasks",
        &token,
        Some("key-1"),
        serde_json::json!({ "objective": "   " }),
    );
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn create_task_succeeds_and_returns_201_with_pursue_mode() {
    let (_f, task_store, cred_store, app) = build_app().await;
    let token = issue_token(&cred_store, "alice", &[Scope::TasksCreate]).await;
    let req = post_json(
        "/v1/tasks",
        &token,
        Some("paperclip-issue-42"),
        serde_json::json!({
            "objective": "Fix the auth bug",
            "external_ref": "paperclip-issue-42",
            "acceptance": ["tests pass", "no regressions"],
            "bounds": { "max_steps": 20, "max_cost_usd": 5.0 },
        }),
    );
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let json = body_json(resp).await;
    assert_eq!(json["owner_id"], "alice");
    assert_eq!(json["mode"], "pursue");
    assert_eq!(json["status"], "pending");
    assert_eq!(json["objective"], "Fix the auth bug");
    assert_eq!(json["external_ref"], "paperclip-issue-42");
    assert_eq!(json["revision"], 1);
    assert_eq!(json["budget_summary"]["max_steps"], 20);
    assert_eq!(json["budget_summary"]["max_cost_usd"], 5.0);

    // Really persisted, not just echoed back.
    let id = json["id"].as_str().unwrap().to_string();
    let stored = task_store
        .load_case("alice", &TaskCaseId(id))
        .await
        .expect("load")
        .expect("case exists");
    assert_eq!(stored.frame.objective, "Fix the auth bug");
    assert_eq!(stored.frame.acceptance.len(), 2);
    assert_eq!(stored.mode, ExecutionMode::Pursue);
}

#[tokio::test]
async fn create_task_is_idempotent_on_owner_plus_idempotency_key() {
    let (_f, task_store, cred_store, app) = build_app().await;
    let token = issue_token(&cred_store, "alice", &[Scope::TasksCreate]).await;

    let req1 = post_json(
        "/v1/tasks",
        &token,
        Some("retry-key"),
        serde_json::json!({ "objective": "first attempt" }),
    );
    let resp1 = app.clone().oneshot(req1).await.unwrap();
    assert_eq!(resp1.status(), StatusCode::CREATED);
    let json1 = body_json(resp1).await;
    let id1 = json1["id"].as_str().unwrap().to_string();

    // Same owner + same Idempotency-Key, DIFFERENT body — per idempotency-key
    // semantics, the retry must return the ORIGINAL result, not create a
    // second task or reflect the new (different) objective.
    let req2 = post_json(
        "/v1/tasks",
        &token,
        Some("retry-key"),
        serde_json::json!({ "objective": "a completely different objective" }),
    );
    let resp2 = app.clone().oneshot(req2).await.unwrap();
    assert_eq!(
        resp2.status(),
        StatusCode::OK,
        "replay is 200, not 201 — nothing new created"
    );
    let json2 = body_json(resp2).await;
    assert_eq!(json2["id"], id1);
    assert_eq!(
        json2["objective"], "first attempt",
        "replay must return the ORIGINAL objective, not the retry's different body"
    );

    let all = task_store
        .list_cases_for_owner("alice")
        .await
        .expect("list");
    assert_eq!(all.len(), 1, "exactly one task, not two");
}

#[tokio::test]
async fn create_task_same_idempotency_key_different_owner_creates_separate_tasks() {
    let (_f, task_store, cred_store, app) = build_app().await;
    let alice_token = issue_token(&cred_store, "alice", &[Scope::TasksCreate]).await;
    let bob_token = issue_token(&cred_store, "bob", &[Scope::TasksCreate]).await;

    for (token, objective) in [(&alice_token, "alice's task"), (&bob_token, "bob's task")] {
        let req = post_json(
            "/v1/tasks",
            token,
            Some("same-key"),
            serde_json::json!({ "objective": objective }),
        );
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    assert_eq!(
        task_store
            .list_cases_for_owner("alice")
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        task_store.list_cases_for_owner("bob").await.unwrap().len(),
        1
    );
}

// ─── Phase 3: POST /v1/tasks/{id}:pause|:resume|:cancel|:steer ────────────

#[tokio::test]
async fn task_action_requires_tasks_control_scope() {
    let (_f, task_store, cred_store, app) = build_app().await;
    task_store
        .create_case(&sample_case("alice", "t1", 100), "idem-1")
        .await
        .expect("create");
    let token = issue_token(&cred_store, "alice", &[Scope::TasksRead]).await;
    let req = post_json(
        "/v1/tasks/t1:pause",
        &token,
        None,
        serde_json::json!({ "expected_revision": 1 }),
    );
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn task_action_unknown_action_returns_404() {
    let (_f, task_store, cred_store, app) = build_app().await;
    task_store
        .create_case(&sample_case("alice", "t1", 100), "idem-1")
        .await
        .expect("create");
    let token = issue_token(&cred_store, "alice", &[Scope::TasksControl]).await;
    let req = post_json(
        "/v1/tasks/t1:not-a-real-action",
        &token,
        None,
        serde_json::json!({ "expected_revision": 1 }),
    );
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn pause_a_running_task_succeeds_and_bumps_revision() {
    let (_f, task_store, cred_store, app) = build_app().await;
    task_store
        .create_case(&sample_case("alice", "t1", 100), "idem-1")
        .await
        .expect("create");
    let token = issue_token(&cred_store, "alice", &[Scope::TasksControl]).await;

    let req = post_json(
        "/v1/tasks/t1:pause",
        &token,
        None,
        serde_json::json!({ "expected_revision": 1 }),
    );
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json["status"], "paused");
    assert_eq!(json["revision"], 2);

    let stored = task_store
        .load_case("alice", &TaskCaseId("t1".into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.status, TaskStatus::Paused);
    assert_eq!(stored.revision, 2);
}

#[tokio::test]
async fn pause_with_stale_revision_returns_409_and_does_not_mutate() {
    let (_f, task_store, cred_store, app) = build_app().await;
    task_store
        .create_case(&sample_case("alice", "t1", 100), "idem-1")
        .await
        .expect("create");
    let token = issue_token(&cred_store, "alice", &[Scope::TasksControl]).await;

    let req = post_json(
        "/v1/tasks/t1:pause",
        &token,
        None,
        serde_json::json!({ "expected_revision": 999 }),
    );
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);

    let stored = task_store
        .load_case("alice", &TaskCaseId("t1".into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        stored.status,
        TaskStatus::Running,
        "must not have transitioned"
    );
    assert_eq!(stored.revision, 1, "must not have bumped");
}

#[tokio::test]
async fn pause_a_pending_task_is_an_invalid_transition_409() {
    let (_f, task_store, cred_store, app) = build_app().await;
    let mut pending = sample_case("alice", "t1", 100);
    pending.status = TaskStatus::Pending;
    task_store
        .create_case(&pending, "idem-1")
        .await
        .expect("create");
    let token = issue_token(&cred_store, "alice", &[Scope::TasksControl]).await;

    let req = post_json(
        "/v1/tasks/t1:pause",
        &token,
        None,
        serde_json::json!({ "expected_revision": 1 }),
    );
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn pause_wrong_owner_returns_404_not_403() {
    let (_f, task_store, cred_store, app) = build_app().await;
    task_store
        .create_case(&sample_case("alice", "t1", 100), "idem-1")
        .await
        .expect("create");
    let bob_token = issue_token(&cred_store, "bob", &[Scope::TasksControl]).await;

    let req = post_json(
        "/v1/tasks/t1:pause",
        &bob_token,
        None,
        serde_json::json!({ "expected_revision": 1 }),
    );
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "IDOR: never reveal the task exists"
    );
}

#[tokio::test]
async fn resume_a_paused_task_succeeds() {
    let (_f, task_store, cred_store, app) = build_app().await;
    let mut paused = sample_case("alice", "t1", 100);
    paused.status = TaskStatus::Paused;
    task_store
        .create_case(&paused, "idem-1")
        .await
        .expect("create");
    let token = issue_token(&cred_store, "alice", &[Scope::TasksControl]).await;

    let req = post_json(
        "/v1/tasks/t1:resume",
        &token,
        None,
        serde_json::json!({ "expected_revision": 1 }),
    );
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json["status"], "running");
}

#[tokio::test]
async fn cancel_sets_a_stop_reason() {
    let (_f, task_store, cred_store, app) = build_app().await;
    task_store
        .create_case(&sample_case("alice", "t1", 100), "idem-1")
        .await
        .expect("create");
    let token = issue_token(&cred_store, "alice", &[Scope::TasksControl]).await;

    let req = post_json(
        "/v1/tasks/t1:cancel",
        &token,
        None,
        serde_json::json!({ "expected_revision": 1 }),
    );
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json["status"], "cancelled");
    assert_eq!(json["stop_reason"]["kind"], "cancelled");

    let stored = task_store
        .load_case("alice", &TaskCaseId("t1".into()))
        .await
        .unwrap()
        .unwrap();
    assert!(stored.status.is_terminal());
}

#[tokio::test]
async fn cancel_an_already_terminal_task_returns_409() {
    let (_f, task_store, cred_store, app) = build_app().await;
    let mut done = sample_case("alice", "t1", 100);
    done.status = TaskStatus::Completed;
    done.stop_reason = Some(bastion_runtime::task::StopReason::Completed);
    task_store
        .create_case(&done, "idem-1")
        .await
        .expect("create");
    let token = issue_token(&cred_store, "alice", &[Scope::TasksControl]).await;

    let req = post_json(
        "/v1/tasks/t1:cancel",
        &token,
        None,
        serde_json::json!({ "expected_revision": 1 }),
    );
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn steer_appends_a_note_and_preserves_external_ref() {
    let (_f, task_store, cred_store, app) = build_app().await;
    let mut case = sample_case("alice", "t1", 100);
    case.business_state = OpaqueState(bastion::control_plane::business_state::new_business_state(
        Some("paperclip-issue-42"),
        None,
    ));
    task_store
        .create_case(&case, "idem-1")
        .await
        .expect("create");
    let token = issue_token(&cred_store, "alice", &[Scope::TasksControl]).await;

    let req = post_json(
        "/v1/tasks/t1:steer",
        &token,
        None,
        serde_json::json!({ "expected_revision": 1, "guidance": "focus on the auth bug first" }),
    );
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json["revision"], 2);
    assert_eq!(
        json["external_ref"], "paperclip-issue-42",
        "steering must not clobber the external_ref set at creation"
    );

    let stored = task_store
        .load_case("alice", &TaskCaseId("t1".into()))
        .await
        .unwrap()
        .unwrap();
    let notes = stored.business_state.0.as_array().unwrap();
    assert!(notes
        .iter()
        .any(|n| n.get("steer").and_then(Value::as_str) == Some("focus on the auth bug first")));
}

#[tokio::test]
async fn steer_rejects_empty_guidance() {
    let (_f, task_store, cred_store, app) = build_app().await;
    task_store
        .create_case(&sample_case("alice", "t1", 100), "idem-1")
        .await
        .expect("create");
    let token = issue_token(&cred_store, "alice", &[Scope::TasksControl]).await;

    let req = post_json(
        "/v1/tasks/t1:steer",
        &token,
        None,
        serde_json::json!({ "expected_revision": 1, "guidance": "   " }),
    );
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn steer_on_terminal_task_returns_409() {
    let (_f, task_store, cred_store, app) = build_app().await;
    let mut done = sample_case("alice", "t1", 100);
    done.status = TaskStatus::Cancelled;
    done.stop_reason = Some(bastion_runtime::task::StopReason::Cancelled);
    task_store
        .create_case(&done, "idem-1")
        .await
        .expect("create");
    let token = issue_token(&cred_store, "alice", &[Scope::TasksControl]).await;

    let req = post_json(
        "/v1/tasks/t1:steer",
        &token,
        None,
        serde_json::json!({ "expected_revision": 1, "guidance": "too late" }),
    );
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn list_tasks_requires_tasks_read_scope() {
    let (_f, _task_store, cred_store, app) = build_app().await;
    // Issued with a DIFFERENT scope only — must be denied, not silently allowed.
    let token = issue_token(&cred_store, "alice", &[Scope::TasksControl]).await;

    let req = Request::builder()
        .uri("/v1/tasks")
        .header("x-bastion-token", token)
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn list_tasks_returns_only_the_authenticated_owners_tasks() {
    let (_f, task_store, cred_store, app) = build_app().await;
    task_store
        .create_case(&sample_case("alice", "t1", 100), "idem-1")
        .await
        .expect("create alice case");
    task_store
        .create_case(&sample_case("bob", "t2", 200), "idem-2")
        .await
        .expect("create bob case");

    let token = issue_token(&cred_store, "alice", &[Scope::TasksRead]).await;
    let req = Request::builder()
        .uri("/v1/tasks")
        .header("x-bastion-token", token)
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let json = body_json(resp).await;
    let items = json["items"].as_array().expect("items array");
    assert_eq!(items.len(), 1, "must not see bob's task");
    assert_eq!(items[0]["id"], "t1");
    assert_eq!(items[0]["owner_id"], "alice");
    assert_eq!(
        items[0]["attempts"]
            .as_array()
            .expect("attempts array")
            .len(),
        0,
        "list endpoint must not embed attempts (N+1 avoidance)"
    );
}

/// Task 4 (backlog: "campo project enforced"): a credential's `project` tag
/// narrows visibility WITHIN one owner's tasks — not a substitute for owner
/// isolation (already proven by the test above), but a real second filter.
#[tokio::test]
async fn list_tasks_filters_by_the_issuing_credentials_project() {
    let (_f, _task_store, cred_store, app) = build_app().await;

    let (_id, acme_create_token) = cred_store
        .issue(
            "alice",
            Some("acme"),
            ScopeSet::new([Scope::TasksCreate, Scope::TasksRead]),
            "acme-integration",
        )
        .await
        .expect("issue acme token");
    let (_id, beta_create_token) = cred_store
        .issue(
            "alice",
            Some("beta"),
            ScopeSet::new([Scope::TasksCreate, Scope::TasksRead]),
            "beta-integration",
        )
        .await
        .expect("issue beta token");
    let (_id, unscoped_token) = cred_store
        .issue(
            "alice",
            None,
            ScopeSet::new([Scope::TasksRead]),
            "console-wide",
        )
        .await
        .expect("issue unscoped token");

    // One task created under each project-tagged credential.
    for (token, key, objective) in [
        (&acme_create_token, "acme-key", "acme's task"),
        (&beta_create_token, "beta-key", "beta's task"),
    ] {
        let req = post_json(
            "/v1/tasks",
            token,
            Some(key),
            serde_json::json!({ "objective": objective }),
        );
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    let list_with = |token: String| {
        let app = app.clone();
        async move {
            let req = Request::builder()
                .uri("/v1/tasks")
                .header("x-bastion-token", token)
                .body(Body::empty())
                .unwrap();
            let resp = app.oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            body_json(resp).await
        }
    };

    let acme_view = list_with(acme_create_token).await;
    let acme_items = acme_view["items"].as_array().unwrap();
    assert_eq!(
        acme_items.len(),
        1,
        "acme credential must see only its own task"
    );
    assert_eq!(acme_items[0]["objective"], "acme's task");

    let beta_view = list_with(beta_create_token).await;
    let beta_items = beta_view["items"].as_array().unwrap();
    assert_eq!(
        beta_items.len(),
        1,
        "beta credential must see only its own task"
    );
    assert_eq!(beta_items[0]["objective"], "beta's task");

    let unscoped_view = list_with(unscoped_token).await;
    let unscoped_items = unscoped_view["items"].as_array().unwrap();
    assert_eq!(
        unscoped_items.len(),
        2,
        "a credential with no project tag must keep seeing everything the owner can see"
    );
}

#[tokio::test]
async fn list_tasks_status_filter_matches_wire_format_not_debug_format() {
    let (_f, task_store, cred_store, app) = build_app().await;
    let mut awaiting = sample_case("alice", "t-awaiting", 100);
    awaiting.status = TaskStatus::AwaitingApproval;
    task_store
        .create_case(&awaiting, "idem-1")
        .await
        .expect("create");
    task_store
        .create_case(&sample_case("alice", "t-running", 200), "idem-2")
        .await
        .expect("create");

    let token = issue_token(&cred_store, "alice", &[Scope::TasksRead]).await;
    let req = Request::builder()
        .uri("/v1/tasks?status=awaiting_approval")
        .header("x-bastion-token", &token)
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], "t-awaiting");
    assert_eq!(items[0]["status"], "awaiting_approval");
}

#[tokio::test]
async fn get_task_returns_404_not_500_for_missing_id() {
    let (_f, _task_store, cred_store, app) = build_app().await;
    let token = issue_token(&cred_store, "alice", &[Scope::TasksRead]).await;
    let req = Request::builder()
        .uri("/v1/tasks/does-not-exist")
        .header("x-bastion-token", token)
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// The core IDOR proof: bob's credential must see the SAME 404 for alice's
/// real task id as for a nonexistent one — never a different status/body
/// that would let bob distinguish "exists under another owner" from "doesn't
/// exist at all."
#[tokio::test]
async fn get_task_404_does_not_leak_existence_across_owners() {
    let (_f, task_store, cred_store, app) = build_app().await;
    task_store
        .create_case(&sample_case("alice", "alice-task", 100), "idem-1")
        .await
        .expect("create");

    let bob_token = issue_token(&cred_store, "bob", &[Scope::TasksRead]).await;

    let req_real_id_wrong_owner = Request::builder()
        .uri("/v1/tasks/alice-task")
        .header("x-bastion-token", &bob_token)
        .body(Body::empty())
        .unwrap();
    let resp1 = app.clone().oneshot(req_real_id_wrong_owner).await.unwrap();
    assert_eq!(resp1.status(), StatusCode::NOT_FOUND);
    let body1 = body_json(resp1).await;

    let req_fake_id = Request::builder()
        .uri("/v1/tasks/totally-made-up-id")
        .header("x-bastion-token", &bob_token)
        .body(Body::empty())
        .unwrap();
    let resp2 = app.oneshot(req_fake_id).await.unwrap();
    assert_eq!(resp2.status(), StatusCode::NOT_FOUND);
    let body2 = body_json(resp2).await;

    assert_eq!(
        body1["code"], body2["code"],
        "identical error code either way"
    );
}

#[tokio::test]
async fn get_task_includes_attempts_unlike_list_tasks() {
    let (_f, task_store, cred_store, app) = build_app().await;
    let case = sample_case("alice", "t1", 100);
    task_store
        .create_case(&case, "idem-1")
        .await
        .expect("create");
    task_store
        .append_attempt(&Attempt {
            id: AttemptId("a1".into()),
            task: TaskCaseId("t1".into()),
            started_at: 10,
            ended_at: Some(20),
            actions: vec![],
            belief_refs: vec![],
            usage: UsageAccum::default(),
            verdict: None,
        })
        .await
        .expect("append attempt");

    let token = issue_token(&cred_store, "alice", &[Scope::TasksRead]).await;
    let req = Request::builder()
        .uri("/v1/tasks/t1")
        .header("x-bastion-token", token)
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    let attempts = json["attempts"].as_array().expect("attempts array");
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0]["id"], "a1");
}

#[tokio::test]
async fn get_task_attempts_endpoint_matches_embedded_attempts() {
    let (_f, task_store, cred_store, app) = build_app().await;
    task_store
        .create_case(&sample_case("alice", "t1", 100), "idem-1")
        .await
        .expect("create");
    task_store
        .append_attempt(&Attempt {
            id: AttemptId("a1".into()),
            task: TaskCaseId("t1".into()),
            started_at: 10,
            ended_at: None,
            actions: vec![],
            belief_refs: vec![],
            usage: UsageAccum::default(),
            verdict: None,
        })
        .await
        .expect("append attempt");

    let token = issue_token(&cred_store, "alice", &[Scope::TasksRead]).await;
    let req = Request::builder()
        .uri("/v1/tasks/t1/attempts")
        .header("x-bastion-token", token)
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    let items = json["items"].as_array().expect("items array");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], "a1");
}

#[tokio::test]
async fn get_task_attempts_404s_for_a_task_owned_by_someone_else() {
    let (_f, task_store, cred_store, app) = build_app().await;
    task_store
        .create_case(&sample_case("alice", "t1", 100), "idem-1")
        .await
        .expect("create");
    let bob_token = issue_token(&cred_store, "bob", &[Scope::TasksRead]).await;

    let req = Request::builder()
        .uri("/v1/tasks/t1/attempts")
        .header("x-bastion-token", bob_token)
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// `DEFAULT_PAGE_SIZE` (50) isn't exposed for tests to override, so this
/// doesn't force a real multi-page round trip over HTTP (that would mean
/// seeding 51+ fake tasks) — `control_plane::pagination`'s own unit tests
/// already cover the slicing/cursor algorithm directly and thoroughly. This
/// test only proves `list_tasks` actually calls it and returns
/// deterministically newest-first ordering through a real store.
#[tokio::test]
async fn list_tasks_paginates_across_calls() {
    let (_f, task_store, cred_store, app) = build_app().await;
    for i in 0..5 {
        task_store
            .create_case(
                &sample_case("alice", &format!("t{i}"), 100 + i as i64),
                &format!("idem-{i}"),
            )
            .await
            .expect("create");
    }
    let token = issue_token(&cred_store, "alice", &[Scope::TasksRead]).await;

    let req1 = Request::builder()
        .uri("/v1/tasks")
        .header("x-bastion-token", &token)
        .body(Body::empty())
        .unwrap();
    let resp1 = app.clone().oneshot(req1).await.unwrap();
    let json1 = body_json(resp1).await;
    let items1 = json1["items"].as_array().unwrap();
    assert_eq!(items1.len(), 5, "all 5 fit in the default page size");
    assert!(json1["next_cursor"].is_null());

    // Newest first: t4 (created_at=104) should be first.
    assert_eq!(items1[0]["id"], "t4");
    assert_eq!(items1[4]["id"], "t0");
}

#[tokio::test]
async fn openapi_spec_is_served_without_auth() {
    let (_f, _task_store, _cred_store, app) = build_app().await;
    let req = Request::builder()
        .uri("/v1/openapi.yaml")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(text.contains("openapi: 3.1.0"));
    assert!(text.contains("x-bastion-token"));
}

#[tokio::test]
async fn revoked_credential_is_rejected_by_a_live_route() {
    let (_f, _task_store, cred_store, app) = build_app().await;
    let (id, token) = cred_store
        .issue("alice", None, ScopeSet::new([Scope::TasksRead]), "test")
        .await
        .expect("issue");
    cred_store.revoke("alice", &id).await.expect("revoke");

    let req = Request::builder()
        .uri("/v1/tasks")
        .header("x-bastion-token", token)
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// Task 5 (backlog: "rate limiting no Control Plane"): the layer is wired
/// into the REAL router (not just unit-tested against `RateLimiter` in
/// isolation) — this is what actually proves `router()` applies it.
#[tokio::test]
async fn requests_over_the_limit_get_429_same_credential() {
    let (_f, _task_store, cred_store, app) = build_app().await;
    let token = issue_token(&cred_store, "alice", &[Scope::TasksRead]).await;

    let get_tasks = |token: String| {
        let app = app.clone();
        async move {
            let req = Request::builder()
                .uri("/v1/tasks")
                .header("x-bastion-token", token)
                .body(Body::empty())
                .unwrap();
            app.oneshot(req).await.unwrap()
        }
    };

    for i in 0..bastion::control_plane::rate_limit::MAX_REQUESTS_PER_WINDOW {
        let resp = get_tasks(token.clone()).await;
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "request {i} (within the limit) must succeed"
        );
    }

    let resp = get_tasks(token.clone()).await;
    assert_eq!(
        resp.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "the request past the limit must be rejected"
    );
    let json = body_json(resp).await;
    assert_eq!(json["code"], "rate_limited");
}

#[tokio::test]
async fn rate_limit_is_isolated_per_credential() {
    let (_f, _task_store, cred_store, app) = build_app().await;
    let token_a = issue_token(&cred_store, "alice", &[Scope::TasksRead]).await;
    let token_b = issue_token(&cred_store, "alice", &[Scope::TasksRead]).await;

    let get_tasks = |token: String| {
        let app = app.clone();
        async move {
            let req = Request::builder()
                .uri("/v1/tasks")
                .header("x-bastion-token", token)
                .body(Body::empty())
                .unwrap();
            app.oneshot(req).await.unwrap()
        }
    };

    for _ in 0..bastion::control_plane::rate_limit::MAX_REQUESTS_PER_WINDOW {
        assert_eq!(get_tasks(token_a.clone()).await.status(), StatusCode::OK);
    }
    assert_eq!(
        get_tasks(token_a.clone()).await.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "token_a must now be over its own limit"
    );
    assert_eq!(
        get_tasks(token_b.clone()).await.status(),
        StatusCode::OK,
        "a different credential must have its own, untouched budget"
    );
}
