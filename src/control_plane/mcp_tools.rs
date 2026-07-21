//! MCP tool exposure for the Control Plane (US — External Control Plane and
//! SDK, Phase 5: "MCP alignment"). Exposes exactly the 5 tools the planning
//! doc names — `create_task`, `get_task`, `list_tasks`, `steer_task`,
//! `cancel_task` — each a thin [`Capability`] wrapper around
//! [`super::core_ops`], the same business logic the HTTP `/v1/*` routes
//! (`routes.rs`) call. One implementation, two surfaces.
//!
//! ## Why a DEDICATED registry, not `agent.capability_registry`
//!
//! Every other `Capability` in this codebase (`CompanionEventCapability`,
//! `adaptive::browser`'s tools, ...) is registered into the ONE
//! `agent.capability_registry` that BOTH Bastion's own internal LLM
//! tool-calling loop (`agent/loop_.rs`) AND `BastionMcpServer` dispatch
//! through (`main.rs` clones the same `Arc` into both) — there is no
//! existing precedent in this codebase for a capability being MCP-only.
//!
//! These 5 tools are deliberately the exception. The whole point of the
//! Control Plane (per the planning doc) is letting an EXTERNAL orchestrator
//! (e.g. Paperclip) create/steer/cancel Bastion's durable tasks over a
//! network-exposed, token-authenticated surface — it was never a request to
//! let a running Pursue task's OWN internal LLM reasoning call `create_task`
//! or `cancel_task` on itself (or on a sibling task) as a side effect of
//! "MCP alignment." Registering these into the shared registry would grant
//! that new, unrequested internal capability silently. So `build_registry`
//! below returns a SEPARATE `CapabilityRegistry`
//! (`CapabilityRegistry::new()`, same "minimal, scoped" idiom `main.rs`'s
//! Reflector already uses for an unrelated reason — see the comment at its
//! `reflector_registry` construction), and `BastionMcpServer` gets a second
//! registry field ([`bastion::mcp::server::BastionMcpServer`]'s
//! `control_plane_registry`) it checks IN ADDITION TO the shared one —
//! visible to MCP callers, invisible to Bastion's own tool-calling loop.
//!
//! ## Auth
//!
//! Unlike the HTTP routes, these tools do NOT authenticate against
//! [`super::credential::SqliteCredentialStore`] — MCP callers are already
//! authenticated by `mcp::server::authenticate_token`'s
//! `TokenPermissions.owner_id` before `CapabilityRegistry::invoke` is ever
//! reached (`InvokeCtx.owner`, threaded in by `BastionMcpServer::call_tool`).
//! `core_ops` takes that already-resolved owner directly — there is
//! deliberately no second, MCP-specific credential/scope system here; an
//! MCP token's owner can do everything Control Plane scopes would otherwise
//! gate (read/create/control), same as every other MCP tool today has no
//! per-tool scoping beyond `TokenPermissions.read_only`.

use std::sync::Arc;

use async_trait::async_trait;
use bastion_runtime::capability::{Capability, CapabilityRegistry, InvokeCtx};
use bastion_runtime::task::{StopReason, TaskStatus};
use serde_json::{json, Value};

use super::core_ops::{self, CoreOpError, CoreOpsState};
use super::dto::{CreateTaskBoundsDto, CreateTaskRequest};

