"""Python client for Bastion's External Control Plane API (``/v1/*``).
Mirrors ``sdk/typescript``'s public surface field-for-field against the same
frozen wire contract (``docs/en/contracts/control-plane-v1.openapi.yaml``).
"""

from .client import BastionClient
from .idempotency import generate_idempotency_key
from .pagination import paginate
from .types import (
    AttemptListResponse,
    AttemptSummary,
    AttemptVerification,
    BastionApiError,
    BudgetSummary,
    CreateTaskBounds,
    CreateTaskRequest,
    ErrorEnvelope,
    StopReason,
    TaskEventEnvelope,
    TaskListResponse,
    TaskMode,
    TaskResource,
    TaskStatus,
    WebhookEventType,
    WebhookSubscriptionRequest,
    WebhookSubscriptionResource,
)
from .webhook import verify_webhook_signature

__all__ = [
    "BastionClient",
    "generate_idempotency_key",
    "paginate",
    "verify_webhook_signature",
    "AttemptListResponse",
    "AttemptSummary",
    "AttemptVerification",
    "BastionApiError",
    "BudgetSummary",
    "CreateTaskBounds",
    "CreateTaskRequest",
    "ErrorEnvelope",
    "StopReason",
    "TaskEventEnvelope",
    "TaskListResponse",
    "TaskMode",
    "TaskResource",
    "TaskStatus",
    "WebhookEventType",
    "WebhookSubscriptionRequest",
    "WebhookSubscriptionResource",
]
