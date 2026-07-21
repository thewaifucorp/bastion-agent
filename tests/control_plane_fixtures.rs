//! Checks that `src/control_plane/dto.rs`'s wire shapes and
//! `docs/en/contracts/control-plane-v1.openapi.yaml` stay in sync (US —
//! External Control Plane and SDK, Phase 1: "API/spec fixtures").
//!
//! The YAML is parsed generically (`serde_norway`, already this repo's YAML
//! library — see `personas/soul.rs`) into a `serde_json::Value`, then for
//! each DTO under test this asserts every key present in a real serialized
//! instance is declared in the fixture's `components.schemas.<Name>.properties`,
//! and every fixture-declared `required` field is actually present. This
//! catches the two ways the two artifacts drift apart: a Rust field added
//! without updating the YAML, or a YAML field promised without a matching
//! Rust field.

use std::collections::BTreeSet;

use bastion::control_plane::dto::{
    AttemptListResponse, AttemptSummaryDto, BudgetSummaryDto, CreateTaskBoundsDto,
    CreateTaskRequest, ErrorEnvelope, StopReasonDto, TaskEventEnvelope, TaskListResponse, TaskMode,
    TaskResource, TaskStatusDto, WebhookSubscriptionRequest, WebhookSubscriptionResource,
};
use serde_json::Value;

const FIXTURE_YAML: &str = include_str!("../docs/en/contracts/control-plane-v1.openapi.yaml");

fn fixture() -> Value {
    serde_norway::from_str(FIXTURE_YAML).expect("fixture must parse as YAML")
}

fn schema<'a>(doc: &'a Value, name: &str) -> &'a Value {
    doc.pointer(&format!("/components/schemas/{name}"))
        .unwrap_or_else(|| panic!("fixture missing components.schemas.{name}"))
}

fn declared_properties(schema: &Value) -> BTreeSet<String> {
    schema
        .get("properties")
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("schema has no properties object: {schema:?}"))
        .keys()
        .cloned()
        .collect()
}

fn declared_required(schema: &Value) -> BTreeSet<String> {
    schema
        .get("required")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .map(|v| v.as_str().expect("required entries are strings").to_owned())
                .collect()
        })
        .unwrap_or_default()
}

fn serialized_keys<T: serde::Serialize>(value: &T) -> BTreeSet<String> {
    let json = serde_json::to_value(value).expect("DTO must serialize");
    json.as_object()
        .unwrap_or_else(|| panic!("DTO did not serialize to a JSON object: {json:?}"))
        .keys()
        .cloned()
        .collect()
}

/// Every key Rust actually emits must be declared in the fixture's
/// `properties` — an undeclared field is a silent contract break a caller
/// generating a client from the YAML would never see coming.
fn assert_serialized_keys_are_declared(fixture_schema_name: &str, rust_keys: &BTreeSet<String>) {
    let doc = fixture();
    let schema = schema(&doc, fixture_schema_name);
    let declared = declared_properties(schema);
    let undeclared: Vec<_> = rust_keys.difference(&declared).collect();
    assert!(
        undeclared.is_empty(),
        "{fixture_schema_name}: DTO emits fields not declared in the OpenAPI fixture: \
         {undeclared:?} (declared: {declared:?})"
    );
}

/// Every field the fixture marks `required` must actually appear in a real
/// serialized instance — a required field the DTO sometimes omits would
/// break any client generated strictly from the YAML.
fn assert_required_are_present(fixture_schema_name: &str, rust_keys: &BTreeSet<String>) {
    let doc = fixture();
    let schema = schema(&doc, fixture_schema_name);
    let required = declared_required(schema);
    let missing: Vec<_> = required.difference(rust_keys).collect();
    assert!(
        missing.is_empty(),
        "{fixture_schema_name}: fixture requires fields the DTO does not emit: {missing:?}"
    );
}

fn sample_task_resource() -> TaskResource {
    TaskResource {
        id: "task_123".into(),
        owner_id: "alice".into(),
        external_ref: Some("paperclip-issue-42".into()),
        mode: TaskMode::Pursue,
        objective: "Ship the thing".into(),
        status: TaskStatusDto::Running,
        stop_reason: None,
        created_at: 1,
        updated_at: 2,
        revision: 3,
        budget_summary: sample_budget_summary(),
        attempts: vec![],
    }
}

