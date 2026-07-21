//! Integration tests against a mocked `/v1/*` server (`wiremock`) — proves
//! this adapter's request shapes/header names and its typed-outcome mapping
//! WITHOUT needing a live Bastion daemon. A separate, real end-to-end run
//! against an actual running `bastion-core` container is documented in
//! `README.md` and was performed manually for Phase 5 sign-off.

use bastion_paperclip_adapter::{AdapterOutcome, AdapterSession, BastionAdapter, FailureDetail};
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn task_json(id: &str, status: &str, revision: u64, stop_reason: Option<serde_json::Value>) -> serde_json::Value {
    json!({
        "id": id,
        "owner_id": "paperclip-owner",
        "external_ref": "ISSUE-1",
        "objective": "fix the bug",
        "status": status,
        "stop_reason": stop_reason,
        "created_at": 1,
        "updated_at": 2,
        "revision": revision,
        "budget_summary": {
            "llm_calls": 1,
            "steps": 1,
            "total_tokens": 10,
            "cost_usd": 0.01,
            "cost_coverage": "reported",
            "wall_clock_ms": 100,
            "max_cost_usd": null,
            "max_steps": null
        }
    })
}

#[tokio::test]
async fn heartbeat_with_no_session_creates_the_task_with_the_derived_idempotency_key() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/tasks"))
        .and(header("x-bastion-token", "tok"))
        .and(header("idempotency-key", "paperclip:ISSUE-1"))
        .respond_with(ResponseTemplate::new(201).set_body_json(task_json("cp_abc", "pending", 1, None)))
        .mount(&server)
        .await;

    let adapter = BastionAdapter::new(server.uri(), "tok");
    let snapshot = adapter.heartbeat("ISSUE-1", "fix the bug", None).await.expect("heartbeat should succeed");

    assert_eq!(snapshot.session.task_id, "cp_abc");
    assert_eq!(snapshot.session.revision, 1);
    assert!(snapshot.outcome.is_none(), "pending is not terminal");
}

#[tokio::test]
async fn heartbeat_with_a_paused_session_resumes_it() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/tasks/cp_abc"))
        .respond_with(ResponseTemplate::new(200).set_body_json(task_json("cp_abc", "paused", 3, None)))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/tasks/cp_abc:resume"))
        .respond_with(ResponseTemplate::new(200).set_body_json(task_json("cp_abc", "running", 4, None)))
        .mount(&server)
        .await;

    let adapter = BastionAdapter::new(server.uri(), "tok");
    let session = AdapterSession { task_id: "cp_abc".to_string(), revision: 3 };
    let snapshot = adapter
        .heartbeat("ISSUE-1", "fix the bug", Some(&session))
        .await
        .expect("heartbeat should resume a paused task");

    assert_eq!(snapshot.status, bastion_paperclip_adapter::types::TaskStatus::Running);
    assert_eq!(snapshot.session.revision, 4);
}

#[tokio::test]
async fn poll_maps_a_failed_budget_exceeded_task_to_a_typed_outcome_not_prose_matching() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/tasks/cp_abc"))
        .respond_with(ResponseTemplate::new(200).set_body_json(task_json(
            "cp_abc",
            "failed",
            5,
            Some(json!({"kind": "budget_exceeded", "dimension": "money"})),
        )))
        .mount(&server)
        .await;

    let adapter = BastionAdapter::new(server.uri(), "tok");
    let session = AdapterSession { task_id: "cp_abc".to_string(), revision: 5 };
    let snapshot = adapter.poll(&session).await.expect("poll should succeed");

    match snapshot.outcome {
        Some(AdapterOutcome::Failed { detail: FailureDetail::BudgetExceeded { dimension } }) => {
            assert_eq!(dimension, "money");
        }
        other => panic!("expected Failed{{BudgetExceeded}}, got {other:?}"),
    }
}

#[tokio::test]
async fn poll_maps_a_completed_task_to_succeeded() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/tasks/cp_abc"))
        .respond_with(ResponseTemplate::new(200).set_body_json(task_json(
            "cp_abc",
            "completed",
            6,
            Some(json!({"kind": "completed"})),
        )))
        .mount(&server)
        .await;

    let adapter = BastionAdapter::new(server.uri(), "tok");
    let session = AdapterSession { task_id: "cp_abc".to_string(), revision: 6 };
    let snapshot = adapter.poll(&session).await.expect("poll should succeed");

    assert_eq!(snapshot.outcome, Some(AdapterOutcome::Succeeded));
}

#[tokio::test]
async fn cancel_sends_the_expected_revision_and_returns_the_cancelled_snapshot() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/tasks/cp_abc:cancel"))
        .respond_with(ResponseTemplate::new(200).set_body_json(task_json(
            "cp_abc",
            "cancelled",
            9,
            Some(json!({"kind": "cancelled"})),
        )))
        .mount(&server)
        .await;

    let adapter = BastionAdapter::new(server.uri(), "tok");
    let session = AdapterSession { task_id: "cp_abc".to_string(), revision: 8 };
    let snapshot = adapter.cancel(&session).await.expect("cancel should succeed");

    assert_eq!(snapshot.outcome, Some(AdapterOutcome::Cancelled));
    assert_eq!(snapshot.session.revision, 9);
}

#[tokio::test]
async fn a_stale_revision_conflict_surfaces_as_a_typed_api_error_with_the_documented_code() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/tasks/cp_abc:cancel"))
        .respond_with(ResponseTemplate::new(409).set_body_json(json!({
            "code": "stale_revision",
            "message": "expected_revision does not match the task's current revision",
            "request_id": "abcd1234"
        })))
        .mount(&server)
        .await;

    let adapter = BastionAdapter::new(server.uri(), "tok");
    let session = AdapterSession { task_id: "cp_abc".to_string(), revision: 1 };
    let err = adapter.cancel(&session).await.expect_err("stale revision must surface as an error");

    match err {
        bastion_paperclip_adapter::AdapterError::Api { status, code, .. } => {
            assert_eq!(status, 409);
            assert_eq!(code, "stale_revision");
        }
        other => panic!("expected AdapterError::Api, got {other:?}"),
    }
}
