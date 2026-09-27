//! Multi-device support (spec `multi-device-brain-and-nodes.md`): this
//! device's identity, role and registry ([`state`]), the keys that protect
//! what it holds for the owner ([`vault`]), the primitives it runs as a node
//! ([`catalog`]), and the Windows UI Automation primitives ([`ui`]).

pub mod catalog;
pub mod state;
pub mod ui;
pub mod vault;
