# Installation notes

The supported, auditable installation path is a source checkout: build it with Cargo or build the included Compose stack. The checked-in `installer.sh` is an operational script under active maintenance; inspect it before running it because it can perform host-level setup, Docker checks, cloning, and optional skill installation.

## Recommended path

```bash
git clone https://github.com/thewaifucorp/bastion-agent.git
cd bastion-agent
cargo build
cargo run -- daemon
```

For a multi-service local deployment:

```bash
docker compose up --build
```

Before either path, review `bastion.toml`, create a private `.env` for secrets, and read [Configuration](configuration.md). Do not use a curl-pipe-to-shell command unless its endpoint, repository URL, and version have been independently verified.
