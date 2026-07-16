# Testing

Bastion uses Rust tests for the product runtime and Python tests for the local skills. The `tests/` directory includes conformance, adversarial, integration, live-contract, and end-to-end coverage; not every test is appropriate to run against production credentials.

## Fast local checks

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

The GitHub Actions workflow runs these commands on pushes to `main` and on pull requests. It also runs `bash scripts/check-scope-and-scrub.sh` to prevent public repository leaks.

## Skill tests

```bash
python3 -m pytest skills/ -q
python3 -m pytest skills/weight-system/tests/ -v --rootdir=.
```

Use the second form to focus on one skill. Install the relevant skill’s Python dependencies first; this repository intentionally does not pretend that every optional skill has a single shared dependency environment.

## Focused Rust tests

Cargo supports test-name filtering. For example:

```bash
cargo test config
cargo test --test extension_adversarial
```

Read the targeted test before running any file described as live or end-to-end. Those tests can expect external services, credentials, local binaries, or a Docker environment.

## Adding coverage

- Put product tests in `tests/` when they exercise a cross-module behavior; keep focused unit tests near the code they document.
- Name the safety property being protected, especially for trust, identity, egress, or extension tests.
- Prefer fixtures and stubs over real credentials.
- When a behavior crosses the Rust/Python boundary, add coverage on the boundary rather than only on one side.
