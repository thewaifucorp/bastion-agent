//! Reconciles a pack member's declared `[[mcp_dependencies]]` into
//! `bastion.toml`'s `[mcp.servers.*]` — the SAME config
//! `bastion_mcp::McpClient::connect_from_config` already reads (once, at
//! boot — `main.rs`). This is product-level install-flow glue, deliberately
//! NOT a `bastion-mcp`/bastion-core change: the MCP client stays a generic,
//! pack-unaware connector; only bastion-agent's `/extension install` knows
//! what a "pack" or "manifest" even is.
//!
//! `bastion_extension_protocol::ExtensionManifest` doesn't model
//! `mcp_dependencies` at all (out of scope for that contracts-only crate) —
//! parsed directly from the extension's raw TOML text instead of that typed
//! struct.

use toml_edit::{value, DocumentMut};

/// One `[[mcp_dependencies]]` entry from an `extension.toml`.
#[derive(serde::Deserialize, Clone, Debug, PartialEq)]
pub struct McpDependency {
    pub name: String,
    pub endpoint: String,
    /// Informational — `McpServerEntry` (bastion-types) has no per-server
    /// read-only field to write this into; kept for now so a future
    /// reconciler revision (or an owner-facing summary) can surface it
    /// without re-parsing the manifest.
    #[serde(default)]
    #[allow(dead_code)]
    pub read_only: bool,
}

#[derive(serde::Deserialize, Default)]
struct ManifestMcpDependencies {
    #[serde(default)]
    mcp_dependencies: Vec<McpDependency>,
}

/// Extracts `[[mcp_dependencies]]` from a raw `extension.toml` string.
/// Empty if the manifest declares none, or if the raw text doesn't parse at
/// all under this narrower shape — never the reason a whole install fails.
pub fn parse_mcp_dependencies(raw_extension_toml: &str) -> Vec<McpDependency> {
    toml::from_str::<ManifestMcpDependencies>(raw_extension_toml)
        .map(|m| m.mcp_dependencies)
        .unwrap_or_default()
}

