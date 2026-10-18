# Bastion mode for Claude Code

This plugin turns Claude Code into your Bastion. Claude keeps running the model
and the tools; who it is, what it remembers, which goals it serves and which
tool calls need a yes come from your Bastion:

- **Persona**: Claude acts as your active Bastion persona, and knows the
  others. `/bastion:persona <name>` switches, effective from the next prompt.
- **Memory**: each session starts with Bastion's core memories and goals;
  each prompt brings the memories relevant to it. With the MCP server
  connected, Claude saves what it learns with `memory_store`.
- **Rules and approvals**: `[[presence.rules]]` in your `bastion.toml` decide
  tool calls. A `remote` rule asks for a yes on every Bastion surface — the
  web app, the mobile app, `bastion presence approve`, a desk device — and
  falls back to Claude Code's own prompt when nobody answers in time.
- **Presence and companion**: your sessions show up live on every surface,
  and the companion feels the work (and reminds you to drink water).
- **Journal**: when a turn ends, a short local-only note of what was asked
  and answered is kept in Bastion's memory. It never goes back to the cloud.

Nothing leaves your machine except through Claude Code itself, and the context
Claude receives passes Bastion's egress rule: `local_only` memories and
personas are never sent. When the Bastion daemon is not running, every hook
exits silently and Claude Code behaves exactly as without the plugin.

## Requirements

- Claude Code 2.1 or newer.
- The `bastion` binary on `PATH` (or `BASTION_BIN=/path/to/bastion`).
- A Bastion daemon reachable at `BASTION_URL` (default
  `http://127.0.0.1:8080`) with `[presence]` enabled (the default).
- A token: `BASTION_TOKEN=<owner token>`, or sign in once with `bastion chat`
  (the hooks reuse the saved session).

## Install

```bash
claude plugin marketplace add /path/to/bastion-agent/packs/claude-code
claude plugin install bastion@bastion
```

To try it without installing:

```bash
claude --plugin-dir /path/to/bastion-agent/packs/claude-code
```

### Memory tools (optional)

The plugin declares the daemon's HTTP MCP server as `bastion`. Enable it in
`bastion.toml` and give the plugin its token:

```toml
[mcp_server]
enabled = true

# The table key is the token itself.
[mcp_server.tokens."a-long-random-token"]
owner_id = "_local"
read_only = false
```

`memory_store` is a local capability, so the token needs no `cloud_ok`.

```bash
export BASTION_MCP_TOKEN=a-long-random-token
```

Without it everything else works; Claude just cannot write memories itself.

### Rules

```toml
[presence]
approval_wait_secs = 45

[[presence.rules]]
tool = "Bash"
input = "git push*"
decision = "remote"   # ask on every Bastion surface

[[presence.rules]]
tool = "Bash"
input = "*rm -rf /*"
decision = "deny"

[[presence.rules]]
tool = "Read"
decision = "allow"
```

The `PreToolUse` hook waits up to 120 seconds; keep `approval_wait_secs`
below that. While a `remote` approval is pending, answer it from another
surface (the Claude Code window is waiting on it).

## Commands

| command | does |
|---|---|
| `/bastion:status` | sessions, pending approvals, active persona |
| `/bastion:persona <name>` | switch persona (no name: default) |
| `/bastion:approve <id>` / `/bastion:deny <id>` | answer an approval |

## Troubleshooting

`BASTION_HOOK_DEBUG=1 claude` prints why a hook did nothing (daemon not
reachable, token refused, …) on the hook's stderr.

The full contract is in [`docs/en/presence.md`](../../docs/en/presence.md).
