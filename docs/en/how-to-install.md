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

## Native install (desktop, no Docker)

Runs Bastion directly on your machine (Linux or macOS). Subscription logins
work the normal way — the browser opens on the same machine, and `claude`,
`codex` and `opencode` use the login you already have — because nothing sits
between Bastion and your host.

Requirements: Git, a Rust toolchain (`cargo`), and [uv](https://docs.astral.sh/uv/)
for the Python sidecars. On Linux, a kernel with Landlock (5.13+, 6.7+ for the
full network rules) or working unprivileged user namespaces for bubblewrap; on
macOS, the built-in `sandbox-exec`.

```bash
./installer.sh --native              # add --with-voice for local speech (~1 GB of models)
```

What it does:

- builds `bastion` and installs the launcher in `~/.local/bin`;
- keeps state in `<install dir>/data` (`BASTION_DATA_DIR`) and writes
  `bastion.native.toml` (merged over the tracked `bastion.toml`) with
  `[sandbox] mode = "required"` and the sidecars to run;
- installs each sidecar (memupalace, skill-writer, self-improving, optionally
  voice) in its own virtualenv and downloads its models — at runtime the
  sidecars have **no network at all**: they talk to Bastion and to each other
  over Unix sockets in a private run directory (`$XDG_RUNTIME_DIR/bastion`,
  else `$TMPDIR/bastion-<uid>`, 0700; `BASTION_RUN_DIR` overrides), never over TCP;
- registers a user service: `systemctl --user status bastion` on Linux, the
  `ai.thewaifucorp.bastion` launch agent on macOS (logs in `data/logs/`).

Everything Bastion runs — agent harnesses, the git pack, extensions,
sidecars — is confined by the OS sandbox (see [Configuration](configuration.md#sandbox)).
The daemon refuses to start if this host has no sandbox backend.

Log in to a subscription with `bastion connect claude|codex|opencode` (it runs
the CLI's own login on your machine) or, for the ChatGPT subscription inside
Bastion's own loop, `/auth connect codex` with `[subscriptions.codex] login = "browser"`.
With Claude Code logged in, `/backend use acp_claude` runs the conversation on
it and every edit it wants to make waits for your `sim` (see
[Claude Code subscription](configuration.md#claude-code-subscription)).

`bastion update --apply --yes` updates a native install in place (rebuild,
sidecars, service restart) and rolls back if the new release fails its health
check.

For development without the service: `./installer.sh --native --no-start`, then
`bastion daemon` from the install dir.
