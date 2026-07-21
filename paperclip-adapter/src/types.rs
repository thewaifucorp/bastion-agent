//! Minimal wire types this adapter decodes, hand-transcribed from
//! `docs/en/contracts/control-plane-v1.openapi.yaml` (the frozen v1
//! contract) — NOT a dependency on `bastion`'s internal `control_plane::dto`
//! structs. That's deliberate: this crate exists to prove an external
//! consumer only ever needs the public HTTP contract, never Bastion's Rust
//! types, so importing them here would undercut the whole point.

use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    Running,
    AwaitingApproval,
    Paused,
    Completed,
    Escalated,
    Cancelled,
    Failed,
}

impl TaskStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            TaskStatus::Completed | TaskStatus::Escalated | TaskStatus::Cancelled | TaskStatus::Failed
        )
    }
}

/// Mirrors `control_plane::dto::StopReasonDto` — a TYPED variant per kind,
/// never a free-text status message. The adapter's outcome mapping
/// ([`crate::AdapterOutcome`]) switches on this enum's discriminant, never on
/// any string field's contents — "map terminal status/evidence/usage without
/// parsing terminal prose" (this crate's own design brief) means exactly
/// this: `reason`/`dimension` below are carried through as opaque, DISPLAYED
/// detail, never matched against to decide control flow.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StopReason {
    Completed,
    BudgetExceeded { dimension: String },
    Cancelled,
    AwaitingApproval,
    Impossible { reason: String },
    Escalated { reason: String },
}

#[derive(Debug, Clone, Deserialize)]
pub struct BudgetSummary {
    pub llm_calls: u32,
    pub steps: u32,
    pub total_tokens: u64,
    pub cost_usd: Option<f64>,
    pub cost_coverage: String,
    pub wall_clock_ms: u64,
    pub max_cost_usd: Option<f64>,
    pub max_steps: Option<u32>,
}

/// The subset of `TaskResource` this adapter reads. `attempts` is
/// deliberately omitted — heartbeat/poll/cancel never need evidence detail,
/// only status/budget/revision.
#[derive(Debug, Clone, Deserialize)]
pub struct TaskResource {
    pub id: String,
    pub owner_id: String,
    pub external_ref: Option<String>,
    pub objective: String,
    pub status: TaskStatus,
    pub stop_reason: Option<StopReason>,
    pub created_at: i64,
    pub updated_at: i64,
    pub revision: u64,
    pub budget_summary: BudgetSummary,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ErrorEnvelope {
    pub code: String,
    pub message: String,
    pub request_id: String,
}