/// Merges every dependency in `deps` into `bastion_toml_path`'s
/// `[mcp.servers.*]` — additive and idempotent. An already-present server
/// name (whether from a prior reconcile or the operator's own edit) is left
/// COMPLETELY untouched, never overwritten. Returns the names actually
/// added (empty if every one was already present, or `deps` was empty).
pub async fn reconcile_mcp_dependencies(
    deps: &[McpDependency],
    bastion_toml_path: &str,
) -> anyhow::Result<Vec<String>> {
    if deps.is_empty() {
        return Ok(Vec::new());
    }

    let current = tokio::fs::read_to_string(bastion_toml_path)
        .await
        .map_err(|e| anyhow::anyhow!("failed to read '{bastion_toml_path}': {e}"))?;
    let mut doc: DocumentMut = current
        .parse()
        .map_err(|e| anyhow::anyhow!("failed to parse '{bastion_toml_path}' as TOML: {e}"))?;

    if !doc.contains_key("mcp") {
        doc["mcp"] = toml_edit::Item::Table(toml_edit::Table::new());
    }
    let mcp_table = doc["mcp"]
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("'[mcp]' in '{bastion_toml_path}' is not a table"))?;
    if !mcp_table.contains_key("servers") {
        mcp_table["servers"] = toml_edit::Item::Table(toml_edit::Table::new());
    }
    let servers = mcp_table["servers"]
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("'[mcp.servers]' in '{bastion_toml_path}' is not a table"))?;

    let mut added = Vec::new();
    for dep in deps {
        if servers.contains_key(&dep.name) {
            continue; // an existing entry (operator's own, or a prior reconcile) always wins
        }
        let mut entry = toml_edit::Table::new();
        entry["url"] = value(dep.endpoint.clone());
        entry["label"] = value(dep.name.clone());
        servers.insert(&dep.name, toml_edit::Item::Table(entry));
        added.push(dep.name.clone());
    }

    if !added.is_empty() {
        let tmp_path = format!("{bastion_toml_path}.tmp");
        tokio::fs::write(&tmp_path, doc.to_string())
            .await
            .map_err(|e| anyhow::anyhow!("failed to write tmp config '{tmp_path}': {e}"))?;
        tokio::fs::rename(&tmp_path, bastion_toml_path)
            .await
            .map_err(|e| {
                anyhow::anyhow!("failed to rename '{tmp_path}' -> '{bastion_toml_path}': {e}")
            })?;
    }

    Ok(added)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn dep(name: &str, endpoint: &str) -> McpDependency {
        McpDependency {
            name: name.to_string(),
            endpoint: endpoint.to_string(),
            read_only: true,
        }
    }

    #[test]
    fn parse_mcp_dependencies_reads_the_array() {
        let raw = r#"
            id = "bastion/context7-mcp"

            [[mcp_dependencies]]
            name = "context7"
            endpoint = "https://mcp.context7.com/mcp"
            read_only = true
        "#;
        let deps = parse_mcp_dependencies(raw);
        assert_eq!(deps, vec![dep("context7", "https://mcp.context7.com/mcp")]);
    }

    #[test]
    fn parse_mcp_dependencies_empty_when_absent() {
        assert!(parse_mcp_dependencies(r#"id = "acme/thing""#).is_empty());
    }

    #[tokio::test]
    async fn reconcile_adds_missing_server_and_is_idempotent() {
        let file = NamedTempFile::new().unwrap();
        tokio::fs::write(file.path(), "[session]\ndb_path = \".bastion/sessions.db\"\n")
            .await
            .unwrap();
        let path = file.path().to_str().unwrap();

        let added = reconcile_mcp_dependencies(
            &[dep("context7", "https://mcp.context7.com/mcp")],
            path,
        )
        .await
        .unwrap();
        assert_eq!(added, vec!["context7".to_string()]);

        let contents = tokio::fs::read_to_string(path).await.unwrap();
        assert!(contents.contains("[mcp.servers.context7]"));
        assert!(contents.contains("https://mcp.context7.com/mcp"));

        // Second run: already present, no duplicate, nothing reported added.
        let added_again = reconcile_mcp_dependencies(
            &[dep("context7", "https://mcp.context7.com/mcp")],
            path,
        )
        .await
        .unwrap();
        assert!(added_again.is_empty());
        let contents_again = tokio::fs::read_to_string(path).await.unwrap();
        assert_eq!(
            contents_again.matches("[mcp.servers.context7]").count(),
            1
        );
    }

    #[tokio::test]
    async fn reconcile_never_overwrites_an_existing_differing_entry() {
        let file = NamedTempFile::new().unwrap();
        let existing = "[mcp.servers.context7]\n\
             url = \"https://operator-override.example\"\n\
             label = \"context7\"\n";
        tokio::fs::write(file.path(), existing).await.unwrap();
        let path = file.path().to_str().unwrap();

        let added =
            reconcile_mcp_dependencies(&[dep("context7", "https://mcp.context7.com/mcp")], path)
                .await
                .unwrap();
        assert!(added.is_empty(), "existing entry must not be reported as added");

        let contents = tokio::fs::read_to_string(path).await.unwrap();
        assert!(contents.contains("https://operator-override.example"));
        assert!(!contents.contains("https://mcp.context7.com/mcp"));
    }

    #[tokio::test]
    async fn reconcile_is_a_noop_for_empty_dependencies() {
        let file = NamedTempFile::new().unwrap();
        tokio::fs::write(file.path(), "[session]\ndb_path = \"x\"\n")
            .await
            .unwrap();
        let path = file.path().to_str().unwrap();
        let before = tokio::fs::read_to_string(path).await.unwrap();

        let added = reconcile_mcp_dependencies(&[], path).await.unwrap();
        assert!(added.is_empty());
        let after = tokio::fs::read_to_string(path).await.unwrap();
        assert_eq!(before, after, "must not touch the file when deps is empty");
    }
}
