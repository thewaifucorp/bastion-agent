# Extensions

Bastion's persona/skill/capability set is not fixed at compile time. A pack
bundles personas, skills, and (optionally) capabilities into one installable
unit — reviewed as a unit, never granted authority just by being installed.

The official catalog of packs lives in a separate repository:
[`bastion-extensions`](https://github.com/thewaifucorp/bastion-extensions).
This page documents what installing a pack from that catalog (or your own
local pack directory) actually does today.

## Installing a pack

This is a **console command inside the running daemon** — there is no
`bastion extension install` shell subcommand.

```
/extension install path/to/bastion-extensions/packs/software-sdlc
/extension list
/extension revoke <id>
```

Console-only, same tier as `/credential` — no remote channel can reach it.

## What v1 actually does, per pack member

A pack's `pack.toml` lists personas, skills, and extensions. Each is handled
differently:

- **Personas** are copied into your configured persona directory (the same
  one the daemon already loads from — see `BASTION_PERSONAS_DIR` in
  [Configuration](configuration.md)). They don't go through the extension
  machinery at all. Reload the persona registry to activate a newly-copied
  persona: restart the daemon, or `POST /lifecycle/reload`.
- **Skills** are copied alongside for the record. Bastion doesn't scan a
  skills directory at daemon startup yet — a pre-existing gap this command
  surfaces rather than hides, not something this feature caused.
- **Declarative extensions with no capability of their own** (an
  `mcp_dependencies`-only manifest, e.g. `context7-mcp`/`github-capability`
  in the official catalog) activate as an inert no-op registration, and any
  MCP server they declare gets merged into your `bastion.toml`'s
  `[mcp.servers.*]` — additive, idempotent, never overwrites an entry you
  already have for that server name. **Restart the daemon to activate** a
  newly-added MCP server; there's no hot-reload yet.
- **`native_crate` extensions** only work if this build recognizes the
  specific `crate_name` — today that's exactly one:
  `bastion/git-capability`, a workspace-confined local Git capability
  (`init/status/diff/add/commit/branch/log` only — no push/remote/fetch/
  clone). Any other `native_crate` reports a clear skip instead of silently
  doing nothing.

## Why GitHub access is MCP, not a bespoke capability

The official catalog's `github-capability` wraps GitHub's own hosted remote
MCP server instead of a custom REST client. A tool Bastion calls through its
own MCP client already inherits the per-persona tool-authority allowlist
(the same gate every capability goes through) and gets approval-gating on
writes for free from the MCP wire protocol's own `destructive_hint`
annotation — no bespoke capability code, no token to manage beyond the MCP
server's own OAuth. Local Git has no equivalent shortcut: `bastion-mcp`'s
client only speaks remote HTTP, and there's no remote MCP server that could
act on a workspace that only exists on your own host — that's why
`git-capability` stays a real, compiled-in capability
(`src/extension/cli_capability.rs::CliCapability`, a generic mechanism for
wrapping an already-authenticated host CLI binary — git today, reusable for
a future one without a new Rust type).

## Safety notes

- A pack's `personas`/`skills` names are untrusted input (the pack author,
  not you) — anything that isn't a plain single path segment (`..`,
  absolute paths, path separators) is rejected before it ever reaches a
  filesystem path, and a symlinked entry inside a pack is refused rather
  than followed.
- `CliCapability` rejects any argument that looks like a flag (starts with
  `-`) unless it's explicitly allowlisted for that exact subcommand — an
  allowlisted subcommand is not blanket permission to pass it any flag
  (`git-capability` allows exactly one: `-m`/`--message` on `commit`).

## Known gaps (disclosed, not silently worked around)

- No sqlite-backed loadout yet — a daemon restart loses the installed set.
- `ExtensionHost`'s own `upgrade`/`rollback` exist but aren't exposed by the
  console command yet.
- No signed-catalog install (`bastion-extensions`' `catalog.toml` is shaped
  for it, but no signing key exists and there's no `install-remote` code
  path) — every install today is from a local checkout.
