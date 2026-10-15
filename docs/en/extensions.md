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
  newly-added MCP server; there's no hot-reload yet. If the pack's
  `[[mcp_dependencies]]` entry declares an `auth_token` (a pack author's
  reference to an env var name, e.g. `"${SOME_API_KEY}"` — never a literal
  secret), it's written into the new `[mcp.servers.*]` entry too, so a
  server requiring an API key (e.g. a pack wrapping a paid MCP server) works
  after install without a manual `bastion.toml` edit — you still need to set
  the referenced env var yourself.
- **`native_crate` extensions** only work if this build recognizes the
  specific `crate_name` — today that's exactly one:
  `bastion/git-capability`, a workspace-confined local Git capability
  (`init/status/diff/add/commit/branch/log` only — no push/remote/fetch/
  clone). Any other `native_crate` reports a clear skip instead of silently
  doing nothing.

## Trust tier: signed packs

Every install shows a trust line, and `/extension list` shows it for each
installed extension. The tier is derived from a signature, never from what
the pack says about itself:

| What the pack ships | Tier | Install |
|---|---|---|
| no `pack.toml.sig` | `local` (unsigned) | proceeds after your review |
| `pack.toml.sig` from a publisher this build has no key for | `local` (unknown publisher, not verified) | proceeds after your review |
| `pack.toml.sig` that verifies against a key in `keys/official.pub` | `official` | proceeds after your review |
| `pack.toml.sig` that is malformed or does not verify | — | **refused, nothing installed** |

- `pack.toml.sig` is a detached Ed25519 signature over every file in the pack
  directory (the payload is defined by `bastion-extension-protocol`), so
  changing any shipped byte — a skill, a persona, a member extension — breaks
  it. Each member extension inherits the pack's tier. An inline `[signature]`
  table in `pack.toml` is refused (it would have to cover its own file).
- The signature is checked when the install is previewed and again right
  before anything is copied, so a pack edited between the two is refused.
- The official public keys are compiled in from `keys/official.pub`.
  Publishers sign with `bastion-sign` in
  [`bastion-extensions`](https://github.com/thewaifucorp/bastion-extensions)
  (see its README, "Signing").
- The tier is recorded in the lockfile and the persisted loadout, so a
  restart, an upgrade and a rollback keep the tier each version was installed
  with. Every install/upgrade/rollback/revoke logs an
  `extension_trust_decision` event.
- The tier is **informational**: it changes what you are shown, never what an
  extension may do. An `official` extension is bounded by its declared
  `PermissionSet` exactly like a `local` one.

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

- `ExtensionHost`'s own `upgrade`/`rollback` exist but aren't exposed by the
  console command yet.
- No remote catalog install (`bastion-extensions`' `catalog.toml` is shaped
  for it, but there's no `install-remote` code path) — every install today is
  from a local checkout. Signatures are verified on that local checkout.
- No official key is published in `keys/official.pub` yet, so until one is
  added every pack installs as `local`.
