# Security model

Bastion is built to make trust, authority, and data egress explicit. It is not a promise that an agent with broad credentials is risk-free: deployment choices still determine what the process can reach.

## Product safeguards

- **Identity-gated channels:** channel adapters map a sender to an explicit owner; unknown senders are rejected.
- **Trust classification:** public Discord/Slack messages and all inbound email are treated as untrusted input.
- **Signed WhatsApp ingress:** the WhatsApp path verifies the raw request HMAC before JSON parsing.
- **Capability boundary:** tool activity is routed through the runtime capability registry rather than through ad-hoc raw side effects.
- **Local sidecar isolation:** the Compose network places Python sidecars on an internal network; only the core joins the egress-capable network.
- **Secret hygiene:** channel constructors avoid logging tokens, and `.env` is ignored by Git.

## Operator responsibilities

1. Use distinct, revocable credentials for every enabled integration.
2. Keep `APP_JWT_SECRET` strong and private when the webhook/mobile surface is enabled.
3. Restrict port `8080` with a local bind, firewall, private network, or authenticated reverse proxy appropriate to your environment.
4. Map only known owners in `bastion.toml`; do not use public channel IDs as a substitute for access control.
5. Review every third-party skill or extension before installing it. Treat it as code, not as a harmless prompt.
6. Keep model-provider and telemetry choices aligned with the privacy requirements of the conversation data.

## External Control Plane API

The external `/v1/tasks*` Control Plane API is live: an outside orchestrator
authenticates with a scoped, revocable credential (`bcp_<random>`, bound to
one owner, optionally tagged with a project) and can create, list, and steer
`Pursue` tasks without adopting Bastion's internal Rust types. The same
operations are also reachable as 5 MCP tools, gated by the same scopes. See
[Control Plane security model](control-plane-security.md) for the full
threat model, credential/scope design, and the current "Known gaps" list —
this section only summarizes what an operator needs to know:

- **Rate limiting is live on the HTTP routes, not yet on the equivalent MCP
  tools.** `/v1/*` enforces a fixed 60 requests/minute per credential
  (`src/control_plane/rate_limit.rs`) — a leaked or over-issued credential
  used through an MCP client can still be called at whatever rate the
  caller sends requests; budget for that gap until it closes.
- **`project` narrows visibility within one owner's tasks on the HTTP
  routes; MCP tools don't scope by it yet.** Tag a credential with a
  `project` at issuance (`/credential issue <label> [scopes] --project
  <name>`) and it only sees tasks created by a credential with that same
  tag — but only for `/v1/*`; the equivalent MCP tools (`create_task`/
  `list_tasks`) resolve auth through a context that has no `project`
  concept, so MCP-created/listed tasks are never project-scoped yet.
  Owner is still the real, always-enforced boundary either way — `project`
  is a refinement on top of it, not a substitute.
- **Webhook subscriptions can be created but not listed or revoked** via the
  API yet — track what you register out-of-band until that surface exists.

## Incident response

If a secret may have leaked, revoke it at the provider, replace it in the deployment secret store, restart the affected service, and inspect logs without copying sensitive content into an issue. For a potential product vulnerability, follow the private reporting route in [CONTRIBUTING.md](../../CONTRIBUTING.md).

## What this does not guarantee

No configuration can make a publicly exposed, over-privileged agent safe by itself. Bastion cannot independently verify external content, provider behavior, a compromised host, or credentials you grant to an integration. Start with narrow permissions and expand only after observing the exact workflow you want.
