# One agent across your devices

Bastion is normally one install on one machine. With **devices** it becomes
one brain — your memory, personas and approvals — shared across the machines
you own: one is the **primary** (the source of truth), the others are
**nodes** that run only what the primary tells them, confined, and never
decide anything on their own. If the primary is lost you promote a node and
carry on.

This is the product side of the spec `multi-device-brain-and-nodes`. The
substrate lives in `bastion-mesh::devices` (Core); this page is how you use it
from `bastion-agent`.

## Roles

- **Primary** — holds the memory, personas and approvals, serves the web app,
  and is the one point of policy: a node never approves, never writes memory
  and never calls a model on its own.
- **Node** — dials the primary (it never opens a port), and runs the confined
  primitives the owner granted it: `system.run`, `file.read`, `file.write`,
  and on Windows the UI Automation primitives `ui.snapshot` / `ui.act`.

## Set up the primary

On the machine that will hold the brain:

```
bastion node init
```

This creates the owner key and a device registry with just this device, at
epoch 1. Then turn devices on in `bastion.toml` and restart the daemon:

```toml
[devices]
enabled = true
# Where other devices reach this one (published so clients find the primary):
address = "https://linux-box.tailnet.ts.net:8443"
# A self-signed primary: the PEM of extra CAs a node should trust.
# ca_file = "/etc/bastion/primary-ca.pem"
```

The daemon now serves `GET /node` (the WebSocket a node dials) and the
`/devices/*` API. Reach it over your private network — Tailscale is the
documented path; any private route with TLS works. The transport is TLS by
default; `allow_plain_transport = true` permits `ws://` only for a route that
is already encrypted end to end (a tailnet) or for loopback tests.

## Add a node

On the primary, open **Devices** in the web app (or `POST
/devices/pairing-codes` with the daemon token) to get a one-time code. On the
new machine:

```
bastion node pair --primary https://linux-box.tailnet.ts.net:8443 --code BAST-XXXX-XXXX
bastion node run
```

`pair` asks to join and waits; you approve it on the primary (a device is
admitted only with your signature **and** approval from a device already
registered). `run` then serves the primary until stopped. A node starts with
**no** grants — you grant capabilities per device, and for UI Automation per
application.

## Grants

Capabilities a node may run are granted per device (and, for `ui.act`, per
app), from the Devices view or `PUT /devices/{id}/grants`. A grant can require
your approval each time; the primary only ever hardens that, never relaxes it.
A revoked-then-changed grant takes effect at once; a brand-new grant is picked
up the next time the daemon starts (the tool list is part of the model's
cached prompt, kept stable on purpose).

## Replica, promotion and reconciliation

A node you mark as holding a replica keeps an encrypted copy of the memory
event log, updated as you use the primary. If the primary is lost:

```
bastion node promote
```

turns that node's replica into a live primary at the next epoch. When the old
primary comes back it rejoins as a node; what it wrote during the split is
merged in — beliefs are unioned, and a belief both sides changed becomes a
**conflict** you resolve in the Devices view (neither version is dropped until
you decide).

An epoch fences the old primary: the moment it learns a newer one exists it
stops accepting writes, so two primaries never both write in one epoch.

## Secrets

By default **no** credential (API key, token) is copied to a node. Copying
secrets to a device, dormant until promotion, is a further step (spec BMD-18,
BMD-29..33) not enabled in this release: a promoted device starts without the
owner's credentials, and you set them there yourself.

## The desktop shell (Windows)

`desktop/shell` is a tray app that embeds the primary's web app in a WebView2
— it has no interface of its own. Its tray **Stop node** cuts the local node
at once. Install it per user (no admin) with `desktop/shell/install.ps1`. See
that folder's README.

## Security summary

- A node dials out; it never listens (BMD-09).
- A node runs only granted capabilities, and refuses an order from an older
  epoch (BMD-10, BMD-12).
- Every remote call passes the primary's persona authority, egress and
  approval before it is sent (BMD-11).
- The replica is encrypted at rest under a key in the system vault (Windows
  Credential Manager, macOS Keychain; an owner-only file on Linux).
- Revoking a device stops it connecting and lists the secrets to rotate.
