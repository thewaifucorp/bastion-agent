# Presence and Bastion mode

Presence is one contract for every surface that shows Bastion or acts on it:
the TUI, the web app, the mobile app, a desk device, a chat channel. It has
three parts:

- **State out**: a compact snapshot (`GET /presence/snapshot`) and change
  notifications on `/events`.
- **Actions in**: one route (`POST /presence/action`) to approve, deny, switch
  persona, care for the companion, or leave a note on a device.
- **Surfaces**: a registry of paired devices, each with its own scoped token.

**Bastion mode** builds on it: a plugin installed into another coding agent
(Claude Code first) turns that agent into a Bastion. The identity, personas,
memory model, companion and rules are Bastion's; the host agent runs the model
and the tools. See [Bastion mode for Claude Code](#bastion-mode-for-claude-code).

All routes are served by the daemon next to `/companion` and are authenticated
like it (`x-bastion-token`, owner token or app JWT). Device-facing routes also
accept a device token (`x-bastion-device-token`), limited to that device's
scopes.

## Configuration

```toml
[presence]
enabled = true            # mount the /presence routes (default true)
approval_wait_secs = 45   # how long a remote approval waits before falling back to the host
journal = true            # record a local-only journal entry when a host turn ends
context_max_chars = 6000  # cap for the context injected into a host agent
advertise_url = ""        # URL devices use to reach this daemon (e.g. http://192.168.0.10:8080)
device_approvals = false  # paired devices with a touch input may approve/deny (see Devices)

# Rules for the host agent's tool calls. First match wins; no match = the host decides.
[[presence.rules]]
tool = "Bash"             # glob on the tool name ("*", "mcp__*", "Write")
input = "git push*"       # optional glob on the call's main input (command, file path, url)
decision = "remote"       # allow | deny | ask | remote
```

Decisions:

| decision | effect in the host |
|---|---|
| `allow` | runs without the host's own prompt |
| `deny` | blocked, with the rule as the reason |
| `ask` | the host asks the person, as it normally would |
| `remote` | Bastion opens an approval on every surface (app, web, device, CLI) and waits up to `approval_wait_secs`; approved = `allow`, denied = `deny`, no answer = `ask` |

## Snapshot

`GET /presence/snapshot` — optional `max_bytes` and `max_sessions` query
parameters cut the snapshot to a device's capacity (dropped sessions are
counted in `more`; alerts are kept first).

```json
{
  "v": 1,
  "seq": 42,
  "now": 1760745600,
  "host": "workstation",
  "persona": "ada",
  "pet": { "level": 3, "xp": 120, "needs": { "water": 40, "food": 70, "play": 55, "rest": 80 }, "cue": "water" },
  "sessions": [
    { "id": "a1b2c3d4", "src": "claude-code", "name": "bastion-agent", "st": "working",
      "tool": "Bash", "det": "cargo test", "since": 1760745500, "model": "claude-opus-5-5" }
  ],
  "more": 0,
  "alerts": [
    { "id": "ap-7", "kind": "approval", "sid": "a1b2c3d4", "tool": "Bash", "det": "git push origin main", "exp": 1760745645 }
  ],
  "note": { "text": "back in 10 min", "exp": 1760747400 },
  "today": { "turns": 12, "tools": 140, "errors": 1 }
}
```

- `st`: `working` | `waiting` (needs the person) | `idle` | `done` | `error`.
- `alerts[].input` (approvals, owner snapshots only): the call's whole main
  input, control characters flattened, at most 2000 characters. `det` is cut
  for display; a surface that lets the person approve shows `input`, and the
  desktop tray offers no one-click Approve when `det` is not the whole call.
  Device snapshots never carry it.
- `alerts[].kind`: `approval` (resolvable from any surface with
  `POST /presence/action`), `question` (the host is asking at the computer),
  `error` (a turn ended with an error).
- Empty or unknown values are omitted. `now`, `since` and `exp` are Unix
  seconds. `seq` grows on every change.
- Fields never present: prompts, replies, file contents, diffs, tool output.
  `det` is at most 48 characters of the call's main input.

## Events on `/events`

Each is one JSON string on the existing SSE feed:

- `{"event":"presence.updated","seq":42}` — anything changed; refetch the snapshot.
- `{"event":"approval.requested","id":"ap-1f3a9c2e"}`
- `{"event":"approval.resolved","id":"ap-1f3a9c2e","decision":"approve|deny|timeout","by":"owner|device:<id>"}`
- `{"event":"presence.persona"}`

`/events` is one feed for every owner of the daemon, so these carry no
content: a surface reads the details from its own snapshot.

