//! Extension pack cockpit: install/list/revoke via `ExtensionHost`. Needs
//! `&mut ExtensionHost` (and the operator's persona directory, to copy pack
//! content into), not the generic `CommandHandler` port — special-cased in
//! the daemon dispatch exactly like `/task`, `/schedule`, `/credential`.
//!
//! v1 scope: only `ExtensionKind::Declarative` members with an empty
//! `provides` list activate through `ExtensionHost` (e.g.
//! `bastion/context7-mcp`, which carries no capability of its own). Any
//! other kind (`native_crate`, `wasm`, `subprocess`) is reported as a clear,
//! actionable skip — bastion-agent doesn't wire those mechanisms into a pack
//! install yet (each is its own follow-up task). Personas are copied into
//! the operator's configured persona directory — the SAME directory
//! `PersonaRegistry::load_dir` already reads from; a pack's personas never
//! route through `ExtensionManifest` at all (mirrors how `bastion-extensions`
//! itself describes the split). Skills are copied alongside for the record,
//! but bastion-agent doesn't scan a skills directory at startup yet
//! (`SkillsLoader::load_all` exists but nothing calls it from `main()`) — a
//! pre-existing gap this command surfaces rather than silently papering over.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use bastion_extension_protocol::{ExtensionKind, ExtensionManifest, PackManifest, PermissionSet};

use crate::extension::declarative::DeclarativeExtension;
use crate::extension::{ExtensionHost, ExtensionInstance};

/// Handle `/extension <sub> [args]`.
pub async fn handle(
    host: &mut ExtensionHost,
    personas_dir: &str,
    arg: Option<&str>,
    owner: &str,
) -> anyhow::Result<String> {
    let arg = arg.unwrap_or("").trim();
    let (sub, rest) = match arg.split_once(char::is_whitespace) {
        Some((s, r)) => (s, r.trim()),
        None => (arg, ""),
    };
    match sub {
        "" | "list" => Ok(list(host)),
        "install" => Ok(install(host, personas_dir, owner, rest).await),
        "revoke" => Ok(revoke(host, rest).await),
        other => Ok(format!(
            "unknown /extension subcommand '{other}'. Use: install <path> | list | revoke <id>"
        )),
    }
}

fn list(host: &ExtensionHost) -> String {
    let loadout = host.loadout();
    if loadout.extensions.is_empty() {
        return "no extensions installed.".to_string();
    }
    let mut out = String::from("installed extensions:\n");
    for (id, version) in &loadout.extensions {
        out.push_str(&format!("  {id}  v{version}\n"));
    }
    out.trim_end().to_string()
}

async fn install(host: &mut ExtensionHost, personas_dir: &str, owner: &str, path: &str) -> String {
    if path.is_empty() {
        return "usage: /extension install <path>".to_string();
    }
    let pack_dir = Path::new(path);
    let pack_toml_path = pack_dir.join("pack.toml");
    let raw = match std::fs::read_to_string(&pack_toml_path) {
        Ok(s) => s,
        Err(e) => return format!("cannot read {}: {e}", pack_toml_path.display()),
    };
    let pack: PackManifest = match toml::from_str(&raw) {
        Ok(p) => p,
        Err(e) => return format!("invalid pack.toml at {}: {e}", pack_toml_path.display()),
    };

    let mut report = format!("installing {} v{}\n", pack.id, pack.version);

    let personas_copied = copy_pack_members(&pack_dir.join("personas"), &pack.personas, |name| {
        Path::new(personas_dir).join(name)
    });
    for (name, error) in &personas_copied.failed {
        report.push_str(&format!("  ! persona {name}: failed to copy — {error}\n"));
    }
    if !personas_copied.ok.is_empty() {
        report.push_str(&format!(
            "  personas copied: {} (reload the persona registry to activate — restart or \
             POST /lifecycle/reload)\n",
            personas_copied.ok.join(", ")
        ));
    }

    let skills_copied = copy_pack_members(&pack_dir.join("skills"), &pack.skills, |name| {
        Path::new("skills").join(name)
    });
    for (name, error) in &skills_copied.failed {
        report.push_str(&format!("  ! skill {name}: failed to copy — {error}\n"));
    }
    if !skills_copied.ok.is_empty() {
        report.push_str(&format!(
            "  skills copied: {} (note: bastion-agent doesn't scan a skills directory at \
             startup yet — a pre-existing gap, not caused by this install)\n",
            skills_copied.ok.join(", ")
        ));
    }

    let manifests = load_extension_manifests(&pack_dir.join("extensions"));
    for (ext_id, _version_req) in &pack.extensions {
        report.push_str(&install_one_extension(host, owner, &manifests, ext_id).await);
    }

    report.trim_end().to_string()
}

