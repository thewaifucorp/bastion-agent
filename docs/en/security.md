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

- **No rate limiting yet, on either the HTTP routes or the equivalent MCP
  tools.** A leaked or over-issued credential can be used at whatever rate
  the caller sends requests. Owner isolation (below) still bounds the blast
  radius, but budget accordingly until this closes.
- **`project` is accepted at credential issuance but not yet enforced as a
  filter.** A credential's `project` tag is stored and returned, but every
  read today is scoped by owner only — tagging two credentials with
  different `project` values does not currently separate what they can see.
  Don't rely on it for isolation yet; owner is the real boundary.
- **Webhook subscriptions can be created but not listed or revoked** via the
  API yet — track what you register out-of-band until that surface exists.

## Incident response

If a secret may have leaked, revoke it at the provider, replace it in the deployment secret store, restart the affected service, and inspect logs without copying sensitive content into an issue. For a potential product vulnerability, follow the private reporting route in [CONTRIBUTING.md](../../CONTRIBUTING.md).

## What this does not guarantee

No configuration can make a publicly exposed, over-privileged agent safe by itself. Bastion cannot independently verify external content, provider behavior, a compromised host, or credentials you grant to an integration. Start with narrow permissions and expand only after observing the exact workflow you want.
