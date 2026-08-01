# Release surfaces — `/app` and the TUI

Gate 4/5 of `bastion-agent#32` (`agent/release-v0.3.0`): classify the two
end-user surfaces this repo ships (the embedded web app at `GET /app` and
the terminal UI, `bastion tui`) so the release notes can say what's safe to
rely on and what's still rough, plus a concrete UAT checklist.

> **Status legend:** ✅ implemented · 🧪 experimental / partial · 🕓 planned.

This classification comes from reading the actual routing/view code
(`src/webapp.rs`, `web/src/App.tsx` and its views, `src/tui.rs`) — it is not
a substitute for a human actually opening the app and using it. See
"Manual UAT checklist" below for what still needs a person.

## `/app` — the embedded web app (`web/`, served at `GET /app`)

Same-origin SPA, CSP-pinned so the bundle cannot call out anywhere but this
daemon (`src/webapp.rs`). Two tokens gate it (owner token for
`/events`/`/webhook`/chat, a `bcp_` credential for `/v1/*` task control),
entered once under **Connection** and kept in `localStorage` only. Built
WITHOUT `web/dist` (every local `cargo build` and the CI `rust` job), `GET
/app` answers with plain-text guidance instead of a broken page — this is
tested (`webapp.rs::absent_build_answers_with_guidance_not_a_broken_page`).

| View | Status | Notes |
|---|---|---|
| Connection (token entry) | ✅ | Local-only token storage, no server round-trip to save. |
| Overview / Loadout / Live feed | ✅ | Read-only, driven by `/events` SSE — no write path to get wrong. |
| Chat | ✅ | Thin wrapper over the same console turn/command router every other surface uses (`chat.turn`). |
| Tasks / Schedules / Personas | ✅ | REST CRUD against the existing `/v1/*`/`/schedule`/`/as` surfaces. |
| Buddy | ✅ | Reads/writes companion state via the existing API; re-fetches on `companion.updated`. |
| Models | 🧪 | Stages a `model_config` proposal (default + fallback ladder) for **catalog (API-key) models only** — applying still requires `/proposal approve <id>` on the console. **Cannot select or even see a subscription-backed model** (`/model codex/...`, BACOMP-01) — that syntax has no web UI today, console/TUI only. |
| Providers | 🧪 | Shows connection status and stages API-key `secret_set` proposals (same apply-on-console pattern as Models). For `subscription_cli` providers it only shows a status dot and points to `/connect` on the console — no way to start or complete a subscription login from the web. |
| Backends / Logs / Update | ✅ | Thin `CommandView` passthrough to `/backend`, `/logs`, `/update` — no web-specific logic to diverge from the console's own behavior. |
| About | ✅ | Static. |

**Known gap (🕓 planned):** the subscription login/model-selection flow
(BAAUTH `/auth connect|status|disconnect`, BACOMP `/model <provider>/<model>`)
has zero web surface. An operator who only ever uses `/app` cannot connect
or use a subscription today — this is a real, not cosmetic, gap for 0.3.0.

## TUI (`bastion tui`, `src/tui.rs`)