fn sample_budget_summary() -> BudgetSummaryDto {
    BudgetSummaryDto {
        llm_calls: 1,
        steps: 2,
        total_tokens: 3,
        cost_usd: Some(0.5),
        cost_coverage: "reported".into(),
        wall_clock_ms: 100,
        max_cost_usd: Some(5.0),
        max_steps: Some(10),
    }
}

#[test]
fn fixture_parses() {
    let doc = fixture();
    assert_eq!(
        doc.pointer("/openapi").and_then(Value::as_str),
        Some("3.1.0")
    );
}

#[test]
fn task_resource_matches_fixture() {
    let keys = serialized_keys(&sample_task_resource());
    assert_serialized_keys_are_declared("TaskResource", &keys);
    assert_required_are_present("TaskResource", &keys);
}

#[test]
fn budget_summary_matches_fixture() {
    let keys = serialized_keys(&sample_budget_summary());
    assert_serialized_keys_are_declared("BudgetSummary", &keys);
    assert_required_are_present("BudgetSummary", &keys);
}

#[test]
fn attempt_summary_matches_fixture() {
    let sample = AttemptSummaryDto {
        id: "attempt_1".into(),
        started_at: 1,
        ended_at: Some(2),
        verified: None,
        llm_calls: 1,
        total_tokens: 10,
        cost_usd: None,
    };
    let keys = serialized_keys(&sample);
    assert_serialized_keys_are_declared("AttemptSummary", &keys);
    assert_required_are_present("AttemptSummary", &keys);
}

#[test]
fn create_task_request_matches_fixture() {
    let sample = CreateTaskRequest {
        objective: "Ship the thing".into(),
        external_ref: None,
        acceptance: vec!["tests pass".into()],
        bounds: None,
    };
    let keys = serialized_keys(&sample);
    assert_serialized_keys_are_declared("CreateTaskRequest", &keys);
    assert_required_are_present("CreateTaskRequest", &keys);
}

#[test]
fn task_list_response_matches_fixture() {
    let sample = TaskListResponse {
        items: vec![sample_task_resource()],
        next_cursor: None,
    };
    let keys = serialized_keys(&sample);
    assert_serialized_keys_are_declared("TaskListResponse", &keys);
    assert_required_are_present("TaskListResponse", &keys);
}

#[test]
fn task_status_enum_variants_match_fixture() {
    let doc = fixture();
    let declared: BTreeSet<String> = schema(&doc, "TaskStatus")
        .get("enum")
        .and_then(Value::as_array)
        .expect("TaskStatus has an enum array")
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();

    let rust_variants: BTreeSet<String> = [
        TaskStatusDto::Pending,
        TaskStatusDto::Running,
        TaskStatusDto::AwaitingApproval,
        TaskStatusDto::Paused,
        TaskStatusDto::Completed,
        TaskStatusDto::Escalated,
        TaskStatusDto::Cancelled,
        TaskStatusDto::Failed,
    ]
    .iter()
    .map(|v| {
        let json = serde_json::to_value(v).expect("status serializes");
        json.as_str().expect("status is a string").to_owned()
    })
    .collect();

    assert_eq!(
        declared, rust_variants,
        "TaskStatus enum drifted between the fixture and TaskStatusDto"
    );
}

#[test]
fn attempt_list_response_matches_fixture() {
    let sample = AttemptListResponse {
        items: vec![],
        next_cursor: Some("cursor_abc".into()),
    };
    let keys = serialized_keys(&sample);
    assert_serialized_keys_are_declared("AttemptListResponse", &keys);
    assert_required_are_present("AttemptListResponse", &keys);
}

#[test]
fn create_task_bounds_matches_fixture() {
    let sample = CreateTaskBoundsDto {
        max_steps: Some(10),
        max_cost_usd: Some(1.0),
    };
    let keys = serialized_keys(&sample);
    assert_serialized_keys_are_declared("CreateTaskBounds", &keys);
    assert_required_are_present("CreateTaskBounds", &keys);
}

