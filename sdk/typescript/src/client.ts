import { generateIdempotencyKey } from "./idempotency.js";
import { paginate } from "./pagination.js";
import type {
  AttemptListResponse,
  CreateTaskRequest,
  ErrorEnvelope,
  TaskListResponse,
  TaskResource,
  TaskStatus,
  WebhookSubscriptionRequest,
  WebhookSubscriptionResource,
} from "./types.js";
import { BastionApiError } from "./types.js";

export interface BastionClientOptions {
  /** e.g. `"http://127.0.0.1:8080"` — no trailing slash needed. */
  baseUrl: string;
  /**
   * The `x-bastion-token` credential (`bcp_<opaque>`, from
   * `src/control_plane/credential.rs`). Optional because
   * {@link BastionClient.getOpenApiSpec} is the one endpoint that doesn't
   * need one — every task/webhook method requires it and throws
   * synchronously if it's missing.
   *
   * BROWSER WARNING (planning doc: "no secret accepted in browser bundles by
   * default"): this SDK does not stop you from constructing a client with a
   * token in browser-run code, but you should not. A token embedded in a
   * browser bundle is readable by anyone who opens devtools. If you need
   * task data in a browser, proxy authenticated calls through your own
   * backend (which holds the token) rather than shipping the token to the
   * client — the constructor logs a warning if it detects a browser-like
   * global (`window`) AND a token, precisely to catch this by accident.
   */
  token?: string;
  /** Injectable for testing; defaults to the global `fetch`. */
  fetch?: typeof fetch;
}

export interface ListTasksParams {
  cursor?: string;
  status?: TaskStatus;
}

export interface ListAttemptsParams {
  cursor?: string;
}

/**
 * Client for Bastion's External Control Plane API (`/v1/*`). One class for
 * both Node and browser use (see {@link BastionClientOptions.token}'s
 * warning) — built on the global `fetch`, no Node-only imports anywhere in
 * this file.
 */
export class BastionClient {
  private readonly baseUrl: string;
  private readonly token: string | undefined;
  private readonly fetchImpl: typeof fetch;

  constructor(options: BastionClientOptions) {
    this.baseUrl = options.baseUrl.replace(/\/+$/, "");
    this.token = options.token;
    this.fetchImpl = options.fetch ?? fetch;

    if (this.token && typeof (globalThis as { window?: unknown }).window !== "undefined") {
      console.warn(
        "[BastionClient] a token was provided in what looks like a browser environment. " +
          "Tokens embedded in browser bundles are readable by anyone who opens devtools — " +
          "proxy authenticated calls through your own backend instead. See BastionClientOptions.token.",
      );
    }
  }

  private async request<T>(
    method: string,
    path: string,
    options: { body?: unknown; headers?: Record<string, string>; requiresAuth?: boolean } = {},
  ): Promise<T> {
    const requiresAuth = options.requiresAuth ?? true;
    if (requiresAuth && !this.token) {
      throw new Error(
        `BastionClient: ${method} ${path} requires a token, but none was provided to the constructor.`,
      );
    }

    const headers: Record<string, string> = { ...options.headers };
    if (requiresAuth && this.token) {
      headers["x-bastion-token"] = this.token;
    }
    // Built incrementally (not `{ method, headers, body: body ?? undefined }`)
    // because `exactOptionalPropertyTypes` treats an explicit `body: undefined`
    // as a type error against `RequestInit.body?: BodyInit | null` — the key
    // must be OMITTED, not present-with-undefined, for a bodyless GET.
    const init: RequestInit = { method, headers };
    if (options.body !== undefined) {
      headers["content-type"] = "application/json";
      init.body = JSON.stringify(options.body);
    }

    const resp = await this.fetchImpl(`${this.baseUrl}${path}`, init);

    if (!resp.ok) {
      let envelope: ErrorEnvelope;
      try {
        envelope = (await resp.json()) as ErrorEnvelope;
      } catch {
        envelope = { code: "unknown", message: resp.statusText, request_id: "" };
      }
      throw new BastionApiError(resp.status, envelope);
    }

    if (resp.status === 204) {
      return undefined as T;
    }
    return (await resp.json()) as T;
  }

  // ─── Reads ────────────────────────────────────────────────────────────