| Flow | Status | Notes |
|---|---|---|
| Pairing (fresh session) | ✅ | Plain-terminal OTC exchange (`pair()`, `src/tui.rs:323`) before raw mode is entered; local installs skip straight to a bootstrap token. |
| Session-expiry recovery | ✅ | An `Unauthorized` turn result drops out of the alternate screen, clears the stale session, re-pairs (bootstrap token or a fresh OTC prompt), and resumes — tested path, not just a crash-and-reconnect. |
| `/model` picker | ✅ (🧪 discovery gap) | Typing `/model <id>` works for any resolver-supported model, including the subscription syntax `codex/gpt-5[@profile]` — but `MODEL_COMMANDS` (`src/tui.rs:559`) only autocompletes the fixed API-key shortlist, so a subscription model is reachable only if the operator already knows the syntax. Not broken, just undiscoverable from the picker. |
| `/connect` picker | ✅ | Lists both API-key setup (`/connect gemini`, ...) and host-CLI subscription login (`/connect claude`\|`codex`\|`opencode`). **UAT-worthy naming collision**: `/connect codex` (host CLI login inside the container) and `/auth connect codex` (BAAUTH's own subscription flow, console-only, not in this picker at all) are two unrelated commands that happen to share a provider name — this is already called out in `src/agent/auth_command.rs`'s own module doc as a deliberate-but-confusable choice. |
| `/backend` picker | ✅ | Lists the model loop plus registered runtime backends (`acpx_claude`, `codex_app_server`, `acpx_opencode`). |

## Manual UAT checklist

This is the part that needs a human — "does this feel right," "does this
screen load cleanly," "is this error message clear" is judgment this
document cannot substitute for. Each item below is a concrete step; check
them off by hand.

### Onboarding / pairing
- [ ] Fresh `/app` load with no tokens set: confirm the Connection view is
      the landing page (not a broken/empty Overview) and saving a valid
      owner token flips the connection dot to "live".
- [ ] Fresh TUI run with no saved session: confirm the plain-terminal OTC
      prompt appears before raw mode, and a valid code from
      `/connect-app terminal` (issued in an already-authorized channel)
      completes pairing and enters the alternate screen.
- [ ] A local install with `BASTION_BOOTSTRAP_TOKEN` set: confirm the TUI
      skips the OTC prompt entirely.

### Model switching
- [ ] Console: `/model <api-key-model>` hot-swaps and the next turn uses it.
- [ ] Console: after `/auth connect codex <profile>` succeeds, `/model
      codex/<model>[@profile]` hot-swaps to the subscription-backed
      provider and a turn completes.
- [ ] TUI: type `/model codex/<model>` manually (not from the picker,
      since it isn't listed) and confirm it works identically to the
      console.
- [ ] `/app` → Models: stage a default-model change, confirm it shows as
      "pending" until `/proposal approve` runs on the console, then
      confirms as applied.

### `/connect` flows
- [ ] Console/TUI: `/connect codex` (host CLI login) and `/auth connect
      codex` (subscription login) — confirm each does what it says and an
      operator reading both command names side by side isn't misled about
      which one they need.
- [ ] `/app` → Providers: stage an API key for a provider that isn't
      connected yet, confirm the pending-proposal note appears and the
      status dot flips after console approval.

### Restart recovery
- [ ] Connect a subscription profile, install an extension pack (M4.2),
      restart the daemon, confirm via `/auth status` and `/extension list`
      that both survive without re-authenticating or reinstalling.
- [ ] With the TUI open and connected, restart the daemon process and send
      a turn while it's down: confirm it surfaces as a readable
      `TurnOutcome::Error` line (`src/tui.rs:2189`), not a panic — then
      confirm a turn sent AFTER the daemon is back up succeeds without
      needing to quit and relaunch the TUI. There is no dedicated
      reconnect/backoff loop for a plain connection failure today (only
      `Unauthorized`/session-expiry has one) — this step is as much about
      confirming that gap as it is about confirming recovery.
- [ ] With `/app` open on the Live feed, restart the daemon and confirm
      the SSE connection dot goes `off`/`connecting` and comes back to
      `live` once the daemon is back, without a manual page reload.

### Loading / error states
- [ ] `/app` with an invalid/expired owner token: confirm Chat and the
      config views show a clear "token rejected" state, not a silent
      empty screen.
- [ ] `/app` with the daemon unreachable: confirm Models/Providers show
      the error line + retry button rather than hanging on "loading…"
      forever.
- [ ] TUI pointed at a daemon that isn't running: confirm the startup
      failure message (`src/tui.rs::startup_failure_message`) is legible
      and actionable, not a raw panic/stack trace.