async fn install_one_extension(
    host: &mut ExtensionHost,
    owner: &str,
    manifests: &HashMap<String, ExtensionManifest>,
    ext_id: &str,
) -> String {
    let Some(manifest) = manifests.get(ext_id) else {
        return format!("  ! {ext_id}: referenced by pack but no matching extension.toml found\n");
    };
    if manifest.kind != ExtensionKind::Declarative {
        return format!(
            "  - {ext_id}: skipped — requires mechanism {:?}, which bastion-agent doesn't wire \
             into a pack install yet (tracked separately)\n",
            manifest.kind
        );
    }
    if !manifest.provides.is_empty() {
        return format!(
            "  - {ext_id}: skipped — declarative extension with non-empty `provides` needs \
             artifact-data loading, not implemented yet\n"
        );
    }
    let instance: Arc<dyn ExtensionInstance> =
        Arc::new(DeclarativeExtension::new(manifest.clone(), vec![]));
    match host.install(instance, owner, &PermissionSet::none()).await {
        Ok(()) => format!("  + {ext_id}: installed\n"),
        Err(e) => format!("  ! {ext_id}: install failed — {e}\n"),
    }
}

async fn revoke(host: &mut ExtensionHost, id: &str) -> String {
    if id.is_empty() {
        return "usage: /extension revoke <id>".to_string();
    }
    match host.revoke(id).await {
        Ok(()) => format!("extension {id} revoked."),
        Err(e) => format!("cannot revoke {id}: {e}"),
    }
}

struct CopyResults {
    ok: Vec<String>,
    failed: Vec<(String, String)>,
}

/// Copies each named subdirectory of `src_root` to `dest_of(name)`, best
/// effort per member — one failure never blocks the others.
fn copy_pack_members(
    src_root: &Path,
    names: &[String],
    dest_of: impl Fn(&str) -> std::path::PathBuf,
) -> CopyResults {
    let mut ok = Vec::new();
    let mut failed = Vec::new();
    if src_root.is_dir() {
        for name in names {
            let src = src_root.join(name);
            match copy_dir(&src, &dest_of(name)) {
                Ok(()) => ok.push(name.clone()),
                Err(e) => failed.push((name.clone(), e.to_string())),
            }
        }
    }
    CopyResults { ok, failed }
}

fn load_extension_manifests(dir: &Path) -> HashMap<String, ExtensionManifest> {
    let mut out = HashMap::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let toml_path = entry.path().join("extension.toml");
        let Ok(raw) = std::fs::read_to_string(&toml_path) else {
            continue;
        };
        if let Ok(manifest) = toml::from_str::<ExtensionManifest>(&raw) {
            out.insert(manifest.id.clone(), manifest);
        }
    }
    out
}

