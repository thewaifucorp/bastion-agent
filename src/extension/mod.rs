//! Extension host + package manager (`docs/revamp/C3-extension-protocol-design.md`
//! §3, M4-08..12). Deliberately OUTSIDE the kernel — this is product code
//! (dependency resolution, lockfile, install/upgrade/rollback/revoke), never
//! a `bastion-runtime`/`bastion-extension-protocol` concern.
//!
//! REGRA-MÃE, restated at the ONE place it is actually enforced: installing
//! an extension never grants authority. [`facade::HostFacade`] is the single
//! chokepoint every mechanism (declarative/subprocess/wasm) must go through
//! to register a capability, reach a host, read memory, or bind a socket —
//! mirroring `CapabilityRegistry::invoke`'s "one policy boundary" precedent
//! one layer earlier, at the extension's OWN authority rather than the
//! turn's.
//!
//! Modules:
//! - [`facade`] — `ExtensionInstance` (the mechanism trait) + `HostFacade`
//!   (the enforcement boundary).
//! - [`host`] — `ExtensionHost` (install/upgrade/rollback/revoke, pack
//!   resolution) + `Loadout`.
//! - [`declarative`] — the `Declarative` mechanism (data only, §2).
//! - [`subprocess`] — the `Subprocess` mechanism (separate process,
//!   `env_clear`, versioned stdio protocol, §2).
//! - [`wasm`] — the `Wasm` mechanism (sandboxed, zero imports, fuel-bounded,
//!   §2). The wasm runtime dependency (`wasmi`) itself lives in the isolated
//!   `bastion-extension-wasm` crate (§8.7) — this module only wraps it into
//!   an `ExtensionInstance`/`Capability`.
//! - [`review`] — `permission_summary`, the owner-facing text an install flow
//!   shows before committing (M4-09).
//! - [`ui`] — extension UI isolation (Loop 3-D, CLD-08): sandboxed asset
//!   serving + the one mediated `CapabilityRegistry` bridge a served UI may
//!   use, gated by that extension's own declared `PermissionSet`.

pub mod cli_capability;
pub mod declarative;
pub mod facade;
pub mod host;
pub mod review;
pub mod subprocess;
pub mod ui;
pub mod wasm;

pub use cli_capability::CliCapability;
pub use facade::{ExtensionInstance, HostFacade};
pub use host::{ExtensionHost, Loadout};
