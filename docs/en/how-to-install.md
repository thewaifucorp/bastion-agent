# Install Bastion

## Full self-hosted stack

Requirements: Git, Docker Engine, and Docker Compose v2.

```bash
git clone https://github.com/thewaifucorp/bastion-agent.git
cd bastion-agent
less installer.sh
./installer.sh
```

The installer is idempotent. It preserves `.env`, generates missing internal secrets,
validates Compose, rebuilds images, and starts the stack. It does not install Node,
an external skill registry, legacy plugin bootstrap, or a second configuration format.

Useful modes:

```bash
./installer.sh --prepare-only       # create/update .env; do not require Docker
./installer.sh --no-start           # configure and build without starting
./installer.sh --non-interactive    # read provider keys from exported environment
./installer.sh --dir /opt/bastion   # explicit checkout/install path
```

## Native Rust

```bash
./installer.sh --prepare-only
cargo build --locked
cargo run -- daemon
```

The checked-in `bastion.toml` uses local paths and loopback MCP URLs. Compose overrides
those values for its network and volumes. See [Configuration](configuration.md).