#[test]
fn error_envelope_matches_fixture() {
    let sample = ErrorEnvelope {
        code: "stale_revision".into(),
        message: "expected_revision did not match".into(),
        request_id: "req_1".into(),
    };
    let keys = serialized_keys(&sample);
    assert_serialized_keys_are_declared("ErrorEnvelope", &keys);
    assert_required_are_present("ErrorEnvelope", &keys);
}

#[test]
fn webhook_subscription_request_matches_fixture() {
    let sample = WebhookSubscriptionRequest {
        target_url: "https://example.com/hooks/bastion".into(),
        event_types: vec!["task.created".into()],
    };
    let keys = serialized_keys(&sample);
    assert_serialized_keys_are_declared("WebhookSubscriptionRequest", &keys);
    assert_required_are_present("WebhookSubscriptionRequest", &keys);
}

#[test]
fn webhook_subscription_resource_matches_fixture() {
    let sample = WebhookSubscriptionResource {
        id: "sub_1".into(),
        owner_id: "alice".into(),
        target_url: "https://example.com/hooks/bastion".into(),
        event_types: vec!["task.created".into()],
        created_at: 1,
        secret: Some("shown-once".into()),
    };
    let keys = serialized_keys(&sample);
    assert_serialized_keys_are_declared("WebhookSubscriptionResource", &keys);
    assert_required_are_present("WebhookSubscriptionResource", &keys);
}

/// `secret` is `skip_serializing_if = "Option::is_none"` — a list-item
/// response (a future endpoint reusing this DTO with `secret: None`) must
/// omit the key entirely, not serialize `"secret": null`, so a client can
/// tell "never shown to you" apart from "the field doesn't exist in this
/// context" by simple key presence.
#[test]
fn webhook_subscription_resource_omits_secret_key_when_none() {
    let sample = WebhookSubscriptionResource {
        id: "sub_1".into(),
        owner_id: "alice".into(),
        target_url: "https://example.com/hooks/bastion".into(),
        event_types: vec![],
        created_at: 1,
        secret: None,
    };
    let json = serde_json::to_value(&sample).unwrap();
    assert!(
        json.as_object().unwrap().get("secret").is_none(),
        "secret key must be entirely absent when None, not present as null"
    );
}

#[test]
fn task_event_envelope_matches_fixture() {
    let sample = TaskEventEnvelope {
        event_id: "evt_1".into(),
        event_type: "task.status_changed".into(),
        schema_version: 1,
        task_id: "task_123".into(),
        revision: 3,
        occurred_at: 1,
        payload: serde_json::json!({}),
    };
    let keys = serialized_keys(&sample);
    assert_serialized_keys_are_declared("TaskEventEnvelope", &keys);
    assert_required_are_present("TaskEventEnvelope", &keys);
}

/// The `webhooks.taskEvent` outbound-delivery description (OpenAPI 3.1's
/// native sibling to `paths`) must actually reference the same
/// `TaskEventEnvelope` schema the DTO test above checks — otherwise the
/// "delivery payload contract" and the "DTO shape" could silently drift
/// apart from each other even while each individually looks consistent.
#[test]
fn webhooks_section_references_task_event_envelope_schema() {
    let doc = fixture();
    let schema_ref = doc
        .pointer("/webhooks/taskEvent/post/requestBody/content/application~1json/schema/$ref")
        .and_then(Value::as_str)
        .expect("webhooks.taskEvent must declare a request body schema $ref");
    assert_eq!(schema_ref, "#/components/schemas/TaskEventEnvelope");
}

#[test]
fn stop_reason_dto_serializes_with_internally_tagged_kind() {
    // Spot-check the one non-trivial serde shape (internally tagged enum) so
    // a future serde attribute change doesn't silently break the fixture's
    // `kind` discriminator contract.
    let json = serde_json::to_value(StopReasonDto::BudgetExceeded {
        dimension: "steps".into(),
    })
    .expect("serializes");
    assert_eq!(json["kind"], "budget_exceeded");
    assert_eq!(json["dimension"], "steps");
}