/// Map [`CoreOpError`] to an `anyhow::Error` whose text is prefixed with the
/// SAME stable slug HTTP's `ErrorEnvelope.code` uses for the equivalent
/// condition (`not_found`, `task_terminal`, ...) — `rmcp` has no structured
/// error-code channel (a tool failure is flat `Content::text`, see
/// `mcp/server.rs:218`), so a caller that wants to distinguish "stale
/// revision" from "not found" programmatically parses this prefix, exactly
/// as an HTTP client would parse `ErrorEnvelope.code`.
fn map_core_op_error(err: CoreOpError) -> anyhow::Error {
    match err {
        CoreOpError::NotFound => {
            anyhow::anyhow!("not_found: no task with that id is visible to this owner")
        }
        CoreOpError::Terminal(status) => {
            anyhow::anyhow!("task_terminal: task is already {status:?}")
        }
        CoreOpError::InvalidTransition(status) => {
            anyhow::anyhow!("invalid_transition: cannot transition a task in its current status ({status:?})")
        }
        CoreOpError::StaleRevision => anyhow::anyhow!(
            "stale_revision: expected_revision does not match the task's current revision"
        ),
        CoreOpError::Conflict => anyhow::anyhow!("conflict: concurrent modification"),
        CoreOpError::InvalidInput(msg) => anyhow::anyhow!("invalid_input: {msg}"),
        CoreOpError::Internal => anyhow::anyhow!("internal_error: internal error"),
    }
}

fn require_str(args: &Value, field: &str) -> anyhow::Result<String> {
    args.get(field)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("invalid_input: {field} is required and must not be empty"))
}

fn require_u64(args: &Value, field: &str) -> anyhow::Result<u64> {
    args.get(field)
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow::anyhow!("invalid_input: {field} is required and must be a non-negative integer"))
}

pub struct CreateTaskCapability {
    pub state: CoreOpsState,
    schema: Value,
}

impl CreateTaskCapability {
    pub fn new(state: CoreOpsState) -> Self {
        Self {
            state,
            schema: json!({
                "type": "object",
                "properties": {
                    "objective": {"type": "string", "description": "What the task should accomplish"},
                    "idempotency_key": {
                        "type": "string",
                        "description": "Repeating this call with the same key (per-caller) returns the original task instead of creating a duplicate"
                    },
                    "external_ref": {"type": "string", "description": "Opaque caller id, e.g. an issue id"},
                    "acceptance": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Acceptance-criterion descriptions"
                    },
                    "bounds": {
                        "type": "object",
                        "properties": {
                            "max_steps": {"type": "integer"},
                            "max_cost_usd": {"type": "number"}
                        },
                        "additionalProperties": false
                    }
                },
                "required": ["objective", "idempotency_key"],
                "additionalProperties": false
            }),
        }
    }
}

#[async_trait]
impl Capability for CreateTaskCapability {
    fn name(&self) -> &str {
        "create_task"
    }

    fn description(&self) -> &str {
        "Create (or idempotently return) a durable Pursue task Bastion executes toward an objective"
    }

    fn input_schema(&self) -> &Value {
        &self.schema
    }

    async fn invoke(&self, args: Value, ctx: &InvokeCtx) -> anyhow::Result<Value> {
        let objective = require_str(&args, "objective")?;
        let idempotency_key = require_str(&args, "idempotency_key")?;
        let external_ref = args
            .get("external_ref")
            .and_then(Value::as_str)
            .map(str::to_string);
        let acceptance: Vec<String> = args
            .get("acceptance")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        let bounds = args.get("bounds").and_then(|b| {
            if b.is_null() {
                return None;
            }
            Some(CreateTaskBoundsDto {
                max_steps: b.get("max_steps").and_then(Value::as_u64).map(|n| n as u32),
                max_cost_usd: b.get("max_cost_usd").and_then(Value::as_f64),
            })
        });

        let req = CreateTaskRequest {
            objective,
            external_ref,
            acceptance,
            bounds,
        };

        let outcome = core_ops::create_task(&self.state, &ctx.owner, &idempotency_key, req)
            .await
            .map_err(map_core_op_error)?;

        let mut value = serde_json::to_value(&outcome.resource)?;
        if let Value::Object(ref mut map) = value {
            map.insert("created".to_string(), json!(outcome.created));
        }
        Ok(value)
    }

    fn is_local(&self) -> bool {
        true
    }
}

pub struct GetTaskCapability {
    pub state: CoreOpsState,
    schema: Value,
}

impl GetTaskCapability {
    pub fn new(state: CoreOpsState) -> Self {
        Self {
            state,
            schema: json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string", "description": "The task id"}
                },
                "required": ["id"],
                "additionalProperties": false
            }),
        }
    }
}

