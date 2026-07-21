"""Spins up a real local HTTP server as a mock Bastion API
(``http.server``), so requests are exercised against actual
``urllib.request`` calls, not a mocked transport -- mirrors
``sdk/typescript``'s documented test approach (a real ``node:http`` server),
ported to Python's stdlib equivalent.
"""

import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import pytest

from bastion_control_plane import BastionApiError, BastionClient

TASK = {
    "id": "t1",
    "owner_id": "alice",
    "external_ref": None,
    "mode": "pursue",
    "objective": "write a report",
    "status": "pending",
    "stop_reason": None,
    "created_at": 1,
    "updated_at": 1,
    "revision": 1,
    "budget_summary": {
        "llm_calls": 0,
        "steps": 0,
        "total_tokens": 0,
        "cost_usd": None,
        "cost_coverage": "unknown",
        "wall_clock_ms": 0,
        "max_cost_usd": None,
        "max_steps": None,
    },
    "attempts": [],
}


class _Handler(BaseHTTPRequestHandler):
    server: "_MockServer"

    def log_message(self, *args):  # silence default stderr logging
        pass

    def _send_json(self, status: int, body) -> None:
        payload = json.dumps(body).encode("utf-8")
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def _record(self) -> None:
        length = int(self.headers.get("content-length", 0))
        body = self.rfile.read(length) if length else b""
        self.server.requests.append(
            {
                "method": self.command,
                "path": self.path,
                # Lowercased: HTTP header names are case-insensitive and
                # urllib.request.Request.add_header title-cases what it
                # sends (e.g. "x-bastion-token" -> "X-bastion-token") --
                # tests assert against lowercase keys, so normalize here
                # rather than special-casing every assertion.
                "headers": {k.lower(): v for k, v in self.headers.items()},
                "body": json.loads(body) if body else None,
            }
        )

    def do_GET(self):
        self._record()
        if self.path == "/v1/openapi.yaml":
            self.send_response(200)
            self.send_header("content-type", "text/yaml")
            self.end_headers()
            self.wfile.write(b"openapi: 3.0.0\n")
            return
        if self.path.startswith("/v1/tasks/") and ":" not in self.path and "attempts" not in self.path:
            self._send_json(200, TASK)
            return
        if self.path.startswith("/v1/tasks"):
            self._send_json(200, {"items": [TASK], "next_cursor": None})
            return
        self._send_json(404, {"code": "not_found", "message": "no route", "request_id": "r1"})

    def do_POST(self):
        self._record()
        if self.path == "/v1/tasks":
            self._send_json(201, TASK)
            return
        if self.path.startswith("/v1/tasks/") and ":" in self.path:
            self._send_json(200, TASK)
            return
        if self.path == "/v1/webhook-subscriptions":
            self._send_json(
                201,
                {
                    "id": "wh1",
                    "owner_id": "alice",
                    "target_url": "https://example.test/hook",
                    "event_types": ["task.created"],
                    "created_at": 1,
                    "secret": "whsec_abc",
                },
            )
            return
        if self.path == "/v1/fail":
            self._send_json(403, {"code": "forbidden", "message": "no scope", "request_id": "r2"})
            return
        self._send_json(404, {"code": "not_found", "message": "no route", "request_id": "r1"})


class _MockServer:
    def __init__(self):
        self.requests = []
        self.httpd = ThreadingHTTPServer(("127.0.0.1", 0), _Handler)
        self.httpd.requests = self.requests  # type: ignore[attr-defined]
        self.thread = threading.Thread(target=self.httpd.serve_forever, daemon=True)
        self.thread.start()

    @property
    def base_url(self) -> str:
        host, port = self.httpd.server_address[:2]
        return f"http://{host}:{port}"

    def close(self):
        self.httpd.shutdown()
        self.httpd.server_close()


@pytest.fixture
def server():
    srv = _MockServer()
    yield srv
    srv.close()


def test_list_tasks_sends_token_and_parses_response(server):
    client = BastionClient(server.base_url, token="bcp_test")
    resp = client.list_tasks()
    assert resp["items"][0]["id"] == "t1"
    assert server.requests[-1]["headers"]["x-bastion-token"] == "bcp_test"


def test_get_task(server):
    client = BastionClient(server.base_url, token="bcp_test")
    task = client.get_task("t1")
    assert task["objective"] == "write a report"
    assert "/v1/tasks/t1" in server.requests[-1]["path"]


def test_create_task_sends_idempotency_key_and_body(server):
    client = BastionClient(server.base_url, token="bcp_test")
    task = client.create_task({"objective": "do the thing"}, idempotency_key="my-key-1")
    assert task["id"] == "t1"
    req = server.requests[-1]
    assert req["headers"]["idempotency-key"] == "my-key-1"
    assert req["body"]["objective"] == "do the thing"


def test_create_task_generates_a_key_when_none_given(server):
    client = BastionClient(server.base_url, token="bcp_test")
    client.create_task({"objective": "x"})
    assert "idempotency-key" in server.requests[-1]["headers"]


def test_task_actions_hit_the_colon_suffixed_path(server):
    client = BastionClient(server.base_url, token="bcp_test")
    client.pause_task("t1", 1)
    assert server.requests[-1]["path"] == "/v1/tasks/t1:pause"
    client.steer_task("t1", 1, "go faster")
    assert server.requests[-1]["path"] == "/v1/tasks/t1:steer"
    assert server.requests[-1]["body"] == {"expected_revision": 1, "guidance": "go faster"}


def test_create_webhook_subscription_returns_the_one_time_secret(server):
    client = BastionClient(server.base_url, token="bcp_test")
    sub = client.create_webhook_subscription({"target_url": "https://example.test/hook"})
    assert sub["secret"] == "whsec_abc"


def test_get_openapi_spec_needs_no_token(server):
    client = BastionClient(server.base_url)  # no token at all
    spec = client.get_openapi_spec()
    assert "openapi" in spec


def test_missing_token_raises_before_any_request_for_authenticated_calls(server):
    client = BastionClient(server.base_url)
    with pytest.raises(RuntimeError):
        client.list_tasks()
    assert server.requests == []


def test_non_2xx_response_raises_bastion_api_error_with_parsed_envelope(server):
    client = BastionClient(server.base_url, token="bcp_test")
    with pytest.raises(BastionApiError) as exc_info:
        # /v1/fail's 403 route is only wired under do_POST -- see _Handler.
        client._request("POST", "/v1/fail")
    err = exc_info.value
    assert err.status == 403
    assert err.code == "forbidden"
    assert err.request_id == "r2"


def test_tasks_iterator_pages_through_results(server):
    client = BastionClient(server.base_url, token="bcp_test")
    seen = list(client.tasks())
    assert [t["id"] for t in seen] == ["t1"]
