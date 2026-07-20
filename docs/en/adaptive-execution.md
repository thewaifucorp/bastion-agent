# Adaptive Execution

Bastion routes every request into one of three progressive lifecycles. The mode
is chosen per request; you can always override it. The vocabulary
(`Respond`/`Act`/`Pursue`, `TaskCase`/`Attempt`/`Evidence`/`Verdict`) is defined
in [bastion-core](https://github.com/thewaifucorp/bastion-core) — the kernel owns
the mechanism, the agent owns activation and controls.

> **Status legend:** ✅ implemented · 🧪 experimental / partial · 🕓 planned.
> This page documents behavior that exists in the code today. It does not
> describe the roadmap as if it were shipped.

## The three modes

| Mode | What it does | Durable record? |
| --- | --- | --- |
| **Respond** | Answers from beliefs/context with no external side effect. The default, cheapest path. | No — never creates a `TaskCase`. |
| **Act** | A single bounded effect with no continuity beyond the turn (one tool loop, deterministic checks). | Ephemeral only, if approval/recovery needs it. |
| **Pursue** | A durable, resumable objective: multiple dependent effects, out-of-turn duration, decomposition or adaptation. | Yes — a `TaskCase` that survives restart. |

The cost/latency contract is anchored in the mode: `Respond` adds no extra LLM
call by default; `Pursue` is the only mode that persists a durable case. Each
mode selection emits per-mode telemetry so you can see why a request cost what
it did.

## Activation and override

- ✅ **NLP activation (console):** typed input in the TUI/console is classified
  into a mode before the turn runs. `Respond` runs a normal turn unchanged;
  `Pursue` persists a `Pending` `TaskCase` and surfaces a one-line notice.
- ✅ **Scheduler activation:** fired schedules also route through mode selection.
- ✅ **Inbound channels:** messages arriving over channels (Telegram, email,
  etc.) route through the same mode selection before their turn runs.
- ✅ **Override:** the classification is a suggestion. The one-line notice
  includes how to override the chosen mode for that request.

## Pursue: the task cockpit

`Pursue` requests become durable `TaskCase`s you inspect and control with
`/task`:

```
/task                       # list your open tasks (same as `list`)
/task inspect <id>          # show a task's attempts, evidence and verdict
/task pause <id>            # pause a running task
/task resume <id>           # resume a paused task
/task steer <id> <text>     # inject guidance into an in-flight task
/task cancel <id>           # cancel a task (records a Cancelled stop reason)
```

Each task is owner-scoped: you only ever see and steer your own tasks. A task
records concrete `Attempt`s, the `Evidence` each attempt captured, and the
`Verdict` a verifier reached — the next step is recomputed after each
observation, not walked from a stored plan.

## Scheduling

✅ Durable, owner-scoped personal schedules that fire intents through the same
adaptive path:

```
/schedule                       # list your schedules
/schedule add every <secs> <intent>   # recurring
/schedule add once  <secs> <intent>   # one-shot
/schedule cancel <id>                  # cancel (alias: revoke)
```

Schedules survive restart. A fired schedule routes through mode selection like
console input.

## Capabilities used inside a task

- ✅ **Browser** — a governed browser capability (navigate / snapshot /
  interact / download / screenshot). Retrieved page content is treated as
  untrusted data, never as instructions; sensitive effects require approval,
  and downloads are SSRF-guarded and symlink-safe.
- ✅ **Coding / delegated runtime** — a `Pursue` objective can drive an external
  agent runtime session (via the runtime registry) and collect diffs/artifacts
  as `Evidence`, verified deterministically before the verdict.
- ✅ **Delegation** — a `Pursue` objective can be decomposed into concurrent
  child tasks coordinated by the core orchestrator (no central DAG).

## Budgets and accountability

- ✅ Each mode carries its own budget/telemetry; `Pursue` tasks have budgets and
  context compaction.
- ✅ Every LLM call has an attributed reason and mode, so the cost of a task is
  auditable after the fact.

## Not in this build (🕓 planned / backlog)

- Offline outcome evaluation by a separate judge service.
- Promotion of learned procedures into shared skills/rules.

These are intentionally out of scope here; the agent does not promise coverage
its harness does not have.
