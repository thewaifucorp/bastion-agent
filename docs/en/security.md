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

## Incident response

If a secret may have leaked, revoke it at the provider, replace it in the deployment secret store, restart the affected service, and inspect logs without copying sensitive content into an issue. For a potential product vulnerability, follow the private reporting route in [CONTRIBUTING.md](../../CONTRIBUTING.md).

## What this does not guarantee

No configuration can make a publicly exposed, over-privileged agent safe by itself. Bastion cannot independently verify external content, provider behavior, a compromised host, or credentials you grant to an integration. Start with narrow permissions and expand only after observing the exact workflow you want.
