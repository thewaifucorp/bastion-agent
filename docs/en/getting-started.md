# Getting Started

This guide gets you to a local, inspectable Bastion process. It intentionally begins with the terminal interface: enable a channel only after you understand its credentials and owner mapping.

## What you need

- A recent stable Rust toolchain with Cargo.
- Git.
- A model provider configuration appropriate to your environment.
- Docker and Docker Compose only if you choose the Compose deployment.

The repository consumes `bastion-core` crates from a pinned Git commit, so the first build may fetch and compile more than a small CLI project.

## Run a first turn

1. Clone and enter the repository.

   ```bash
   git clone https://github.com/thewaifucorp/bastion-agent.git
   cd bastion-agent
   ```

2. Review `installer.sh`, then prepare `.env` and the required internal secrets.

   ```bash
   ./installer.sh --prepare-only
   ```

3. Review `bastion.toml`. It contains non-secret defaults such as the model name, local session path, enabled channels, and MCP definitions.

4. Build and make one request.

   ```bash
   cargo run -- agent --message "Summarize what you can safely do in this installation."
   ```

5. Start the interactive daemon when you are ready for a persistent session.

   ```bash
   cargo run -- daemon
   ```

   With a daemon already running, `cargo run -- chat` opens the official remote TUI.
   It accepts a previously paired session or `BASTION_TOKEN` for bootstrap access.

## Run the Compose stack

The included Compose file builds the core and local sidecars. It mounts `bastion.toml` read-only and stores state in named volumes.

```bash
./installer.sh
```

The core exposes port `8080` in the provided configuration. Treat that as an administrative surface: bind or firewall it for your deployment and do not publish it broadly merely to test it. The installer generates `APP_JWT_SECRET`, `BASTION_INFER_TOKEN`, and an owner-scoped bootstrap token.

## Confirm it is healthy

```bash
docker compose ps
docker compose logs -f core
```

For a source build, runtime logs follow the `logging.log_path` configured in `bastion.toml`. The default Compose configuration writes them to the Bastion data volume.

## Where next?

- [Configuration](configuration.md) for model, identity, and deployment settings.
- [Channels](channels.md) before adding a messaging token.
- [Security](security.md) before making the instance reachable from outside your machine.
- [Development](development.md) if you plan to modify the code.
