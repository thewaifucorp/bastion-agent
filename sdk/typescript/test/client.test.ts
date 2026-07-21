import { test } from "node:test";
import assert from "node:assert/strict";
import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { AddressInfo } from "node:net";
import { BastionClient, BastionApiError } from "../src/index.js";

type Handler = (req: IncomingMessage, res: ServerResponse, body: string) => void;

/** Minimal mock Bastion server — records every request it receives. */
async function withMockServer(
  handler: Handler,
  run: (baseUrl: string, requests: { method: string; url: string; headers: IncomingMessage["headers"]; body: string }[]) => Promise<void>,
) {
  const requests: { method: string; url: string; headers: IncomingMessage["headers"]; body: string }[] = [];
  const server = createServer((req, res) => {
    const chunks: Buffer[] = [];
    req.on("data", (c) => chunks.push(c));
    req.on("end", () => {
      const body = Buffer.concat(chunks).toString("utf8");
      requests.push({ method: req.method ?? "", url: req.url ?? "", headers: req.headers, body });
      handler(req, res, body);
    });
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const { port } = server.address() as AddressInfo;
  try {
    await run(`http://127.0.0.1:${port}`, requests);
  } finally {
    await new Promise<void>((resolve) => server.close(() => resolve()));
  }
}

function sendJson(res: ServerResponse, status: number, body: unknown) {
  res.writeHead(status, { "content-type": "application/json" });
  res.end(JSON.stringify(body));
}

test("getTask sends the token header and parses the response", async () => {
  await withMockServer(
    (req, res) => {
      sendJson(res, 200, {
        id: "t1",
        owner_id: "alice",
        external_ref: null,
        mode: "pursue",
        objective: "test",
        status: "running",
        stop_reason: null,
        created_at: 1,
        updated_at: 1,
        revision: 1,
        budget_summary: {
          llm_calls: 0,
          steps: 0,
          total_tokens: 0,
          cost_usd: null,
          cost_coverage: "unknown",
          wall_clock_ms: 0,
          max_cost_usd: null,
          max_steps: null,
        },
        attempts: [],
      });
    },
    async (baseUrl, requests) => {
      const client = new BastionClient({ baseUrl, token: "bcp_test" });
      const task = await client.getTask("t1");
      assert.equal(task.id, "t1");
      assert.equal(requests.length, 1);
      assert.equal(requests[0]!.method, "GET");
      assert.equal(requests[0]!.url, "/v1/tasks/t1");
      assert.equal(requests[0]!.headers["x-bastion-token"], "bcp_test");
    },
  );
});

test("request without a token throws before making any network call", async () => {
  const client = new BastionClient({ baseUrl: "http://127.0.0.1:1" }); // unreachable if it tried
  await assert.rejects(() => client.getTask("t1"), /requires a token/);
});

test("createTask sends an idempotency-key header, generating one if not supplied", async () => {
  await withMockServer(
    (req, res) => {
      sendJson(res, 201, {
        id: "t1",
        owner_id: "alice",
        external_ref: null,
        mode: "pursue",
        objective: "ship it",
        status: "pending",
        stop_reason: null,
        created_at: 1,
        updated_at: 1,
        revision: 1,
        budget_summary: {
          llm_calls: 0,
          steps: 0,
          total_tokens: 0,
          cost_usd: null,
          cost_coverage: "unknown",
          wall_clock_ms: 0,
          max_cost_usd: null,
          max_steps: null,
        },
        attempts: [],
      });
    },
    async (baseUrl, requests) => {
      const client = new BastionClient({ baseUrl, token: "bcp_test" });
      const task = await client.createTask({ objective: "ship it" });
      assert.equal(task.objective, "ship it");
      assert.equal(requests[0]!.method, "POST");
      assert.equal(requests[0]!.url, "/v1/tasks");
      assert.ok(requests[0]!.headers["idempotency-key"], "an idempotency key was generated");
      assert.deepEqual(JSON.parse(requests[0]!.body), { objective: "ship it" });
    },
  );
});

test("createTask uses a caller-supplied idempotency key when given", async () => {
  await withMockServer(
    (req, res) => sendJson(res, 200, { id: "t1" }),
    async (baseUrl, requests) => {
      const client = new BastionClient({ baseUrl, token: "bcp_test" });
      await client.createTask({ objective: "x" }, "my-stable-key");
      assert.equal(requests[0]!.headers["idempotency-key"], "my-stable-key");
    },
  );
});

test("pauseTask posts to the colon-action path with expected_revision", async () => {
  await withMockServer(
    (req, res) => sendJson(res, 200, { id: "t1", status: "paused" }),
    async (baseUrl, requests) => {
      const client = new BastionClient({ baseUrl, token: "bcp_test" });
      await client.pauseTask("t1", 3);
      assert.equal(requests[0]!.method, "POST");
      assert.equal(requests[0]!.url, "/v1/tasks/t1:pause");
      assert.deepEqual(JSON.parse(requests[0]!.body), { expected_revision: 3 });
    },
  );
});

test("steerTask includes guidance alongside expected_revision", async () => {
  await withMockServer(
    (req, res) => sendJson(res, 200, { id: "t1" }),
    async (baseUrl, requests) => {
      const client = new BastionClient({ baseUrl, token: "bcp_test" });
      await client.steerTask("t1", 2, "focus on X");
      assert.equal(requests[0]!.url, "/v1/tasks/t1:steer");
      assert.deepEqual(JSON.parse(requests[0]!.body), {
        expected_revision: 2,
        guidance: "focus on X",
      });
    },
  );
});

test("a non-2xx response throws BastionApiError with the parsed envelope", async () => {
  await withMockServer(
    (req, res) => {
      sendJson(res, 409, { code: "stale_revision", message: "nope", request_id: "req_1" });
    },
    async (baseUrl) => {
      const client = new BastionClient({ baseUrl, token: "bcp_test" });
      await assert.rejects(
        () => client.pauseTask("t1", 1),
        (err: unknown) => {
          assert.ok(err instanceof BastionApiError);
          assert.equal(err.status, 409);
          assert.equal(err.code, "stale_revision");
          assert.equal(err.requestId, "req_1");
          return true;
        },
      );
    },
  );
});

test("listTasks builds the correct query string", async () => {
  await withMockServer(
    (req, res) => sendJson(res, 200, { items: [], next_cursor: null }),
    async (baseUrl, requests) => {
      const client = new BastionClient({ baseUrl, token: "bcp_test" });
      await client.listTasks({ status: "running", cursor: "abc" });
      // Param ORDER isn't semantically meaningful for a query string, so
      // this parses and compares as a set rather than asserting one exact
      // literal ordering that happens to match this client's field-check
      // sequence today.
      const [path, query] = requests[0]!.url.split("?");
      assert.equal(path, "/v1/tasks");
      const params = new URLSearchParams(query);
      assert.equal(params.get("status"), "running");
      assert.equal(params.get("cursor"), "abc");
    },
  );
});

test("tasks() iterates across multiple server-paginated pages", async () => {
  let callCount = 0;
  await withMockServer(
    (req, res) => {
      callCount++;
      const url = new URL(req.url ?? "", "http://x");
      const cursor = url.searchParams.get("cursor");
      if (!cursor) {
        sendJson(res, 200, { items: [{ id: "t1" }, { id: "t2" }], next_cursor: "page2" });
      } else {
        sendJson(res, 200, { items: [{ id: "t3" }], next_cursor: null });
      }
    },
    async (baseUrl) => {
      const client = new BastionClient({ baseUrl, token: "bcp_test" });
      const ids: string[] = [];
      for await (const task of client.tasks()) {
        ids.push((task as { id: string }).id);
      }
      assert.deepEqual(ids, ["t1", "t2", "t3"]);
      assert.equal(callCount, 2);
    },
  );
});

test("createWebhookSubscription posts the request and returns the resource", async () => {
  await withMockServer(
    (req, res) => {
      sendJson(res, 201, {
        id: "sub1",
        owner_id: "alice",
        target_url: "https://example.com/hook",
        event_types: ["task.created"],
        created_at: 1,
      });
    },
    async (baseUrl, requests) => {
      const client = new BastionClient({ baseUrl, token: "bcp_test" });
      const sub = await client.createWebhookSubscription({
        target_url: "https://example.com/hook",
        event_types: ["task.created"],
      });
      assert.equal(sub.id, "sub1");
      assert.equal(requests[0]!.url, "/v1/webhook-subscriptions");
      assert.equal(requests[0]!.method, "POST");
    },
  );
});
