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
It extracts the release binary from the image and installs a launcher at
`~/.local/bin/bastion`; add that directory to `PATH` if your shell does not
already include it. After installation, the normal command is simply:

```bash
bastion
```

Useful modes:

```bash
./installer.sh --prepare-only       # create/update .env; do not require Docker
./installer.sh --no-start           # configure and build without starting
./installer.sh --non-interactive    # read provider keys from exported environment
./installer.sh --dir /opt/bastion   # explicit checkout/install path
```

## Extension packs that need a host CLI (e.g. git)

`bastion/git-capability` (from `bastion-extensions`' `software-sdlc` pack)
wraps the `git` binary — the default `runtime` image doesn't include it, to
keep every deployment that doesn't use that pack lean. Build the
`runtime-devtools` stage instead when you plan to install a pack with a
CLI-backed capability:

```bash
docker build --target runtime-devtools -t bastion:devtools .
```

CI and the published release images both build the plain `runtime` stage —
`runtime-devtools` is opt-in only, never the default.

## Updating a running installation

Check the official GitHub Release from the host:

```bash
bastion update
```

Apply the newest release explicitly:

```bash
bastion update --apply --yes
```

The installer fetches the release tag, refuses a checkout with tracked local
changes, rebuilds and restarts Compose, health-checks `core`, and restores the
previous revision if the new release does not become healthy.

Every installed Compose deployment also has a narrowly-scoped host updater.
From a trusted, mapped channel or the TUI, `/update` reports release status and
`/update apply` requests the same host-side flow. The container never receives
the Docker socket or write access to the source checkout; this command is an
explicit owner action, not an automatic update.

## Native Rust

```bash
./installer.sh --prepare-only
cargo build --locked
cargo run
```

The checked-in `bastion.toml` uses local paths and loopback MCP URLs. Compose overrides
those values for its network and volumes. See [Configuration](configuration.md).