#[async_trait]
impl Capability for GetTaskCapability {
    fn name(&self) -> &str {
        "get_task"
    }

    fn description(&self) -> &str {
        "Fetch one task's safe summary (status, budget, attempts) by id"
    }

    fn input_schema(&self) -> &Value {
        &self.schema
    }

    async fn invoke(&self, args: Value, ctx: &InvokeCtx) -> anyhow::Result<Value> {
        let id = require_str(&args, "id")?;
        let resource = core_ops::get_task(&self.state, &ctx.owner, &id)
            .await
            .map_err(map_core_op_error)?;
        Ok(serde_json::to_value(resource)?)
    }

    fn is_local(&self) -> bool {
        true
    }
}

pub struct ListTasksCapability {
    pub state: CoreOpsState,
    schema: Value,
}

impl ListTasksCapability {
    pub fn new(state: CoreOpsState) -> Self {
        Self {
            state,
            schema: json!({
                "type": "object",
                "properties": {
                    "status": {"type": "string", "description": "Filter by status, e.g. \"running\""},
                    "cursor": {"type": "string", "description": "Opaque pagination cursor from a previous call's next_cursor"}
                },
                "additionalProperties": false
            }),
        }
    }
}

#[async_trait]
impl Capability for ListTasksCapability {
    fn name(&self) -> &str {
        "list_tasks"
    }

    fn description(&self) -> &str {
        "List the caller's tasks, optionally filtered by status, cursor-paginated"
    }

    fn input_schema(&self) -> &Value {
        &self.schema
    }

    async fn invoke(&self, args: Value, ctx: &InvokeCtx) -> anyhow::Result<Value> {
        let status = args.get("status").and_then(Value::as_str);
        let cursor = args.get("cursor").and_then(Value::as_str);
        let resp = core_ops::list_tasks(&self.state, &ctx.owner, status, cursor)
            .await
            .map_err(map_core_op_error)?;
        Ok(serde_json::to_value(resp)?)
    }

    fn is_local(&self) -> bool {
        true
    }
}

pub struct SteerTaskCapability {
    pub state: CoreOpsState,
    schema: Value,
}

impl SteerTaskCapability {
    pub fn new(state: CoreOpsState) -> Self {
        Self {
            state,
            schema: json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string"},
                    "guidance": {"type": "string", "description": "Fresh guidance to append for the running task"},
                    "expected_revision": {
                        "type": "integer",
                        "description": "The task's current revision (from a prior get_task/list_tasks/create_task call) — rejected if stale"
                    }
                },
                "required": ["id", "guidance", "expected_revision"],
                "additionalProperties": false
            }),
        }
    }
}

#[async_trait]
impl Capability for SteerTaskCapability {
    fn name(&self) -> &str {
        "steer_task"
    }

    fn description(&self) -> &str {
        "Append fresh guidance to a running task, guarded by its expected revision"
    }

    fn input_schema(&self) -> &Value {
        &self.schema
    }

    async fn invoke(&self, args: Value, ctx: &InvokeCtx) -> anyhow::Result<Value> {
        let id = require_str(&args, "id")?;
        let guidance = require_str(&args, "guidance")?;
        let expected_revision = require_u64(&args, "expected_revision")?;
        let resource = core_ops::steer_task(&self.state, &ctx.owner, &id, &guidance, expected_revision)
            .await
            .map_err(map_core_op_error)?;
        Ok(serde_json::to_value(resource)?)
    }

    fn is_local(&self) -> bool {
        true
    }
}

pub struct CancelTaskCapability {
    pub state: CoreOpsState,
    schema: Value,
}

impl CancelTaskCapability {
    pub fn new(state: CoreOpsState) -> Self {
        Self {
            state,
            schema: json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string"},
                    "expected_revision": {
                        "type": "integer",
                        "description": "The task's current revision (from a prior get_task/list_tasks/create_task call) — rejected if stale"
                    }
                },
                "required": ["id", "expected_revision"],
                "additionalProperties": false
            }),
        }
    }
}

