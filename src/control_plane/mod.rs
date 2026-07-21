//! Control Plane (US "External Control Plane and SDK").
//!
//! An external HTTP API (`/v1/tasks*`) that lets an outside orchestrator
//! create/list/pause/resume/steer/cancel Bastion's durable `Pursue` tasks
//! without adopting its internal Rust types. See
//! `docs/en/control-plane-security.md` and
//! `docs/en/contracts/control-plane-v1.openapi.yaml`.
//!
//! - Phase 1 shipped the scoped-credential model ([`credential`], [`scope`])
//!   and the frozen v1 wire contract ([`dto`]).
//! - Phase 2 added the read-only routes ([`routes`]), the
//!   `bastion_runtime::task::*` → [`dto`] translation ([`translate`]), and
//!   app-layer cursor pagination ([`pagination`]) — `GET /v1/tasks`,
//!   `GET /v1/tasks/{id}`, `GET /v1/tasks/{id}/attempts`, and
//!   `GET /v1/openapi.yaml`.
//! - Phase 3 adds `POST /v1/tasks` (idempotent create) and
//!   `POST /v1/tasks/{id}:pause|:resume|:cancel|:steer` (OCC-guarded
//!   mutations), plus [`business_state`] — the helpers that store Control
//!   Plane metadata (`external_ref`, steer notes) inside `TaskCase.business_state`
//!   without colliding with `agent::task_command`'s own use of that same
//!   opaque field.
//! - Phase 4 (current) adds `POST /v1/webhook-subscriptions`
//!   ([`webhook_subscription`], SSRF-gated via `adaptive::browser`'s
//!   existing guard) and signed, retried outbound delivery
//!   ([`webhook_delivery`]) for `task.created`/`task.status_changed`/
//!   `task.terminal` — the event types Control Plane's OWN routes can
//!   actually produce.
//! - Phase 5 extracts [`routes`]'s task-mutation/read logic into
//!   [`core_ops`] (a typed-error business-logic layer with no HTTP
//!   awareness) and exposes it a second way, as 5 MCP tools
//!   (`create_task`/`get_task`/`list_tasks`/`steer_task`/`cancel_task`, see
//!   [`mcp_tools`], feature `mcp-server`) — so an MCP-speaking orchestrator
//!   gets the identical task-store behavior, event emission, and error
//!   conditions the HTTP surface already has, from one shared implementation
//!   rather than a second, potentially-drifting one.

pub mod business_state;
pub mod core_ops;
pub mod credential;
pub mod dto;
#[cfg(feature = "mcp-server")]
pub mod mcp_tools;
pub mod pagination;
pub mod routes;
pub mod scope;
pub mod translate;
pub mod webhook_delivery;
pub mod webhook_subscription;
