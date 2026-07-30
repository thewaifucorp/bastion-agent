# Dev Workboard Mode (spec)

> **Status legend:** ✅ implemented · 🧪 experimental / partial · 🕓 planned.
> Everything on this page is 🕓 **planned** — this is a spec, not a shipped
> feature. It exists to validate the minimum shape needed before any code is
> written, and to give a future Adaptive Execution phase a starting contract
> instead of a blank page.

## Context

Bastion can evolve a work mode for developing/producing artifacts with a
"Paperclip-like" experience — a simple, visible work board — while keeping
its internal mechanics genuinely stigmergic: the next action/persona is
chosen from workboard state, procedural beliefs, and reinforcement signals,
never from a fixed handoff graph. This mode belongs to the WaifuCorp
OSS/Bastion axis; it explicitly does not become a closed enterprise product
or a substitute for any separate, closed, enterprise-hosted
agent-authoring/lifecycle layer — see [Anti-goals](#anti-goals).

This spec builds entirely on [Adaptive Execution](adaptive-execution.md),
already shipped: `Pursue`'s `TaskCase`/`Attempt`/`Evidence`/`Verdict`
vocabulary, the `TaskExecutor`/`Chooser`/`Verifier` seams, and the
stigmergic procedural-memory substrate. Dev Workboard Mode is a product-level
policy layer on top of those — not a new execution mechanism.

## Contract — the workboard/task ledger

A workboard is shared state describing one objective's progress, not a
workflow graph an operator assembles:

- **Objective** — the goal, in the same terms a `Pursue` `TaskCase` already
  carries.
- **Current plan** — the latest recomputed next step, never a stored
  multi-step plan walked in order (Adaptive Execution's own discipline:
  the next step is recomputed after each observation).
- **Status** — where the objective currently stands.
- **Files touched** — provenance for what's changed so far.
- **Decisions** — choices made along the way, and why.
- **Blockers** — what's currently stopping progress, if anything.
- **Acceptance criteria** — what "done" means for this objective.
- **Next actions** — candidates the selection function below is choosing
  among, not a queue.

## Persona/action selection

The next action/persona is a function of:

1. Current workboard state (above).
2. Procedural beliefs relevant to the objective (`bastion-memory`'s
   procedural belief store).
3. Stigmergic weight/pheromone — reinforcement and evaporation, already
   implemented and tested: `reinforce_belief`/`evaporate_beliefs`
   (`tests/stigmergy_mechanism.rs`, e.g.
   `test_reinforce_belief_increases_weight`,
   `test_evaporate_beliefs_reduces_weight_without_crossing_floor`).
4. Verification/error/contestation/cost signals from the most recent
   attempt.

This is a weighted choice over live signals, not a fixed persona handoff —
the same distinction Adaptive Execution already draws between "recompute
after each observation" and "walk a stored plan."

## Bounded loop

Choose next action → execute → verify → update workboard → stop or
continue. This is the exact shape `TaskExecutor`/`Chooser`/`Verifier`
already implement — Dev Workboard Mode reuses them as policy, not as a new
loop:

- [`Chooser`](https://github.com/thewaifucorp/bastion-core/blob/main/crates/bastion-runtime/src/task/ports.rs)
  (`bastion-runtime/src/task/ports.rs`) — picks the next candidate action
  from `CycleHistory`/`Evidence`, deterministically.
- [`TaskExecutor`](https://github.com/thewaifucorp/bastion-core/blob/main/crates/bastion-runtime/src/task/ports.rs) —
  executes one chosen action, producing `Evidence`.
- [`Verifier`](https://github.com/thewaifucorp/bastion-core/blob/main/crates/bastion-runtime/src/task/ports.rs) —
  judges an attempt's evidence into a `Verdict`, deterministically before
  any LLM-judge call (Adaptive Execution's own "deterministic verification
  before any judge" rule).
- `RuntimeTaskExecutor` (`src/adaptive/exec.rs`) — the concrete coding
  `TaskExecutor` already delegating to an external `AgentRuntime`
  (Codex/ACP). Dev Workboard Mode's "execute" step is this, unchanged —
  the workboard is a view/selection layer above it, not a replacement.

## Development persona packs (suggested)

`architect`, `implementer`, `reviewer`, `tester`, `prompt-engineer`,
`researcher` — one plausible starting set from the source note. Not
specified further here: which packs actually ship, and their exact
system prompts/tool allowlists, is implementation scope for the phase that
picks this spec up, not this document.

## Verification, retry, backoff

To compare fairly against Paperclip-style orchestrators, an implementation
needs the same discipline `RuntimeTaskExecutor`/`CodingChooser` already
apply to delegated coding attempts: bounded retries, deterministic
verification off the harness's own terminal status, and backoff between
attempts — not a fresh design, a direct reuse of what `adaptive/exec.rs`
already does for the single-persona coding case, generalized to a
multi-persona workboard.

## Interop

An external orchestrator (Paperclip, Hermes, OpenClaw, or similar) may call
Bastion as a worker/headless executor. The preferred shape for that is
Bastion as a specialized executor with its own memory/personas — not as a
generic central orchestrator being driven by another one.

## Anti-goals

- Not a workflow/DAG engine.
- Not a visual agent builder.
- Does not displace any closed, enterprise-hosted agent-authoring/
  lifecycle/isolation layer — Dev Workboard Mode is OSS Bastion runtime
  scope only: personas, memory, cabinet, tool loop, procedural learning,
  mesh, seams.
- Introduces no closed-enterprise-product concepts into Bastion's code or
  public docs.
- This document is the spec only — it does not implement the workboard.
  The next step (tracked separately) is a short phase that validates the
  minimum viable shape above against a real comparison run.

## Source

Explored and decided in an internal planning note (2026-07-05, "Bastion Dev
Workboard Estigmergico") — this document is that decision's spec, brought
into the Bastion repo per its own recorded next step, not a paraphrase to
be re-litigated here.
