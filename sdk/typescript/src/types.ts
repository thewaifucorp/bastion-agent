/**
 * Typed mirror of the frozen wire contract in
 * `docs/en/contracts/control-plane-v1.openapi.yaml` (source of truth) and
 * `src/control_plane/dto.rs` (the Rust structs these are transcribed from —
 * every field here should trace back to a field there; see that file's own
 * doc comments for what each maps to on the `bastion_runtime::task::TaskCase`
 * side, one level further back).
 *
 * These are plain data shapes only — no runtime validation. `BastionClient`
 * trusts the server's JSON to match; a hostile or buggy server could send
 * something else, same as any other typed HTTP client.
 */

export type TaskMode = "respond" | "act" | "pursue";

export type TaskStatus =
  | "pending"
  | "running"
  | "awaiting_approval"
  | "paused"
  | "completed"
  | "escalated"
  | "cancelled"
  | "failed";

export type StopReason =
  | { kind: "completed" }
  | { kind: "budget_exceeded"; dimension: string }
  | { kind: "cancelled" }
  | { kind: "awaiting_approval" }
  | { kind: "impossible"; reason: string }
  | { kind: "escalated"; reason: string };

export type AttemptVerification = "unverified" | "failed" | "partial" | "succeeded";

export interface BudgetSummary {
  llm_calls: number;
  steps: number;
  total_tokens: number;
  cost_usd: number | null;
  cost_coverage: "reported" | "estimated" | "unknown";
  wall_clock_ms: number;
  max_cost_usd: number | null;
  max_steps: number | null;
}

export interface AttemptSummary {
  id: string;
  started_at: number;
  ended_at: number | null;
  verified: AttemptVerification | null;
  llm_calls: number;
  total_tokens: number;
  cost_usd: number | null;
}

export interface TaskResource {
  id: string;
  owner_id: string;
  external_ref: string | null;
  mode: TaskMode;
  objective: string;
  status: TaskStatus;
  stop_reason: StopReason | null;
  created_at: number;
  updated_at: number;
  /** Optimistic-concurrency token — pass back as `expectedRevision` on any mutation. */
  revision: number;
  budget_summary: BudgetSummary;
  attempts: AttemptSummary[];
}

export interface CreateTaskBounds {
  max_steps?: number;
  max_cost_usd?: number;
}

export interface CreateTaskRequest {
  objective: string;
  external_ref?: string;
  acceptance?: string[];
  bounds?: CreateTaskBounds;
}

export interface TaskListResponse {
  items: TaskResource[];
  next_cursor: string | null;
}

export interface AttemptListResponse {
  items: AttemptSummary[];
  next_cursor: string | null;
}

export interface ErrorEnvelope {
  code: string;
  message: string;
  request_id: string;
}

export type WebhookEventType =
  | "task.created"
  | "task.status_changed"
  | "attempt.completed"
  | "task.escalated"
  | "task.terminal";

export interface WebhookSubscriptionRequest {
  target_url: string;
  event_types?: WebhookEventType[];
}

export interface WebhookSubscriptionResource {
  id: string;
  owner_id: string;
  target_url: string;
  event_types: string[];
  created_at: number;
  /**
   * The HMAC signing secret — present ONLY in the response to the call that
   * created this subscription (`BastionClient.createWebhookSubscription`),
   * absent (key omitted, not `null`) in any other response reusing this
   * shape. There is no way to retrieve a lost secret.
   */
  secret?: string;
}

export interface TaskEventEnvelope {
  event_id: string;
  event_type: WebhookEventType | string;
  schema_version: number;
  task_id: string;
  /** `TaskResource.revision` at the moment this event was raised. */
  revision: number;
  occurred_at: number;
  payload: unknown;
}

/**
 * Thrown for any non-2xx response. Carries the parsed `ErrorEnvelope` when
 * the server returned one (every `/v1/*` error response does, by contract)
 * so callers can branch on `error.code` (e.g. `"stale_revision"`,
 * `"scope_denied"`) without string-matching `message`.
 */
export class BastionApiError extends Error {
  readonly status: number;
  readonly code: string;
  readonly requestId: string;

  constructor(status: number, envelope: ErrorEnvelope) {
    super(`Bastion API error ${status} [${envelope.code}]: ${envelope.message}`);
    this.name = "BastionApiError";
    this.status = status;
    this.code = envelope.code;
    this.requestId = envelope.request_id;
  }
}
