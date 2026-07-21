// API client for the daemon's three surfaces. Two tokens, mirroring the
// security model: the owner token authenticates /events and /webhook; a
// Control Plane `bcp_` credential authenticates /v1/* (issue one with
// `/credential issue` on the daemon console). Both live in localStorage and
// only ever travel to the daemon itself (same origin).

export const tokens = {
  get owner(): string {
    return localStorage.getItem("bastion.web.owner-token") ?? "";
  },
  set owner(v: string) {
    localStorage.setItem("bastion.web.owner-token", v);
  },
  get cp(): string {
    return localStorage.getItem("bastion.web.cp-token") ?? "";
  },
  set cp(v: string) {
    localStorage.setItem("bastion.web.cp-token", v);
  },
};

export class ApiError extends Error {
  constructor(
    public code: string,
    public status: number,
  ) {
    super(code);
  }
}

async function request<T>(
  token: string,
  path: string,
  init?: RequestInit,
): Promise<T> {
  if (!token) throw new ApiError("token_missing", 0);
  const resp = await fetch(path, {
    ...init,
    headers: {
      "x-bastion-token": token,
      ...(init?.body ? { "content-type": "application/json" } : {}),
    },
  });
  const body = await resp.json().catch(() => ({}));
  if (!resp.ok) {
    throw new ApiError(body?.code ?? `http_${resp.status}`, resp.status);
  }
  return body as T;
}

// ── /v1 (credencial bcp_) ────────────────────────────────────────────────

export interface BudgetSummary {
  llm_calls: number;
  steps: number;
  total_tokens: number;
  cost_usd: number | null;
  cost_coverage: string;
  wall_clock_ms: number;
  max_cost_usd: number | null;
  max_steps: number | null;
}

export interface AttemptSummary {
  id: string;
  started_at: number;
  ended_at: number | null;
  verified: { kind: string; detail?: string } | null;
  llm_calls: number;
  total_tokens: number;
  cost_usd: number | null;
}

export interface Task {
  id: string;
  owner_id: string;
  external_ref: string | null;
  mode: string;
  objective: string;
  status: string;
  stop_reason: { kind: string; detail?: string } | null;
  created_at: number;
  updated_at: number;
  revision: number;
  budget_summary: BudgetSummary;
  attempts: AttemptSummary[];
}

export const v1 = {
  listTasks: () =>
    request<{ items: Task[]; next_cursor: string | null }>(
      tokens.cp,
      "/v1/tasks",
    ),
  getTask: (id: string) =>
    request<Task>(tokens.cp, `/v1/tasks/${encodeURIComponent(id)}`),
  attempts: (id: string) =>
    request<{ items?: AttemptSummary[]; attempts?: AttemptSummary[] }>(
      tokens.cp,
      `/v1/tasks/${encodeURIComponent(id)}/attempts`,
    ),
  action: (
    id: string,
    action: "pause" | "resume" | "cancel" | "steer",
    expectedRevision: number,
    extra?: Record<string, unknown>,
  ) =>
    request<Task>(tokens.cp, `/v1/tasks/${encodeURIComponent(id)}:${action}`, {
      method: "POST",
      body: JSON.stringify({ expected_revision: expectedRevision, ...extra }),
    }),
};

// ── /webhook (owner token) ───────────────────────────────────────────────
// Besides chat, /webhook accepts every Remote-scope slash command
// (/task, /schedule, /model, /backend, /logs, /update, /help, ...) and
// answers with the same text the console prints — the System views render
// that output as terminal blocks.

export const chat = {
  turn: (text: string) =>
    request<{ reply: string }>(tokens.owner, "/webhook", {
      method: "POST",
      body: JSON.stringify({ text }),
    }),
};

export const command = (text: string) => chat.turn(text).then((r) => r.reply);

/** Generic token-authenticated GET — for owner-token JSON routes that are
 * not /v1 (e.g. /loadout). */
export const request2 = <T,>(token: string, path: string) =>
  request<T>(token, path);

// ── personas + staged proposals (owner token; A3) ────────────────────────