#[async_trait]
impl Capability for CancelTaskCapability {
    fn name(&self) -> &str {
        "cancel_task"
    }

    fn description(&self) -> &str {
        "Cancel a non-terminal task through the control API — never a raw process kill, guarded by its expected revision"
    }

    fn input_schema(&self) -> &Value {
        &self.schema
    }

    async fn invoke(&self, args: Value, ctx: &InvokeCtx) -> anyhow::Result<Value> {
        let id = require_str(&args, "id")?;
        let expected_revision = require_u64(&args, "expected_revision")?;
        let resource = core_ops::transition_task(
            &self.state,
            &ctx.owner,
            &id,
            TaskStatus::Cancelled,
            Some(StopReason::Cancelled),
            expected_revision,
            "cancel",
        )
        .await
        .map_err(map_core_op_error)?;
        Ok(serde_json::to_value(resource)?)
    }

    fn is_local(&self) -> bool {
        true
    }
}

/// Build the dedicated MCP-only registry holding exactly these 5 tools — see
/// the module doc comment for why this is NOT `agent.capability_registry`.
pub fn build_registry(state: CoreOpsState) -> CapabilityRegistry {
    let mut registry = CapabilityRegistry::new();
    let caps: Vec<Arc<dyn Capability>> = vec![
        Arc::new(CreateTaskCapability::new(state.clone())),
        Arc::new(GetTaskCapability::new(state.clone())),
        Arc::new(ListTasksCapability::new(state.clone())),
        Arc::new(SteerTaskCapability::new(state.clone())),
        Arc::new(CancelTaskCapability::new(state)),
    ];
    for cap in caps {
        let name = cap.name().to_string();
        if let Err(e) = registry.register(cap) {
            tracing::warn!(event = "control_plane_mcp_tool_register_failed", tool = %name, error = %e);
        }
    }
    registry
}

#[cfg(test)]
mod tests {
    use super::*;
    use bastion_memory::PrivacyTier;
    use bastion_runtime::task::SqliteTaskStore;

    async fn test_state() -> (tempfile::NamedTempFile, CoreOpsState) {
        let f = tempfile::NamedTempFile::new().unwrap();
        let path = f.path().to_str().unwrap().to_owned();
        let task_store = SqliteTaskStore::new(path.clone());
        task_store.init_schema().await.unwrap();
        let webhook_subscription_store =
            Arc::new(super::super::webhook_subscription::SqliteWebhookSubscriptionStore::new(path.clone()));
        webhook_subscription_store.init_schema().await.unwrap();
        let webhook_delivery_store =
            Arc::new(super::super::webhook_delivery::SqliteWebhookDeliveryStore::new(path.clone()));
        webhook_delivery_store.init_schema().await.unwrap();
        (
            f,
            CoreOpsState {
                task_store: Arc::new(task_store),
                webhook_subscription_store,
                webhook_delivery_store,
            },
        )
    }

    fn ctx(owner: &str) -> InvokeCtx {
        InvokeCtx {
            owner: owner.to_string(),
            privacy_tier: Some(PrivacyTier::LocalOnly),
        }
    }

    #[tokio::test]
    async fn build_registry_registers_exactly_the_five_spec_named_tools() {
        let (_f, state) = test_state().await;
        let registry = build_registry(state);
        let mut names = registry.list_names();
        names.sort();
        assert_eq!(names, vec!["cancel_task", "create_task", "get_task", "list_tasks", "steer_task"]);
    }

    #[tokio::test]
    async fn create_task_then_get_task_round_trips_through_the_registry() {
        let (_f, state) = test_state().await;
        let registry = build_registry(state);

        let created = registry
            .invoke(
                "create_task",
                json!({"objective": "write a report", "idempotency_key": "k1"}),
                &ctx("alice"),
            )
            .await
            .expect("create_task should succeed");
        assert_eq!(created.data["created"], json!(true));
        let id = created.data["id"].as_str().unwrap().to_string();

        let fetched = registry
            .invoke("get_task", json!({"id": id}), &ctx("alice"))
            .await
            .expect("get_task should succeed");
        assert_eq!(fetched.data["objective"], json!("write a report"));
    }

