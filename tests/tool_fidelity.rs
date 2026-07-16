use bastion_mcp::registry::ToolRegistry;

#[test]
fn registry_register_and_lookup() {
    let mut reg = ToolRegistry::new();
    for name in ["github_star", "calendar_event"] {
        reg.register_with_schema(
            "composio",
            name.into(),
            serde_json::json!({"type": "object", "properties": {}}),
            String::new(),
            false,
            false,
            false,
        );
    }
    let names = reg.list_tool_names();
    assert!(names.contains(&"github_star"));
    assert!(names.contains(&"calendar_event"));
    assert_eq!(reg.server_for("github_star"), Some("composio"));
    assert_eq!(reg.server_for("unknown_tool"), None);
}

#[test]
fn registry_multi_server() {
    let mut reg = ToolRegistry::new();
    reg.register_with_schema(
        "composio",
        "tool_a".into(),
        serde_json::json!({"type": "object", "properties": {}}),
        String::new(),
        false,
        false,
        false,
    );
    reg.register_with_schema(
        "local",
        "tool_b".into(),
        serde_json::json!({"type": "object", "properties": {}}),
        String::new(),
        false,
        false,
        false,
    );
    assert_eq!(reg.server_for("tool_a"), Some("composio"));
    assert_eq!(reg.server_for("tool_b"), Some("local"));
}

#[test]
fn registry_schema_stored_and_retrieved() {
    let mut reg = ToolRegistry::new();
    let schema = serde_json::json!({"type": "object", "properties": {"repo": {"type": "string"}}});
    reg.register_with_schema(
        "composio",
        "github_star".into(),
        schema.clone(),
        "Star a GitHub repo".into(),
        false,
        false,
        false,
    );
    let retrieved = reg.get_tool_schema("github_star").unwrap();
    assert_eq!(retrieved, &schema);
    assert!(reg.get_tool_schema("nonexistent").is_none());
    assert_eq!(
        reg.get_tool_description("github_star"),
        Some("Star a GitHub repo")
    );
    assert!(reg.get_tool_description("nonexistent").is_none());
}

#[tokio::test]
async fn connect_from_empty_config() {
    let client = bastion_mcp::McpClient::connect_from_config(&std::collections::HashMap::new())
        .await
        .unwrap();
    assert!(client.registry().list_tool_names().is_empty());
}
