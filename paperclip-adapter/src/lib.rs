//! Proof-of-concept Paperclip <-> Bastion Control Plane adapter (US —
//! External Control Plane and SDK, Phase 5: "Paperclip proof adapter").
//!
//! Paperclip's actual codebase isn't available to this repo, so this crate
//! is a REFERENCE implementation: it shows exactly the three calls an
//! orchestrator needs — [`BastionAdapter::heartbeat`], `::poll`, `::cancel`
//! — built ONLY against the public `/v1/*` HTTP contract
//! (`docs/en/contracts/control-plane-v1.openapi.yaml`), never a Bastion Rust
//! type ([`types`] hand-transcribes the wire shapes it reads). If Paperclip
//! (or any other orchestrator) adopts this crate directly, or just copies
//! its request/response shapes, either is a legitimate use of this proof.
//!
//! Design invariants, each traceable to the planning doc's Phase 5 line
//! items:
//! - **heartbeat creates-or-resumes** using the caller's issue id as
//!   `external_ref` — repeat heartbeats for the same issue never create a
//!   second task (idempotency-key derived from the issue id).
//! - **Terminal outcomes are read from typed fields, never parsed prose** —
//!   [`AdapterOutcome`] switches on `TaskStatus`/`StopReason`'s enum
//!   discriminants ([`types::StopReason`]), never on `stop_reason`'s
//!   `reason`/`dimension` string CONTENTS.
//! - **Session state is exactly `{task_id, revision}`** ([`Session`]),
//!   returned to and re-supplied by the caller — this crate holds no
//!   database of its own; Bastion is the sole source of truth.
//! - **Cancellation always goes through the control API** — `cancel` is a
//!   `POST /v1/tasks/{id}:cancel` call, never a local process kill (this
//!   adapter has no local process to kill in the first place: Bastion runs
//!   Pursue tasks in its own daemon, not as an adapter-spawned child).

pub mod types;

use serde::Serialize;
use types::{ErrorEnvelope, TaskResource, TaskStatus};

/// Everything Paperclip needs to persist between calls for one issue/task —
/// nothing else. The caller owns storing/retrieving this (keyed by issue id,
/// or whatever key makes sense on Paperclip's side); this crate never
/// caches it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AdapterSession {
    pub task_id: String,
    pub revision: u64,
}

/// A typed, non-prose-parsed terminal outcome — see the module doc's second
/// invariant. `None` from [`BastionAdapter::poll`]/`::heartbeat` means the
/// task is still non-terminal (keep polling).
#[derive(Debug, Clone, PartialEq)]
pub enum AdapterOutcome {
    Succeeded,
    Cancelled,
    /// `detail` is host-authored, DISPLAYED text — never matched against by
    /// this crate to decide anything; it is opaque cargo, not a signal.
    Failed { detail: FailureDetail },
    Escalated { detail: String },
}

#[derive(Debug, Clone, PartialEq)]
pub enum FailureDetail {
    BudgetExceeded { dimension: String },
    Impossible { reason: String },
    Unspecified,
}

/// One poll/heartbeat/cancel result: the raw resource (for callers that want
/// more than this crate surfaces), the refreshed [`AdapterSession`], and the
/// typed outcome if the task is now terminal.
#[derive(Debug, Clone)]
pub struct TaskSnapshot {
    pub session: AdapterSession,
    pub status: TaskStatus,
    pub outcome: Option<AdapterOutcome>,
    pub resource: TaskResource,
}

fn outcome_from(resource: &TaskResource) -> Option<AdapterOutcome> {
    if !resource.status.is_terminal() {
        return None;
    }
    Some(match resource.status {
        TaskStatus::Completed => AdapterOutcome::Succeeded,
        TaskStatus::Cancelled => AdapterOutcome::Cancelled,
        TaskStatus::Failed => AdapterOutcome::Failed {
            detail: match &resource.stop_reason {
                Some(types::StopReason::BudgetExceeded { dimension }) => {
                    FailureDetail::BudgetExceeded { dimension: dimension.clone() }
                }
                Some(types::StopReason::Impossible { reason }) => {
                    FailureDetail::Impossible { reason: reason.clone() }
                }
                _ => FailureDetail::Unspecified,
            },
        },
        TaskStatus::Escalated => AdapterOutcome::Escalated {
            detail: match &resource.stop_reason {
                Some(types::StopReason::Escalated { reason }) => reason.clone(),
                _ => String::new(),
            },
        },
        // is_terminal() already restricted us to the four arms above.
        _ => unreachable!("non-terminal status passed is_terminal() check"),
    })
}

fn snapshot_from(resource: TaskResource) -> TaskSnapshot {
    let session = AdapterSession { task_id: resource.id.clone(), revision: resource.revision };
    let outcome = outcome_from(&resource);
    TaskSnapshot { session, status: resource.status, outcome, resource }
}

#[derive(Debug)]
pub enum AdapterError {
    /// The HTTP call itself failed (network, TLS, timeout, ...).
    Transport(reqwest::Error),
    /// A non-2xx response with a well-formed `ErrorEnvelope` body — `code`
    /// is the SAME stable slug the HTTP contract documents
    /// (`not_found`, `stale_revision`, `task_terminal`, `scope_denied`, ...).
    Api { status: u16, code: String, message: String },
    /// A non-2xx response whose body wasn't a well-formed `ErrorEnvelope`
    /// (should not happen against a spec-compliant server; kept distinct
    /// from `Api` so a caller can tell "the server sent a shape we didn't
    /// expect" from "the server told us exactly what went wrong").
    UnexpectedResponse { status: u16, body: String },
}

