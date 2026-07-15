//! MCP surface for the app crate.
//!
//! The MCP client (`client`), Composio OAuth (`oauth`), tool registry
//! (`registry`), `CapabilityRegistry` composition helper (`registry_setup`),
//! and `ToolSource` port impl (`tool_source`) live in `bastion_mcp` — every
//! consumer names that crate directly now (M3).
//!
//! `server` (`BastionMcpServer`) stays local — it depends on
//! `bastion_cognition::goal`/`bastion_personas::persona` (product/cognition
//! layers not part of the `bastion_mcp` extraction), so it cannot move into
//! `bastion_mcp` without either a cycle back into the app crate or a
//! port-based redesign out of scope here. See `bastion_mcp`'s crate doc for
//! the full rationale.

// M3-05: the MCP *server* surface (BastionMcpServer, MCP-over-HTTP routes,
// `bastion mcp-stdio`) is product surface, gated behind the `mcp-server`
// feature (also gates rmcp's server-side cargo features). The MCP *client*
// lives in `bastion_mcp` and is substrate, always on.
#[cfg(feature = "mcp-server")]
pub mod server;