fn copy_dir(src: &Path, dest: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let dest_path = dest.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir(&entry.path(), &dest_path)?;
        } else {
            std::fs::copy(entry.path(), dest_path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_pack(
        root: &Path,
        pack_toml: &str,
        personas: &[(&str, &str)],
        extensions: &[(&str, &str)],
    ) {
        std::fs::write(root.join("pack.toml"), pack_toml).unwrap();
        for (name, content) in personas {
            let dir = root.join("personas").join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("SOUL.md"), content).unwrap();
        }
        for (name, content) in extensions {
            let dir = root.join("extensions").join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("extension.toml"), content).unwrap();
        }
    }

    #[tokio::test]
    async fn install_copies_personas_and_activates_empty_declarative_extension() {
        let pack_root = TempDir::new().unwrap();
        let personas_dest = TempDir::new().unwrap();
        write_pack(
            pack_root.path(),
            r#"
                id = "acme/test-pack"
                version = "1.0.0"
                extensions = [["acme/noop-mcp", "*"]]
                skills = []
                personas = ["tech-lead"]

                [defaults]
                enabled_extensions = []
            "#,
            &[("tech-lead", "---\nname: tech-lead\n---\nbody")],
            &[(
                "noop-mcp",
                r#"
                    id = "acme/noop-mcp"
                    version = "1.0.0"
                    kind = "declarative"
                    compat = "*"
                    provides = []
                    requires = []
                    secrets = []
                    migrations = []

                    [permissions]

                    [entrypoint]
                    kind = "declarative"
                    artifact_path = "noop.json"

                    [signature]
                    publisher = "test"
                    algorithm = "ed25519"
                    value = "dGVzdA=="
                "#,
            )],
        );

        let mut host = ExtensionHost::new();
        let report = install(
            &mut host,
            personas_dest.path().to_str().unwrap(),
            "alice",
            pack_root.path().to_str().unwrap(),
        )
        .await;

        assert!(report.contains("acme/noop-mcp: installed"), "{report}");
        assert!(report.contains("personas copied: tech-lead"), "{report}");
        assert!(personas_dest.path().join("tech-lead/SOUL.md").exists());
        assert!(host.is_installed("acme/noop-mcp"));
    }

    #[tokio::test]
    async fn install_reports_unsupported_mechanism_clearly() {
        let pack_root = TempDir::new().unwrap();
        let personas_dest = TempDir::new().unwrap();
        write_pack(
            pack_root.path(),
            r#"
                id = "acme/test-pack"
                version = "1.0.0"
                extensions = [["acme/native-thing", "*"]]
                skills = []
                personas = []

                [defaults]
                enabled_extensions = []
            "#,
            &[],
            &[(
                "native-thing",
                r#"
                    id = "acme/native-thing"
                    version = "1.0.0"
                    kind = "native_crate"
                    compat = "*"
                    provides = [{ kind = "capability", name = "acme:thing" }]
                    requires = []
                    secrets = []
                    migrations = []

                    [permissions]
                    capabilities = ["acme:thing"]

                    [entrypoint]
                    kind = "native_crate"
                    crate_name = "acme/native-thing"

                    [signature]
                    publisher = "test"
                    algorithm = "ed25519"
                    value = "dGVzdA=="
                "#,
            )],
        );

        let mut host = ExtensionHost::new();
        let report = install(
            &mut host,
            personas_dest.path().to_str().unwrap(),
            "alice",
            pack_root.path().to_str().unwrap(),
        )
        .await;

        assert!(
            report.contains("acme/native-thing: skipped — requires mechanism NativeCrate"),
            "{report}"
        );
        assert!(!host.is_installed("acme/native-thing"));
    }

    #[tokio::test]
    async fn install_reports_missing_pack_toml_clearly() {
        let empty = TempDir::new().unwrap();
        let mut host = ExtensionHost::new();
        let report = install(&mut host, ".", "alice", empty.path().to_str().unwrap()).await;
        assert!(report.starts_with("cannot read"), "{report}");
    }

    #[tokio::test]
    async fn list_and_revoke_round_trip() {
        let pack_root = TempDir::new().unwrap();
        write_pack(
            pack_root.path(),
            r#"
                id = "acme/test-pack"
                version = "1.0.0"
                extensions = [["acme/noop-mcp", "*"]]
                skills = []
                personas = []

                [defaults]
                enabled_extensions = []
            "#,
            &[],
            &[(
                "noop-mcp",
                r#"
                    id = "acme/noop-mcp"
                    version = "1.0.0"
                    kind = "declarative"
                    compat = "*"
                    provides = []
                    requires = []
                    secrets = []
                    migrations = []

                    [permissions]

                    [entrypoint]
                    kind = "declarative"
                    artifact_path = "noop.json"

                    [signature]
                    publisher = "test"
                    algorithm = "ed25519"
                    value = "dGVzdA=="
                "#,
            )],
        );

        let mut host = ExtensionHost::new();
        install(&mut host, ".", "alice", pack_root.path().to_str().unwrap()).await;
        assert_eq!(list(&host), "installed extensions:\n  acme/noop-mcp  v1.0.0");

        let out = handle(&mut host, ".", Some("revoke acme/noop-mcp"), "alice")
            .await
            .unwrap();
        assert_eq!(out, "extension acme/noop-mcp revoked.");
        assert_eq!(list(&host), "no extensions installed.");
    }
}
