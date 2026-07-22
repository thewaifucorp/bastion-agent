# Observability

Bastion ships a bundled web app and a real-time event vocabulary so you can
watch the daemon think and work: which personas are speaking, which durable
`Pursue` tasks are running, how their attempts are verified, and
steer/pause/cancel them — visually, without leaving the browser. It is the
same information the TUI and the [Control
Plane](control-plane-security.md) expose.

## The web app (`GET /app`)

The full experience: a Vite/React app (`web/`) served by the daemon itself,
with four tabs (keyboard `1`–`4`) — **vigília** (the live ledger and persona
lanterns), **tarefas** (durable-task table, attempt verdicts,
pause/resume/steer/cancel), **chat** (the same turn the console runs, over
`POST /webhook`), and **config** (the two tokens). Same trust model as
`/ui` below: the shell is static and unauthenticated, every byte of data is
fetched with per-request tokens, and the CSP pins all connections to this
daemon.

The app is embedded into the binary at compile time (`build.rs` picks up
`web/dist` when it exists — releases and the Docker image build it first).
A binary built without it still mounts `/app` and answers with build
instructions; `/ui` below is always available as the zero-build fallback.
Local development: `npm run dev` in `web/` proxies `/v1`, `/events` and
`/webhook` to a running daemon (or the mock).

## The fallback dashboard (`GET /ui`)

Open `http://<daemon>/ui`. The page is served by the daemon itself
(embedded in the binary — no external assets, works offline) and is an
unauthenticated static shell, like `/healthz`: it contains no data. Every
byte it renders is fetched with tokens you provide in the page and which
never leave your browser (localStorage) except toward the daemon itself —
the page's Content-Security-Policy pins all connections to same-origin.

It needs up to two tokens, matching the two surfaces it reads:

| Field | Authenticates | Where it comes from |
|---|---|---|
| owner token | `/events` (live feed) | your webhook owner token (`bastion.toml`/`.env` `OwnerMap`), or a paired-device JWT |
| `bcp_` credential | `/v1/*` (tasks, actions) | `/credential issue dashboard tasks:read,tasks:control` on the daemon console |

With only the owner token you get the live ledger and persona lanterns;
adding the credential lights up the task table, attempt detail, and the
pause/resume/steer/cancel actions (scopes permitting).

## `/credential` (console only)

```
/credential list
/credential issue <label> [scopes]     # default scope: tasks:read
/credential revoke <id>
```

Scopes: `tasks:read`, `tasks:create`, `tasks:control`, `webhooks:manage`
(comma-separated). The plaintext token is printed exactly once — only its
hash is stored. The command is deliberately console-only: minting
credentials is a trusted-host operation and never available over remote
channels or `/v1` itself.

## Event vocabulary on `/events` (SSE)

`GET /events` (authenticated with the owner token) now carries, besides the
mesh `mesh_sync` events:

- **Turn events** (emitted around persona routing): `turn.started`
  (`mode: "cabinet"` when forced via `/cabinet`), `cabinet.started`
  (`personas: [...]`, upfront only for a forced cabinet), `turn.completed`
  (`personas` carries the attribution; `mode: "cabinet"` when more than one
  persona answered), `turn.failed` (no error detail on the wire).
  Limitation: an auto-convened cabinet is only visible post-hoc on
  `turn.completed` — emitting it mid-routing needs a kernel observer port
  (backlogged).
- **Task lifecycle events** (from the adaptive execution loop):
  `task.created`, `task.attempt_started`, `task.action_chosen`,
  `task.action_observed`, `task.verified`, `task.adapted`,
  `task.approval_pending`, `task.status_changed`, `task.terminal` — the
  kernel's own id/status-only metadata, safe for this surface.

Browsers' `EventSource` cannot send the `x-bastion-token` header — consume
the stream with `fetch` + a stream reader, exactly as the dashboard does.

## Control Plane webhook events

Outbound webhook subscribers (see the [threat
model](control-plane-security.md)) now receive all five spec event types:
`task.created`, `task.status_changed`, `task.terminal` (from `/v1`-driven
mutations) plus `attempt.completed` (every attempt verification) and
`task.escalated` (terminal `Escalated`, whether from a single cycle failing
to converge or a delegated parent whose children did not all succeed) from
the execution loop, all through the same signed, durable, retried delivery
queue.
