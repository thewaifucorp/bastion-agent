# Contributing to bastion-agent

`bastion-agent` is the personal AI-agent product built on
[bastion-core](https://github.com/thewaifucorp/bastion-core). Product-level
behavior (channels, config, the extension host, the mobile companion) lives
here; substrate changes (agent loop, capabilities, memory, cognition,
personas, mesh, providers) belong upstream in `bastion-core`.

## Before you start

- Check open issues and PRs first to avoid duplicate work.
- For anything beyond a small fix, open an issue describing the change
  before writing code.
- If the change is actually substrate behavior (a new `Provider`, a new
  capability kind, a change to the agent loop itself), it likely belongs in
  `bastion-core` instead — open the issue there.

## Development setup

```bash
git clone https://github.com/thewaifucorp/bastion-agent.git
cd bastion-agent
cp .env.example .env   # fill in provider keys / channel tokens
cargo build
cargo test
```

`bastion-agent` depends on `bastion-core` via git-pinned crates during
incubation (see `Cargo.toml`) — moves to crates.io versions once the
substrate publishes.

## Required checks before opening a PR

Same gates CI runs (`.github/workflows/ci.yml`):

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
bash scripts/check-scope-and-scrub.sh   # no leaked corporate names in tracked files
```

## Code standards

- Errors: typed `BastionError` (thiserror, `#[non_exhaustive]`) carried via
  `anyhow` and matched at boundaries with `downcast_ref::<BastionError>()`;
  `anyhow` only at the binary boundary (`main.rs` / handlers).
- No `unwrap`/`expect` outside test code except a proven invariant.
- `tracing` structured fields for logging, never `println!`.
- English rustdoc on public items.
- The crate is `#![forbid(unsafe_code)]` — no exceptions.
- Every tool call goes through `CapabilityRegistry::invoke` — agents never
  get raw SQL or an unmediated side effect.

## Commit messages

[Conventional Commits](https://www.conventionalcommits.org/):
`feat(scope): …`, `fix(scope): …`, `docs(scope): …`, `chore(scope): …`.

## Docs

If your change affects install/setup/usage, update the relevant guide under
`docs/en/` and `docs/pt-br/` (both language trees are kept in sync).

## Security

Do not open a public issue for a suspected vulnerability. See
[docs/en/security.md](docs/en/security.md) /
[bastion-core's SECURITY-INVARIANTS.md](https://github.com/thewaifucorp/bastion-core/blob/main/docs/SECURITY-INVARIANTS.md)
for the properties that must never regress, and contact the maintainer
directly (see [CODEOWNERS](.github/CODEOWNERS)) to report a concern
privately.
