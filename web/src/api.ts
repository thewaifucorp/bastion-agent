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
    /** C0-P4: `POST /proposals` 400s a `persona_edit` whose contract fails
     * `validate_persona_contract` with `{"problems": [...]}` instead of a
     * `code` — carried here so the Personas form can render the SAME
     * problem strings the backend produced, inline, instead of a generic
     * "staging failed" toast. */
    public problems?: string[],
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
    throw new ApiError(
      body?.code ?? `http_${resp.status}`,
      resp.status,
      Array.isArray(body?.problems) ? body.problems : undefined,
    );
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

// ── personas + staged proposals (owner token; A3/A4) ─────────────────────

export type ProposalPayload =
  | { kind: "persona_edit"; slug: string; content: string }
  | {
      kind: "model_config";
      default_model: string | null;
      fallback_models: string[] | null;
    }
  | { kind: "secret_set"; provider_id: string; env_key: string }
  | { kind: "routing_config"; rules: Record<string, string> };

export interface Proposal {
  id: string;
  owner_id: string;
  origin: string;
  payload: ProposalPayload;
  status: "pending" | "approved" | "rejected";
  created_at: number;
  resolved_at: number | null;
}

/** C0-P3's parsed persona contract-v2, as `GET /personas/{slug}` reports it
 * under `contract` — `null` only when the SOUL.md front-matter failed to
 * parse at all (see `PersonaReadResponse.problems` for why). `tools: null`
 * means unrestricted (no allowlist); `tools: []` is a legal-but-suspicious
 * explicit empty allowlist the backend's `validate()` flags as a problem. */
export interface PersonaContract {
  name: string;
  description: string | null;
  objectives: string[];
  goals: string[];
  tools: string[] | null;
  scope: string | null;
  skills: string[];
  privacy_tier: string;
  weight: number;
}

export interface PersonaReadResponse {
  slug: string;
  /** Raw SOUL.md text — always present, even when `contract` is `null`, so
   * the raw-mode escape hatch can still show/fix an unparseable file. */
  content: string;
  contract: PersonaContract | null;
  /** `validate()`'s problems for a successful parse (empty when fully
   * contract-v2-complete), or the one parse-error string when `contract` is
   * `null`. */
  problems: string[];
}

export const personas = {
  list: () =>
    request<{ items: string[] }>(tokens.owner, "/personas"),
  read: (slug: string) =>
    request<PersonaReadResponse>(
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
  /** Stage default model and/or fallback ladder; at least one half required. */
  createModelConfig: (body: {
    default_model?: string;
    fallback_models?: string[];
  }) =>
    request<Proposal>(tokens.owner, "/proposals", {
      method: "POST",
      body: JSON.stringify({ kind: "model_config", ...body }),
    }),
  /** Stage a provider API key. The value travels ONLY in this request body;
   * the daemon pens it in memory until console approval and never echoes it
   * back. Callers must not keep it in state after this resolves. */
  createSecretSet: (provider_id: string, env_key: string, value: string) =>
    request<Proposal>(tokens.owner, "/proposals", {
      method: "POST",
      body: JSON.stringify({ kind: "secret_set", provider_id, env_key, value }),
    }),
  /** Stage per-call-site-class routing rules (A4.5). The map REPLACES the
   * whole routing override on approve; an empty map clears it (classes fall
   * back to bastion.toml's [routing]). */
  createRoutingConfig: (rules: Record<string, string>) =>
    request<Proposal>(tokens.owner, "/proposals", {
      method: "POST",
      body: JSON.stringify({ kind: "routing_config", rules }),
    }),
};

// ── providers + model catalog + config audit (owner token; A4 S2) ────────

export interface ProviderItem {
  id: string;
  /** Human name from the daemon's own provider whitelist (S4: replaces the
   * frontend mirror this view used to keep). */
  display_name: string;
  /** Env key the provider's constructor reads; null for local (ollama) and
   * subscription CLI rows. */
  env_key: string | null;
  kind: "api_key" | "subscription_cli" | "local";
  connected: boolean;
  source: "env" | "secrets_dir" | "auth_profile" | null;
  models_count: number;
}

export interface ModelEntry {
  id: string;
  provider_kind: string;
  display_name: string;
}

export interface ModelsResponse {
  providers: { provider_kind: string; models: ModelEntry[] }[];
  default_model: string;
  fallback_models: string[];
}

export interface ConfigOverride {
  key: string;
  value: unknown;
  origin: string;
  applied_at: number; // unix seconds
}

export const providersApi = {
  list: () => request<{ items: ProviderItem[] }>(tokens.owner, "/providers"),
};

export const modelsApi = {
  get: () => request<ModelsResponse>(tokens.owner, "/models"),
};

/** One row of GET /routing — a call-site class and its effective rule.
 * `supported: false` = the rule is persisted but the daemon has no
 * agent-reachable knob for that class yet (requires core support). */
export interface RoutingItem {
  class: string;
  model: string | null;
  source: "override" | "toml" | null;
  supported: boolean;
}

export const routingApi = {
  get: () => request<{ items: RoutingItem[] }>(tokens.owner, "/routing"),
};

export const configApi = {
  overrides: () =>
    request<{ items: ConfigOverride[] }>(tokens.owner, "/config/overrides"),
};

// ── companion / Buddy (A5 S5) ────────────────────────────────────────────

export interface CompanionNeeds {
  water: number;
  food: number;
  play: number;
  rest: number;
}

/** A static, markup-stripped representative portrait — the pack's idle
 * frame (or a small built-in face when no custom pet pack is loaded). The
 * TUI keeps the full tick-animated experience; this is one still frame. */
export interface CompanionFrame {
  rows: string[];
  width: number;
}

export type CareAction = "water" | "feed" | "play" | "sleep";

export interface CompanionSnapshot {
  game_enabled: boolean;
  level: number;
  xp: number;
  successful_turns: number;
  needs: CompanionNeeds;
  /** Needs currently due, using the same names `POST /companion/care`
   * accepts. */
  cues: string[];
  frame: CompanionFrame;
  pack_name: string;
}

export const companionApi = {
  get: () => request<CompanionSnapshot>(tokens.owner, "/companion"),
  care: (action: CareAction) =>
    request<CompanionSnapshot>(tokens.owner, "/companion/care", {
      method: "POST",
      body: JSON.stringify({ action }),
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
