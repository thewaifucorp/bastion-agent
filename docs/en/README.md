# Bastion documentation

Welcome. These guides describe the code in this repository—not an older hosted product or an unrelated gateway. Start with the shortest path, then enable only the pieces you need.

## Start and operate

1. [Getting Started](getting-started.md) — build Bastion, run a first terminal turn, and understand deployment options.
2. [Configuration](configuration.md) — configure `bastion.toml`, environment overrides, identities, and channels.
3. [Channels](channels.md) — connect Telegram, webhook/mobile pairing, WhatsApp, Discord, Slack, email, or local voice.
4. [Security](security.md) — understand the trust boundary before connecting a real account.

## Learn the product

- [Architecture](architecture.md) — runtime, channels, MCP services, storage, and extension boundaries.
- [Personas](personas.md) — organize agent behavior with personas.
- [Mobile companion](mobile-app.md) — build and pair the Flutter client.
- [Crisis mode](crisis-mode.md) — operate with a deliberately narrower posture.
- [FAQ](faq.md) — common operational questions.

## Build and contribute

- [Development](development.md) — local setup and project conventions.
- [Testing](testing.md) — Rust, Python-skill, and integration checks.
- [Installer notes](how-to-install.md) — current installer scope and limitations.
- [VPS deployment](vps-setup.md) — a deployment checklist, not a copy-paste internet exposure guide.
- [ClawHub migration](../clawhub-migration.md) — review imported skills before activating them.

For the stable substrate beneath this application, see [bastion-core](https://github.com/thewaifucorp/bastion-core).