Everything in presence is scoped to an owner. Sessions, alerts, approvals,
the active persona, the note and the day's counters belong to the owner whose
token reported them; a snapshot shows only the caller's, an approval can only
be answered by its owner (or that owner's devices), and approval ids are
random (`ap-` + 8 hex digits).

## Actions

`POST /presence/action`, answered with the updated snapshot:

```json
{ "action": "approve", "id": "ap-7" }
{ "action": "deny", "id": "ap-7" }
{ "action": "persona", "persona": "ada" }
{ "action": "persona", "persona": null }
{ "action": "care", "care": "water" }
{ "action": "note", "text": "back in 10 min", "minutes": 30 }
```

A device token may send `approve`/`deny` (only with `device_approvals = true`
and a device that declared `touch`) and `care`. Everything else needs the
owner token.

## Host agent routes (used by the plugin)

| route | body | answer |
|---|---|---|
| `POST /presence/event` | `{source, session_id, event, cwd?, tool_name?, tool_input?, model?, notification_type?, agent_id?}` | `{ok, seq}` |
| `POST /presence/context` | `{source, session_id, cwd?, kind: "session"\|"prompt", prompt?}` | `{context, persona}` |
| `POST /presence/approval` | `{source, session_id, cwd?, tool_name, tool_input?}` | `{decision: "allow"\|"deny"\|"ask"\|"none", reason?}` |
| `POST /presence/journal` | `{source, session_id, cwd?, prompt?, reply?}` | `{stored, id?}` |

`event` uses the hook vocabulary: `SessionStart`, `UserPromptSubmit`,
`PreToolUse`, `PermissionRequest`, `PostToolUse`, `PostToolUseFailure`,
`Notification`, `SubagentStart`, `SubagentStop`, `PreCompact`, `Stop`,
`StopFailure`, `SessionEnd`. `tool_input` keeps only `command`, `file_path`,
`pattern`, `url`, `query` and `description`, each cut to 200 characters.

The context is built from the active persona, the core beliefs, the beliefs
relevant to the prompt and the goals, and every piece passes the same egress
check as the MCP server (`check_egress(tier, "external")`): a `local_only`
belief or persona never reaches a cloud host agent. The journal entry is
stored `local_only`, so it stays on this machine and is never injected back
into a cloud host.

## Devices

A device is any screen that implements the device API below. Bastion pushes
the snapshot to it; the device sends actions back.

Pairing (owner):

```bash
bastion presence pair 192.168.0.42 4827   # the code shown on the device
bastion presence devices
bastion presence unpair bastion-4f2a
```

`POST /presence/devices/pair` `{addr, code, name?}` → Bastion calls the
device's `POST /api/pair` with
`{code, host, bastion_url, bastion_token}`. The device answers
`{id, token, caps}` and keeps `bastion_url` + `bastion_token` to send actions.
Bastion stores the device's push token and only a SHA-256 hash of the token it
issued. `GET /presence/devices` lists them; `DELETE /presence/devices/{id}`
removes one.

### Device API (what a device implements)

| route | auth | body / answer |
|---|---|---|
| `GET /api/info` | none | `{id, fw, board, paired, caps}` |
| `POST /api/pair` | the on-screen code | `{code, host, bastion_url, bastion_token}` → `{id, token, caps}` |
| `POST /api/state` | `Authorization: Bearer <token>` | the snapshot → `204` |

`caps`: `{ "w": 240, "h": 240, "touch": true, "max_bytes": 4096, "max_sessions": 6 }`.

Bastion pushes on every change (debounced) and every 10 seconds as a
heartbeat; a device that hears nothing for 30 seconds shows "disconnected".
The code rotates after every pairing attempt, right or wrong; five wrong codes
lock pairing for a minute. A paired device never reopens its open Wi-Fi setup
portal by itself — re-setup takes the physical factory reset.

**Device traffic is plain HTTP on the local network.** Anyone who can read
that traffic can copy a device's token and replay it. That is why
`device_approvals` is off by default: with it off, a device token can only
read its owner's snapshot and care for the companion. Turn it on only on a
network you trust.

The reference firmware for GeekMagic SmallTV boards lives in
[`firmware/smalltv`](../../firmware/smalltv/README.md).

## In the TUI

The `bastion` terminal UI follows presence on its own: the companion line
shows what host agents are doing ("2 agents · 1 working · 1 waiting"), and
while an approval waits a highlighted bar shows the whole call and the
companion raises its hand. `Ctrl+Y` approves and `Ctrl+N` denies the first
pending approval — `Ctrl+Y` only when the call is shown whole
(`input_complete`); otherwise review it in the web app. `/agents` prints the
full status, `/approve <id>` and `/deny <id>` answer a specific one, and the
transcript notes each approval as it arrives and when it is answered.

## Bastion mode for Claude Code

The plugin lives in [`integrations/claude-code`](../../integrations/claude-code/README.md).

```bash
claude plugin marketplace add ./integrations/claude-code
claude plugin install bastion@bastion
```

What it does, per hook (`bastion hook claude-code` reads the hook payload):

| hook | Bastion |
|---|---|
| `SessionStart` | registers the session; injects persona, core memory, goals and the persona roster |
| `UserPromptSubmit` | injects the memories relevant to the prompt |
| `PreToolUse` | applies `[[presence.rules]]`; a `remote` rule waits for an approval from any surface |
| `PostToolUse`, `Notification`, `Subagent*`, `PreCompact` | updates presence and the companion |
| `Stop` | marks the turn done; writes the local journal entry |
| `SessionEnd` | closes the session |

The plugin also connects the `bastion` MCP server (`bastion mcp-stdio`), so the
host can save memories with `memory_store` and read personas, memories and
goals. Slash commands: `/bastion:status`, `/bastion:persona <name>`,
`/bastion:approve <id>`, `/bastion:deny <id>`.

Enforcement reaches as far as the host's hooks do. In Claude Code a
`PreToolUse` decision is binding. When the daemon is not running, every hook
exits silently and Claude Code behaves exactly as without the plugin.