impl std::fmt::Display for AdapterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AdapterError::Transport(e) => write!(f, "transport error: {e}"),
            AdapterError::Api { status, code, message } => {
                write!(f, "api error {status} ({code}): {message}")
            }
            AdapterError::UnexpectedResponse { status, body } => {
                write!(f, "unexpected response {status}: {body}")
            }
        }
    }
}

impl std::error::Error for AdapterError {}

impl From<reqwest::Error> for AdapterError {
    fn from(e: reqwest::Error) -> Self {
        AdapterError::Transport(e)
    }
}

/// Talks to one Bastion Control Plane deployment. Cheap to clone (wraps a
/// `reqwest::Client`, which is itself an `Arc` internally).
#[derive(Clone)]
pub struct BastionAdapter {
    base_url: String,
    token: String,
    client: reqwest::Client,
}

impl BastionAdapter {
    pub fn new(base_url: impl Into<String>, token: impl Into<String>) -> Self {
        Self { base_url: base_url.into(), token: token.into(), client: reqwest::Client::new() }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url.trim_end_matches('/'), path)
    }

    async fn handle_task_response(resp: reqwest::Response) -> Result<TaskResource, AdapterError> {
        let status = resp.status();
        let body = resp.text().await?;
        if status.is_success() {
            serde_json::from_str::<TaskResource>(&body).map_err(|_| AdapterError::UnexpectedResponse {
                status: status.as_u16(),
                body,
            })
        } else {
            match serde_json::from_str::<ErrorEnvelope>(&body) {
                Ok(env) => Err(AdapterError::Api { status: status.as_u16(), code: env.code, message: env.message }),
                Err(_) => Err(AdapterError::UnexpectedResponse { status: status.as_u16(), body }),
            }
        }
    }

    /// Fetch the current state of a task by Bastion task id — the primitive
    /// `heartbeat`/`poll` both build on.
    async fn get_task(&self, task_id: &str) -> Result<TaskResource, AdapterError> {
        let resp = self
            .client
            .get(self.url(&format!("/v1/tasks/{task_id}")))
            .header("x-bastion-token", &self.token)
            .send()
            .await?;
        Self::handle_task_response(resp).await
    }

    /// Create (or idempotently return) the durable Pursue task for one
    /// `issue_id`. The idempotency key is DERIVED from `issue_id` — repeat
    /// heartbeats before a caller has persisted a [`AdapterSession`] never
    /// create a second task for the same issue.
    async fn create_for_issue(&self, issue_id: &str, objective: &str) -> Result<TaskResource, AdapterError> {
        #[derive(Serialize)]
        struct Body<'a> {
            objective: &'a str,
            external_ref: &'a str,
        }
        let resp = self
            .client
            .post(self.url("/v1/tasks"))
            .header("x-bastion-token", &self.token)
            .header("idempotency-key", format!("paperclip:{issue_id}"))
            .json(&Body { objective, external_ref: issue_id })
            .send()
            .await?;
        Self::handle_task_response(resp).await
    }

    /// `POST /v1/tasks/{id}` for an OCC-guarded `:pause|:resume|:cancel`
    /// action. `body` is `None` for actions with no extra fields beyond
    /// `expected_revision` (pause/resume/cancel).
    async fn transition(&self, task_id: &str, action: &str, expected_revision: u64) -> Result<TaskResource, AdapterError> {
        #[derive(Serialize)]
        struct Body {
            expected_revision: u64,
        }
        let resp = self
            .client
            .post(self.url(&format!("/v1/tasks/{task_id}:{action}")))
            .header("x-bastion-token", &self.token)
            .json(&Body { expected_revision })
            .send()
            .await?;
        Self::handle_task_response(resp).await
    }

    /// Create-or-resume a task for `issue_id`.
    ///
    /// - No `session` yet: idempotently create (or fetch, if an earlier
    ///   heartbeat already created it under the same `issue_id`).
    /// - `session` present and the task is currently `Paused`: resume it.
    /// - `session` present and the task is anything else: just refresh and
    ///   return its current state — a heartbeat never forces a transition a
    ///   non-paused task isn't in.
    pub async fn heartbeat(
        &self,
        issue_id: &str,
        objective: &str,
        session: Option<&AdapterSession>,
    ) -> Result<TaskSnapshot, AdapterError> {
        let resource = match session {
            None => self.create_for_issue(issue_id, objective).await?,
            Some(s) => {
                let current = self.get_task(&s.task_id).await?;
                if current.status == TaskStatus::Paused {
                    self.transition(&s.task_id, "resume", current.revision).await?
                } else {
                    current
                }
            }
        };
        Ok(snapshot_from(resource))
    }

    /// Refresh a task's state — the caller's regular poll loop.
    pub async fn poll(&self, session: &AdapterSession) -> Result<TaskSnapshot, AdapterError> {
        let resource = self.get_task(&session.task_id).await?;
        Ok(snapshot_from(resource))
    }

    /// Cancel a task through the control API. Never a local process kill —
    /// see the module doc's third invariant.
    pub async fn cancel(&self, session: &AdapterSession) -> Result<TaskSnapshot, AdapterError> {
        let resource = self.transition(&session.task_id, "cancel", session.revision).await?;
        Ok(snapshot_from(resource))
    }
}
