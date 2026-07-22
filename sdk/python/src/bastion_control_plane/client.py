"""Mirrors ``sdk/typescript/src/client.ts`` -- same methods, same wire
contract, same error shape. Built on ``urllib.request`` (stdlib only,
zero runtime dependencies), the same minimalism the TypeScript SDK's
"one class, built on the global fetch" approach follows.
"""

from __future__ import annotations

import json
import urllib.error
import urllib.parse
import urllib.request
from typing import Iterator, Optional

from .idempotency import generate_idempotency_key
from .pagination import paginate
from .types import (
    AttemptListResponse,
    BastionApiError,
    CreateTaskRequest,
    ErrorEnvelope,
    TaskListResponse,
    TaskResource,
    TaskStatus,
    WebhookSubscriptionRequest,
    WebhookSubscriptionResource,
)


class BastionClient:
    """Client for Bastion's External Control Plane API (``/v1/*``)."""

    def __init__(
        self,
        base_url: str,
        token: Optional[str] = None,
        timeout: float = 30.0,
    ) -> None:
        """
        Args:
            base_url: e.g. ``"http://127.0.0.1:8080"`` -- no trailing slash needed.
            token: The ``x-bastion-token`` credential (``bcp_<opaque>``, from
                ``src/control_plane/credential.rs``). Optional because
                :meth:`get_openapi_spec` is the one endpoint that doesn't
                need one -- every task/webhook method requires it and raises
                synchronously if it's missing.
            timeout: Per-request timeout in seconds, passed to
                ``urllib.request.urlopen``.
        """
        self._base_url = base_url.rstrip("/")
        self._token = token
        self._timeout = timeout

    def _request(
        self,
        method: str,
        path: str,
        body: Optional[dict] = None,
        headers: Optional[dict] = None,
        requires_auth: bool = True,
    ):
        if requires_auth and not self._token:
            raise RuntimeError(
                f"BastionClient: {method} {path} requires a token, but none was provided to the constructor."
            )

        req_headers = dict(headers or {})
        if requires_auth and self._token:
            req_headers["x-bastion-token"] = self._token

        data = None
        if body is not None:
            req_headers["content-type"] = "application/json"
            data = json.dumps(body).encode("utf-8")

        request = urllib.request.Request(
            f"{self._base_url}{path}", data=data, headers=req_headers, method=method
        )
        try:
            with urllib.request.urlopen(request, timeout=self._timeout) as resp:
                return self._parse_response(resp.status, resp.read())
        except urllib.error.HTTPError as e:
            payload = e.read()
            try:
                envelope: ErrorEnvelope = json.loads(payload)
            except (json.JSONDecodeError, UnicodeDecodeError):
                envelope = {"code": "unknown", "message": e.reason or "", "request_id": ""}
            raise BastionApiError(e.code, envelope) from None

    @staticmethod
    def _parse_response(status: int, payload: bytes):
        if status == 204 or not payload:
            return None
        return json.loads(payload)

    # ─── Reads ────────────────────────────────────────────────────────────

    def list_tasks(
        self, cursor: Optional[str] = None, status: Optional[TaskStatus] = None
    ) -> TaskListResponse:
        query = {}
        if cursor:
            query["cursor"] = cursor
        if status:
            query["status"] = status
        qs = f"?{urllib.parse.urlencode(query)}" if query else ""
        return self._request("GET", f"/v1/tasks{qs}")

    def get_task(self, id: str) -> TaskResource:
        return self._request("GET", f"/v1/tasks/{urllib.parse.quote(id, safe='')}")

    def list_task_attempts(self, id: str, cursor: Optional[str] = None) -> AttemptListResponse:
        qs = f"?{urllib.parse.urlencode({'cursor': cursor})}" if cursor else ""
        return self._request(
            "GET", f"/v1/tasks/{urllib.parse.quote(id, safe='')}/attempts{qs}"
        )

    def tasks(self, status: Optional[TaskStatus] = None) -> Iterator[TaskResource]:
        """Iterate every task across all pages. See ``pagination.paginate``'s doc comment."""
        return paginate(lambda cursor: self.list_tasks(cursor=cursor, status=status))

    def get_openapi_spec(self) -> str:
        """The live OpenAPI contract, as YAML text. Unauthenticated -- see the route's own doc comment server-side."""
        request = urllib.request.Request(f"{self._base_url}/v1/openapi.yaml", method="GET")
        with urllib.request.urlopen(request, timeout=self._timeout) as resp:
            return resp.read().decode("utf-8")

    # ─── Mutations ────────────────────────────────────────────────────────

    def create_task(
        self, req: CreateTaskRequest, idempotency_key: Optional[str] = None
    ) -> TaskResource:
        """``POST /v1/tasks``. If ``idempotency_key`` is omitted, one is
        generated via :func:`generate_idempotency_key` -- but see that
        function's own doc comment on why passing your OWN stable id (e.g.
        an upstream issue id) is usually better than letting this default
        kick in.
        """
        return self._request(
            "POST",
            "/v1/tasks",
            body=dict(req),
            headers={"idempotency-key": idempotency_key or generate_idempotency_key()},
        )

    def pause_task(self, id: str, expected_revision: int) -> TaskResource:
        return self._task_action(id, "pause", {"expected_revision": expected_revision})

    def resume_task(self, id: str, expected_revision: int) -> TaskResource:
        return self._task_action(id, "resume", {"expected_revision": expected_revision})

    def cancel_task(self, id: str, expected_revision: int) -> TaskResource:
        return self._task_action(id, "cancel", {"expected_revision": expected_revision})

    def steer_task(self, id: str, expected_revision: int, guidance: str) -> TaskResource:
        return self._task_action(
            id, "steer", {"expected_revision": expected_revision, "guidance": guidance}
        )

    def _task_action(self, id: str, action: str, body: dict) -> TaskResource:
        # Matches the server's routing exactly (src/control_plane/routes.rs's
        # `router` doc comment): a literal `:action` suffix on the SAME path
        # segment as the task id, not a `/action` sub-path.
        return self._request(
            "POST", f"/v1/tasks/{urllib.parse.quote(id, safe='')}:{action}", body=body
        )

    # ─── Webhooks ─────────────────────────────────────────────────────────

    def create_webhook_subscription(
        self, req: WebhookSubscriptionRequest
    ) -> WebhookSubscriptionResource:
        """``POST /v1/webhook-subscriptions``. The response's ``secret`` field
        is returned by the SERVER exactly once, in THIS response only --
        store it yourself immediately; there is no way to retrieve it again.
        """
        return self._request("POST", "/v1/webhook-subscriptions", body=dict(req))
