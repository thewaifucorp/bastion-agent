"""Typed mirror of the frozen wire contract in
``docs/en/contracts/control-plane-v1.openapi.yaml`` (source of truth) and
``src/control_plane/dto.rs`` (the Rust structs these are transcribed from) --
see ``sdk/typescript/src/types.ts``, this module's field-for-field sibling.

These are ``TypedDict``s, not validated dataclasses: like the TypeScript
SDK's plain ``interface``s, they carry NO runtime validation. ``BastionClient``
trusts the server's JSON to match; a hostile or buggy server could send
something else, same as any other typed HTTP client for either language.
"""

from __future__ import annotations

from typing import List, Literal, Optional, TypedDict, Union

TaskMode = Literal["respond", "act", "pursue"]

TaskStatus = Literal[
    "pending",
    "running",
    "awaiting_approval",
    "paused",
    "completed",
    "escalated",
    "cancelled",
    "failed",
]

AttemptVerification = Literal["unverified", "failed", "partial", "succeeded"]

WebhookEventType = Literal[
    "task.created",
    "task.status_changed",
    "attempt.completed",
    "task.escalated",
    "task.terminal",
]


class StopReasonCompleted(TypedDict):
    kind: Literal["completed"]


class StopReasonBudgetExceeded(TypedDict):
    kind: Literal["budget_exceeded"]
    dimension: str


class StopReasonCancelled(TypedDict):
    kind: Literal["cancelled"]


class StopReasonAwaitingApproval(TypedDict):
    kind: Literal["awaiting_approval"]


class StopReasonImpossible(TypedDict):
    kind: Literal["impossible"]
    reason: str


class StopReasonEscalated(TypedDict):
    kind: Literal["escalated"]
    reason: str


StopReason = Union[
    StopReasonCompleted,
    StopReasonBudgetExceeded,
    StopReasonCancelled,
    StopReasonAwaitingApproval,
    StopReasonImpossible,
    StopReasonEscalated,
]


class BudgetSummary(TypedDict):
    llm_calls: int
    steps: int
    total_tokens: int
    cost_usd: Optional[float]
    cost_coverage: Literal["reported", "estimated", "unknown"]
    wall_clock_ms: int
    max_cost_usd: Optional[float]
    max_steps: Optional[int]


class AttemptSummary(TypedDict):
    id: str
    started_at: int
    ended_at: Optional[int]
    verified: Optional[AttemptVerification]
    llm_calls: int
    total_tokens: int
    cost_usd: Optional[float]


class TaskResource(TypedDict):
    id: str
    owner_id: str
    external_ref: Optional[str]
    mode: TaskMode
    objective: str
    status: TaskStatus
    stop_reason: Optional[StopReason]
    created_at: int
    updated_at: int
    # Optimistic-concurrency token -- pass back as expected_revision on any mutation.
    revision: int
    budget_summary: BudgetSummary
    attempts: List[AttemptSummary]


class CreateTaskBounds(TypedDict, total=False):
    max_steps: int
    max_cost_usd: float


class CreateTaskRequest(TypedDict, total=False):
    objective: str  # required at the API level; total=False only relaxes the other keys
    external_ref: str
    acceptance: List[str]
    bounds: CreateTaskBounds


class TaskListResponse(TypedDict):
    items: List[TaskResource]
    next_cursor: Optional[str]


class AttemptListResponse(TypedDict):
    items: List[AttemptSummary]
    next_cursor: Optional[str]


class ErrorEnvelope(TypedDict):
    code: str
    message: str
    request_id: str


class WebhookSubscriptionRequest(TypedDict, total=False):
    target_url: str  # required; see CreateTaskRequest's total=False note
    event_types: List[WebhookEventType]


class WebhookSubscriptionResource(TypedDict, total=False):
    id: str
    owner_id: str
    target_url: str
    event_types: List[str]
    created_at: int
    # The HMAC signing secret -- present ONLY in the response to the call
    # that created this subscription, absent (key omitted, not None) in any
    # other response reusing this shape. There is no way to retrieve a lost
    # secret.
    secret: str


class TaskEventEnvelope(TypedDict):
    event_id: str
    event_type: str
    schema_version: int
    task_id: str
    # TaskResource.revision at the moment this event was raised.
    revision: int
    occurred_at: int
    payload: object


class BastionApiError(Exception):
    """Raised for any non-2xx response. Carries the parsed ``ErrorEnvelope``
    when the server returned one (every ``/v1/*`` error response does, by
    contract) so callers can branch on ``.code`` (e.g. ``"stale_revision"``,
    ``"scope_denied"``) without string-matching ``.args[0]``.

    ``envelope`` is only *typed* as ``ErrorEnvelope`` -- at runtime it's
    whatever JSON object the server sent back, and a non-conforming server
    (a proxy's error page, a malformed error body) may omit any of these
    keys. Reading with ``.get(..., "")`` instead of direct indexing means a
    malformed envelope degrades to empty-string fields instead of raising a
    ``KeyError`` that would mask the real HTTP status/error.
    """

    def __init__(self, status: int, envelope: ErrorEnvelope) -> None:
        code = envelope.get("code", "")
        message = envelope.get("message", "")
        super().__init__(f"Bastion API error {status} [{code}]: {message}")
        self.status = status
        self.code = code
        self.request_id = envelope.get("request_id", "")
