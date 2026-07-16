# Development

## Local setup

```bash
git clone https://github.com/thewaifucorp/bastion-agent.git
cd bastion-agent
cargo build
cargo test
```

`bastion-agent` depends on `bastion-core` crates pinned to an exact public commit. A working Git installation and network access are therefore needed for a clean build.

For the Python skill suites, install their declared dependencies in an isolated environment before running pytest. Each skill that needs Python dependencies declares its own requirements or project metadata.

## Useful commands

| Command | Purpose |
| --- | --- |
| `cargo run -- daemon` | Start the local interactive daemon. |
| `cargo run -- chat` | Open the official remote terminal UI. |
| `cargo run -- agent --message "…"` | Run one terminal turn and exit. |
| `cargo build --all-features` | Compile optional product features. |
| `cargo fmt --check` | Verify Rust formatting. |
| `cargo clippy --all-targets --all-features -- -D warnings` | Run the same strict Clippy gate used by CI. |
| `cargo test` | Run Rust tests. |
| `(cd skills/<name> && python3 -m pytest -q)` | Run one Python skill suite without cross-suite module collisions. |
| `bash scripts/check-scope-and-scrub.sh` | Run the repository’s public-scope scrub check. |

## Code conventions

- Unsafe Rust is forbidden by the crate lints.
- Use structured `tracing` events rather than ad-hoc standard output in product code.
- Avoid `unwrap` and `expect` outside tests unless an invariant is proven locally.
- Keep public Rust documentation in English.
- Send tool use through the capability registry; do not add a raw side-effect path around it.

## Contribution scope

This repository owns product-level behavior: channel adapters, configuration, the extension host, and the mobile companion. Agent-loop mechanics, provider substrate, memory primitives, personas, cognition, and mesh behavior generally belong in `bastion-core`.

Before a pull request, run the CI-equivalent checks above and update both language trees when a user-facing behavior changes. See [CONTRIBUTING.md](../../CONTRIBUTING.md) for the project’s PR and security-reporting guidance.
