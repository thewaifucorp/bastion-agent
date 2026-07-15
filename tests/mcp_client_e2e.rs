//! E2E-oriented smoke tests for Bastion's MCP client boundary.
//!
//! Live external MCP servers are environment-specific, so the automated path
//! verifies the runtime client behavior that is stable in CI: config-driven
//! connection setup degrades gracefully, and unknown tools fail before dispatch.

use bastion::config::McpServerEntry;
use bastion_mcp::McpClient;
use serde_json::json;
use std::collections::HashMap;

#[tokio::test]
async fn test_mcp_client_list_tools() -> anyhow::Result<()> {
    let servers = HashMap::new();
    let client = McpClient::connect_from_config(&servers).await?;

    assert!(
        client.registry().list_tool_names().is_empty(),
        "empty config should produce an empty MCP tool registry"
    );
    Ok(())
}

#[tokio::test]
async fn mcp_client_unavailable_external_server_is_non_fatal() -> anyhow::Result<()> {
    let mut servers = HashMap::new();
    servers.insert(
        "echo".to_string(),
        McpServerEntry {
            url: "http://127.0.0.1:9/mcp".to_string(),
            label: "echo".to_string(),
            is_local: false,
            trusted: false,
        },
    );

    let client = McpClient::connect_from_config(&servers).await?;
    assert!(
        client.registry().list_tool_names().is_empty(),
        "unavailable external MCP server should be skipped, not fatal"
    );
    Ok(())
}

#[tokio::test]
async fn mcp_client_unknown_tool_fails_before_dispatch() -> anyhow::Result<()> {
    let client = McpClient::connect_from_config(&HashMap::new()).await?;
    let err = client
        .call_tool_with_timeout("echo", json!({"message": "hello"}), "alice")
        .await
        .expect_err("unknown tool must fail");

    assert!(
        err.to_string().contains("tool 'echo' not found"),
        "unexpected error: {err}"
    );
    Ok(())
}