  async listTasks(params: ListTasksParams = {}): Promise<TaskListResponse> {
    const qs = new URLSearchParams();
    if (params.cursor) qs.set("cursor", params.cursor);
    if (params.status) qs.set("status", params.status);
    const query = qs.toString();
    return this.request<TaskListResponse>("GET", `/v1/tasks${query ? `?${query}` : ""}`);
  }

  async getTask(id: string): Promise<TaskResource> {
    return this.request<TaskResource>("GET", `/v1/tasks/${encodeURIComponent(id)}`);
  }

  async listTaskAttempts(id: string, params: ListAttemptsParams = {}): Promise<AttemptListResponse> {
    const qs = new URLSearchParams();
    if (params.cursor) qs.set("cursor", params.cursor);
    const query = qs.toString();
    return this.request<AttemptListResponse>(
      "GET",
      `/v1/tasks/${encodeURIComponent(id)}/attempts${query ? `?${query}` : ""}`,
    );
  }

  /** Iterate every task across all pages. See `pagination.ts`'s doc comment. */
  tasks(params: Omit<ListTasksParams, "cursor"> = {}): AsyncGenerator<TaskResource, void, undefined> {
    // `cursor` is only spread in when defined — same `exactOptionalPropertyTypes`
    // reasoning as `request`'s `init.body` above (an explicit `cursor: undefined`
    // doesn't satisfy `ListTasksParams.cursor?: string`).
    return paginate((cursor) =>
      this.listTasks(cursor === undefined ? params : { ...params, cursor }),
    );
  }

  /** The live OpenAPI contract, as YAML text. Unauthenticated — see the route's own doc comment server-side. */
  async getOpenApiSpec(): Promise<string> {
    const resp = await this.fetchImpl(`${this.baseUrl}/v1/openapi.yaml`);
    if (!resp.ok) throw new Error(`GET /v1/openapi.yaml failed: ${resp.status}`);
    return resp.text();
  }

  // ─── Mutations ────────────────────────────────────────────────────────

  /**
   * `POST /v1/tasks`. If `idempotencyKey` is omitted, one is generated via
   * {@link generateIdempotencyKey} — but see that function's own doc comment
   * on why passing your OWN stable id (e.g. an upstream issue id) is usually
   * better than letting this default kick in.
   */
  async createTask(req: CreateTaskRequest, idempotencyKey?: string): Promise<TaskResource> {
    return this.request<TaskResource>("POST", "/v1/tasks", {
      body: req,
      headers: { "idempotency-key": idempotencyKey ?? (await generateIdempotencyKey()) },
    });
  }

  async pauseTask(id: string, expectedRevision: number): Promise<TaskResource> {
    return this.taskAction(id, "pause", { expected_revision: expectedRevision });
  }

  async resumeTask(id: string, expectedRevision: number): Promise<TaskResource> {
    return this.taskAction(id, "resume", { expected_revision: expectedRevision });
  }

  async cancelTask(id: string, expectedRevision: number): Promise<TaskResource> {
    return this.taskAction(id, "cancel", { expected_revision: expectedRevision });
  }

  async steerTask(id: string, expectedRevision: number, guidance: string): Promise<TaskResource> {
    return this.taskAction(id, "steer", { expected_revision: expectedRevision, guidance });
  }

  private async taskAction(
    id: string,
    action: "pause" | "resume" | "cancel" | "steer",
    body: unknown,
  ): Promise<TaskResource> {
    // Matches the server's routing exactly (src/control_plane/routes.rs's
    // `router` doc comment): a literal `:action` suffix on the SAME path
    // segment as the task id, not a `/action` sub-path.
    return this.request<TaskResource>("POST", `/v1/tasks/${encodeURIComponent(id)}:${action}`, { body });
  }

  // ─── Webhooks ─────────────────────────────────────────────────────────

  /**
   * `POST /v1/webhook-subscriptions`. The response's `secret` field is
   * returned by the SERVER exactly once, in THIS response only — store it
   * yourself immediately; there is no way to retrieve it again (a future
   * list-subscriptions call would reuse the same `WebhookSubscriptionResource`
   * shape with `secret` absent). Pass it to {@link verifyWebhookSignature}
   * when verifying inbound deliveries for this subscription.
   */
  async createWebhookSubscription(
    req: WebhookSubscriptionRequest,
  ): Promise<WebhookSubscriptionResource> {
    return this.request<WebhookSubscriptionResource>("POST", "/v1/webhook-subscriptions", { body: req });
  }
}
