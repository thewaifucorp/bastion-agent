//! Multi-device support (spec `multi-device-brain-and-nodes.md`): this
//! device's identity, role and registry ([`state`]), the keys that protect
//! what it holds for the owner ([`vault`]), the primitives it runs as a node
//! ([`catalog`]), the Windows UI Automation primitives ([`ui`]), the primary
//! side in the daemon ([`primary`], [`routes`], [`pending`]), and the node
//! commands ([`node_cmd`]).

pub mod catalog;
pub mod node_cmd;
pub mod pending;
pub mod primary;
pub mod routes;
pub mod secret_sync;
pub mod state;
pub mod ui;
pub mod vault;