export interface Proposal {
  id: string;
  owner_id: string;
  origin: string;
  payload: { kind: "persona_edit"; slug: string; content: string };
  status: "pending" | "approved" | "rejected";
  created_at: number;
  resolved_at: number | null;
}

export const personas = {
  list: () =>
    request<{ items: string[] }>(tokens.owner, "/personas"),
  read: (slug: string) =>
    request<{ slug: string; content: string }>(
      tokens.owner,
      `/personas/${encodeURIComponent(slug)}`,
    ),
};

export const proposalsApi = {
  list: () => request<{ items: Proposal[] }>(tokens.owner, "/proposals"),
  create: (slug: string, content: string) =>
    request<Proposal>(tokens.owner, "/proposals", {
      method: "POST",
      body: JSON.stringify({ kind: "persona_edit", slug, content }),
    }),
};

// ── /status (unauthenticated, booleans-only) ────────────────────────────

export interface RuntimeStatusRow {
  id: string;
  cli_present: boolean;
  logged_in: boolean;
}

export interface StatusSnapshot {
  runtimes: RuntimeStatusRow[];
  ready: boolean;
  update: Record<string, unknown>;
}

export async function status(): Promise<StatusSnapshot> {
  const resp = await fetch("/status");
  if (!resp.ok) throw new ApiError(`http_${resp.status}`, resp.status);
  return resp.json();
}

export async function health(): Promise<{ healthz: boolean; readyz: boolean }> {
  const [h, r] = await Promise.all([
    fetch("/healthz").then((x) => x.ok).catch(() => false),
    fetch("/readyz").then((x) => x.ok).catch(() => false),
  ]);
  return { healthz: h, readyz: r };
}

export async function agentCard(): Promise<Record<string, unknown> | null> {
  const resp = await fetch("/agent-card").catch(() => null);
  if (!resp || !resp.ok) return null;
  return resp.json().catch(() => null);
}

// ── /events (SSE via fetch — EventSource não envia header de auth) ───────

export interface BastionEvent {
  event: string;
  owner?: string;
  session_id?: string;
  mode?: string;
  personas?: string[];
  latency_ms?: number;
  task?: string;
  attempt?: string;
  status?: string;
  [k: string]: unknown;
}

export type ConnState = "off" | "connecting" | "live" | "unauthorized";

/** Abre o stream e entrega cada evento parseado. Reconecta com backoff.
 * Retorna função de cancelamento. */
export function streamEvents(
  onEvent: (ev: BastionEvent) => void,
  onState: (s: ConnState) => void,
): () => void {
  const ctrl = new AbortController();
  (async () => {
    let backoff = 1000;
    while (!ctrl.signal.aborted) {
      if (!tokens.owner) {
        onState("off");
        return;
      }
      onState("connecting");
      try {
        const resp = await fetch("/events", {
          headers: { "x-bastion-token": tokens.owner },
          signal: ctrl.signal,
        });
        if (resp.status === 401) {
          onState("unauthorized");
          return;
        }
        if (!resp.ok || !resp.body) throw new Error(`http ${resp.status}`);
        onState("live");
        backoff = 1000;
        const reader = resp.body.getReader();
        const decoder = new TextDecoder();
        let buf = "";
        for (;;) {
          const { done, value } = await reader.read();
          if (done) break;
          buf += decoder.decode(value, { stream: true });
          let idx;
          while ((idx = buf.indexOf("\n\n")) >= 0) {
            const frame = buf.slice(0, idx);
            buf = buf.slice(idx + 2);
            for (const line of frame.split("\n")) {
              if (!line.startsWith("data:")) continue;
              try {
                onEvent(JSON.parse(line.slice(5).trim()));
              } catch {
                // linha não-JSON no stream: ignorada de propósito
              }
            }
          }
        }
        throw new Error("stream ended");
      } catch {
        if (ctrl.signal.aborted) return;
        onState("connecting");
        await new Promise((r) => setTimeout(r, backoff));
        backoff = Math.min(backoff * 2, 15000);
      }
    }
  })();
  return () => ctrl.abort();
}