    #[tokio::test]
    async fn create_task_is_idempotent_via_the_registry() {
        let (_f, state) = test_state().await;
        let registry = build_registry(state);

        let first = registry
            .invoke(
                "create_task",
                json!({"objective": "a", "idempotency_key": "same"}),
                &ctx("alice"),
            )
            .await
            .unwrap();
        let second = registry
            .invoke(
                "create_task",
                json!({"objective": "a", "idempotency_key": "same"}),
                &ctx("alice"),
            )
            .await
            .unwrap();
        assert_eq!(first.data["id"], second.data["id"]);
        assert_eq!(second.data["created"], json!(false));
    }

    #[tokio::test]
    async fn get_task_is_owner_scoped() {
        let (_f, state) = test_state().await;
        let registry = build_registry(state);

        let created = registry
            .invoke(
                "create_task",
                json!({"objective": "a", "idempotency_key": "k"}),
                &ctx("alice"),
            )
            .await
            .unwrap();
        let id = created.data["id"].as_str().unwrap().to_string();

        let err = registry
            .invoke("get_task", json!({"id": id}), &ctx("mallory"))
            .await
            .expect_err("a different owner must not see alice's task");
        assert!(err.to_string().contains("not_found"));
    }

    #[tokio::test]
    async fn steer_task_reports_stale_revision_distinctly() {
        let (_f, state) = test_state().await;
        let registry = build_registry(state);

        let created = registry
            .invoke(
                "create_task",
                json!({"objective": "a", "idempotency_key": "k"}),
                &ctx("alice"),
            )
            .await
            .unwrap();
        let id = created.data["id"].as_str().unwrap().to_string();

        let err = registry
            .invoke(
                "steer_task",
                json!({"id": id, "guidance": "go faster", "expected_revision": 999}),
                &ctx("alice"),
            )
            .await
            .expect_err("stale revision must be rejected");
        assert!(err.to_string().contains("stale_revision"));
    }

    #[tokio::test]
    async fn cancel_task_transitions_to_cancelled_and_is_then_terminal() {
        let (_f, state) = test_state().await;
        let registry = build_registry(state);

        let created = registry
            .invoke(
                "create_task",
                json!({"objective": "a", "idempotency_key": "k"}),
                &ctx("alice"),
            )
            .await
            .unwrap();
        let id = created.data["id"].as_str().unwrap().to_string();
        let revision = created.data["revision"].as_u64().unwrap();

        let cancelled = registry
            .invoke(
                "cancel_task",
                json!({"id": id, "expected_revision": revision}),
                &ctx("alice"),
            )
            .await
            .expect("cancel_task should succeed");
        assert_eq!(cancelled.data["status"], json!("cancelled"));

        let err = registry
            .invoke(
                "cancel_task",
                json!({"id": id, "expected_revision": cancelled.data["revision"]}),
                &ctx("alice"),
            )
            .await
            .expect_err("cancelling an already-terminal task must fail");
        assert!(err.to_string().contains("task_terminal"));
    }

    #[tokio::test]
    async fn list_tasks_filters_by_status() {
        let (_f, state) = test_state().await;
        let registry = build_registry(state);

        registry
            .invoke(
                "create_task",
                json!({"objective": "a", "idempotency_key": "k1"}),
                &ctx("alice"),
            )
            .await
            .unwrap();

        let listed = registry
            .invoke("list_tasks", json!({"status": "pending"}), &ctx("alice"))
            .await
            .expect("list_tasks should succeed");
        let items = listed.data["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);

        let empty = registry
            .invoke("list_tasks", json!({"status": "cancelled"}), &ctx("alice"))
            .await
            .expect("list_tasks should succeed");
        assert_eq!(empty.data["items"].as_array().unwrap().len(), 0);
    }
}
