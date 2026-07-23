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

use bastion_extension_protocol::{
    Entrypoint, ExtensionError, ExtensionKind, ExtensionManifest, PackManifest, PermissionSet,
};

use crate::extension::declarative::DeclarativeExtension;
use crate::extension::{CliCapability, ExtensionHost, ExtensionInstance, HostFacade};

/// Handle `/extension <sub> [args]`.
pub async fn handle(
    host: &mut ExtensionHost,
    personas_dir: &str,
    bastion_toml_path: &str,
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
        "install" => Ok(install(host, personas_dir, bastion_toml_path, owner, rest).await),
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

async fn install(
    host: &mut ExtensionHost,
    personas_dir: &str,
    bastion_toml_path: &str,
    owner: &str,
    path: &str,
) -> String {
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
        report.push_str(
            &install_one_extension(host, owner, &manifests, bastion_toml_path, ext_id).await,
        );
    }

    report.trim_end().to_string()
}

/// `bastion/git-capability`'s `crate_name` — the one `native_crate` mapping
/// this install flow recognizes today. A second native_crate consumer would
/// warrant a real registry (`crate_name` -> constructor); with exactly one,
/// a hardcoded match is honest about the actual state of the mechanism
/// (`docs/en/...` should say so too — see the pack's own README note).
const GIT_CAPABILITY_CRATE_NAME: &str = "bastion/git-capability";

async fn install_one_extension(
    host: &mut ExtensionHost,
    owner: &str,
    manifests: &HashMap<String, (ExtensionManifest, String)>,
    bastion_toml_path: &str,
    ext_id: &str,
) -> String {
    let Some((manifest, raw)) = manifests.get(ext_id) else {
        return format!("  ! {ext_id}: referenced by pack but no matching extension.toml found\n");
    };

    let mut report = String::new();
    report.push_str(&reconcile_one_extension_mcp_deps(raw, bastion_toml_path, ext_id).await);

    if let Entrypoint::NativeCrate { crate_name } = &manifest.entrypoint {
        if crate_name == GIT_CAPABILITY_CRATE_NAME {
            report.push_str(&install_git_capability(host, owner, manifest).await);
            return report;
        }
        report.push_str(&format!(
            "  - {ext_id}: skipped — native_crate '{crate_name}' has no known mapping in this \
             build (only {GIT_CAPABILITY_CRATE_NAME} is wired today)\n"
        ));
        return report;
    }
    if manifest.kind != ExtensionKind::Declarative {
        report.push_str(&format!(
            "  - {ext_id}: skipped — requires mechanism {:?}, which bastion-agent doesn't wire \
             into a pack install yet (tracked separately)\n",
            manifest.kind
        ));
        return report;
    }
    if !manifest.provides.is_empty() {
        report.push_str(&format!(
            "  - {ext_id}: skipped — declarative extension with non-empty `provides` needs \
             artifact-data loading, not implemented yet\n"
        ));
        return report;
    }
    let instance: Arc<dyn ExtensionInstance> =
        Arc::new(DeclarativeExtension::new(manifest.clone(), vec![]));
    report.push_str(
        &match host.install(instance, owner, &PermissionSet::none()).await {
            Ok(()) => format!("  + {ext_id}: installed\n"),
            Err(e) => format!("  ! {ext_id}: install failed — {e}\n"),
        },
    );
    report
}

/// Reconciles whatever `[[mcp_dependencies]]` `raw` declares into
/// `bastion_toml_path`'s `[mcp.servers.*]` — orthogonal to `kind`/`provides`
/// (a manifest can be `declarative` with no capability of its own AND still
/// carry an MCP dependency, e.g. `bastion/context7-mcp`). A manifest with no
/// `mcp_dependencies` produces an empty report line (nothing to reconcile).
async fn reconcile_one_extension_mcp_deps(
    raw: &str,
    bastion_toml_path: &str,
    ext_id: &str,
) -> String {
    let deps = crate::extension::parse_mcp_dependencies(raw);
    if deps.is_empty() {
        return String::new();
    }
    match crate::extension::reconcile_mcp_dependencies(&deps, bastion_toml_path).await {
        Ok(added) if added.is_empty() => {
            format!("  = {ext_id}: mcp dependencies already present in {bastion_toml_path}\n")
        }
        Ok(added) => format!(
            "  + {ext_id}: added [mcp.servers.{}] to {bastion_toml_path} (restart the daemon to \
             activate)\n",
            added.join("], [mcp.servers.")
        ),
        Err(e) => format!(
            "  ! {ext_id}: failed to reconcile mcp dependencies into {bastion_toml_path} — {e}\n"
        ),
    }
}

/// `CliCapability::git`, wrapped as the ONE `ExtensionInstance` this install
/// flow builds for `native_crate` today. Workspace defaults to the daemon's
/// current working directory — there is no separate "project workspace"
/// config concept yet; document this plainly rather than pretending it's
/// configurable.
struct GitCliExtension {
    manifest: ExtensionManifest,
    workspace: std::path::PathBuf,
}

#[async_trait::async_trait]
impl ExtensionInstance for GitCliExtension {
    fn manifest(&self) -> &ExtensionManifest {
        &self.manifest
    }

    async fn activate(&self, facade: &mut HostFacade<'_>) -> Result<(), ExtensionError> {
        facade.register_capability(Arc::new(CliCapability::git(self.workspace.clone())))?;
        Ok(())
    }

    async fn deactivate(&self, facade: &mut HostFacade<'_>) -> Result<(), ExtensionError> {
        facade.deregister_capability("git");
        Ok(())
    }
}

async fn install_git_capability(
    host: &mut ExtensionHost,
    owner: &str,
    manifest: &ExtensionManifest,
) -> String {
    let workspace = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let instance: Arc<dyn ExtensionInstance> = Arc::new(GitCliExtension {
        manifest: manifest.clone(),
        workspace: workspace.clone(),
    });
    let ceiling = PermissionSet {
        capabilities: vec!["git".to_string()],
        ..PermissionSet::none()
    };
    match host.install(instance, owner, &ceiling).await {
        Ok(()) => format!(
            "  + {GIT_CAPABILITY_CRATE_NAME}: installed (workspace: {})\n",
            workspace.display()
        ),
        Err(e) => format!("  ! {GIT_CAPABILITY_CRATE_NAME}: install failed — {e}\n"),
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

/// A pack's own `personas`/`skills` name list is untrusted input (the pack
/// author, not the operator) — reject anything that isn't a single plain
/// path segment before it ever reaches a `Path::join`. Blocks `..`,
/// separators (`/`, `\`), and absolute paths, which would otherwise let a
/// malicious pack write outside both the source pack directory and the
/// operator's persona/skills directory.
fn is_safe_member_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
        && !Path::new(name).is_absolute()
}

/// Copies each named subdirectory of `src_root` to `dest_of(name)`, best
/// effort per member — one failure never blocks the others. Rejects any
/// name that isn't a safe single path segment (see `is_safe_member_name`)
/// before ever joining it into a path.
fn copy_pack_members(
    src_root: &Path,
    names: &[String],
    dest_of: impl Fn(&str) -> std::path::PathBuf,
) -> CopyResults {
    let mut ok = Vec::new();
    let mut failed = Vec::new();
    if src_root.is_dir() {
        for name in names {
            if !is_safe_member_name(name) {
                failed.push((
                    name.clone(),
                    "unsafe member name (must be a single path segment, no '..' or separators)"
                        .to_string(),
                ));
                continue;
            }
            let src = src_root.join(name);
            match copy_dir(&src, &dest_of(name)) {
                Ok(()) => ok.push(name.clone()),
                Err(e) => failed.push((name.clone(), e.to_string())),
            }
        }
    }
    CopyResults { ok, failed }
}

/// Keyed by manifest id, each value carries the parsed `ExtensionManifest`
/// AND the raw TOML text — the latter so `mcp_dependencies` (not part of
/// `ExtensionManifest`'s own fields) can still be recovered per extension.
fn load_extension_manifests(dir: &Path) -> HashMap<String, (ExtensionManifest, String)> {
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
            out.insert(manifest.id.clone(), (manifest, raw));
        }
    }
    out
}

fn copy_dir(src: &Path, dest: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        // `file_type()` on a `DirEntry` does NOT follow symlinks (unlike
        // `Path::metadata`) — a symlinked entry reports `is_symlink() ==
        // true` here, not the type of whatever it points to. A pack could
        // otherwise ship a symlink pointing outside its own directory and
        // have it silently followed by `copy_dir`'s recursion/`fs::copy`.
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            return Err(std::io::Error::other(format!(
                "refusing to follow symlink in pack content: {}",
                entry.path().display()
            )));
        }
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
            "/nonexistent/bastion.toml",
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
            "/nonexistent/bastion.toml",
            "alice",
            pack_root.path().to_str().unwrap(),
        )
        .await;

        assert!(
            report.contains(
                "acme/native-thing: skipped — native_crate 'acme/native-thing' has \
                              no known mapping"
            ),
            "{report}"
        );
        assert!(!host.is_installed("acme/native-thing"));
    }

    #[tokio::test]
    async fn install_wires_git_capability_native_crate_by_name() {
        let pack_root = TempDir::new().unwrap();
        write_pack(
            pack_root.path(),
            r#"
                id = "thewaifucorp/software-sdlc"
                version = "1.0.0"
                extensions = [["bastion/git-capability", "*"]]
                skills = []
                personas = []

                [defaults]
                enabled_extensions = []
            "#,
            &[],
            &[(
                "git-capability",
                r#"
                    id = "bastion/git-capability"
                    version = "1.0.0"
                    kind = "native_crate"
                    compat = "*"
                    provides = [{ kind = "capability", name = "git" }]
                    requires = []
                    secrets = []
                    migrations = []

                    [permissions]
                    capabilities = ["git"]

                    [entrypoint]
                    kind = "native_crate"
                    crate_name = "bastion/git-capability"

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
            ".",
            "/nonexistent/bastion.toml",
            "alice",
            pack_root.path().to_str().unwrap(),
        )
        .await;

        assert!(
            report.contains("bastion/git-capability: installed"),
            "{report}"
        );
        assert!(host.is_installed("bastion/git-capability"));
    }

    #[tokio::test]
    async fn install_reconciles_mcp_dependencies_for_a_provides_nothing_extension() {
        let pack_root = TempDir::new().unwrap();
        // Bound separately (not `TempDir::new().unwrap().path().join(...)`) —
        // an unbound TempDir drops (deleting the directory) at the end of
        // the statement that creates it, before this test ever reads it.
        let bastion_toml_dir = TempDir::new().unwrap();
        let bastion_toml = bastion_toml_dir.path().join("bastion.toml");
        std::fs::write(
            &bastion_toml,
            "[session]\ndb_path = \".bastion/sessions.db\"\n",
        )
        .unwrap();

        write_pack(
            pack_root.path(),
            r#"
                id = "thewaifucorp/software-sdlc"
                version = "1.0.0"
                extensions = [["bastion/context7-mcp", "*"]]
                skills = []
                personas = []

                [defaults]
                enabled_extensions = []
            "#,
            &[],
            &[(
                "context7-mcp",
                r#"
                    id = "bastion/context7-mcp"
                    version = "1.0.0"
                    kind = "declarative"
                    compat = "*"
                    provides = []
                    requires = []
                    secrets = []
                    migrations = []

                    [[mcp_dependencies]]
                    name = "context7"
                    endpoint = "https://mcp.context7.com/mcp"
                    read_only = true

                    [permissions]

                    [entrypoint]
                    kind = "declarative"
                    artifact_path = "context7.json"

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
            ".",
            bastion_toml.to_str().unwrap(),
            "alice",
            pack_root.path().to_str().unwrap(),
        )
        .await;

        assert!(report.contains("added [mcp.servers.context7]"), "{report}");
        assert!(
            report.contains("bastion/context7-mcp: installed"),
            "{report}"
        );

        let contents = std::fs::read_to_string(&bastion_toml).unwrap();
        assert!(contents.contains("[mcp.servers.context7]"));
        assert!(contents.contains("https://mcp.context7.com/mcp"));

        // Re-installing (e.g. a second pack member reusing the same server)
        // must not duplicate the entry.
        let mut host2 = ExtensionHost::new();
        let report2 = install(
            &mut host2,
            ".",
            bastion_toml.to_str().unwrap(),
            "alice",
            pack_root.path().to_str().unwrap(),
        )
        .await;
        assert!(
            report2.contains("mcp dependencies already present"),
            "{report2}"
        );
        let contents2 = std::fs::read_to_string(&bastion_toml).unwrap();
        assert_eq!(contents2.matches("[mcp.servers.context7]").count(), 1);
    }

    #[tokio::test]
    async fn install_reports_missing_pack_toml_clearly() {
        let empty = TempDir::new().unwrap();
        let mut host = ExtensionHost::new();
        let report = install(
            &mut host,
            ".",
            "/nonexistent/bastion.toml",
            "alice",
            empty.path().to_str().unwrap(),
        )
        .await;
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
        install(
            &mut host,
            ".",
            "/nonexistent/bastion.toml",
            "alice",
            pack_root.path().to_str().unwrap(),
        )
        .await;
        assert_eq!(
            list(&host),
            "installed extensions:\n  acme/noop-mcp  v1.0.0"
        );

        let out = handle(
            &mut host,
            ".",
            "/nonexistent/bastion.toml",
            Some("revoke acme/noop-mcp"),
            "alice",
        )
        .await
        .unwrap();
        assert_eq!(out, "extension acme/noop-mcp revoked.");
        assert_eq!(list(&host), "no extensions installed.");
    }

    #[test]
    fn is_safe_member_name_rejects_traversal_and_absolute_paths() {
        assert!(is_safe_member_name("tech-lead"));
        assert!(!is_safe_member_name(".."));
        assert!(!is_safe_member_name("../../etc/cron.d/evil"));
        assert!(!is_safe_member_name("a/../../b"));
        assert!(!is_safe_member_name("a/b"));
        assert!(!is_safe_member_name("a\\b"));
        assert!(!is_safe_member_name("/etc/passwd"));
        assert!(!is_safe_member_name(""));
        assert!(!is_safe_member_name("."));
    }

    #[tokio::test]
    async fn install_rejects_path_traversal_in_persona_name_without_touching_disk() {
        let pack_root = TempDir::new().unwrap();
        let personas_dest = TempDir::new().unwrap();
        let outside_marker = personas_dest.path().parent().unwrap().join("pwned");

        write_pack(
            pack_root.path(),
            r#"
                id = "acme/evil-pack"
                version = "1.0.0"
                extensions = []
                skills = []
                personas = ["../pwned"]

                [defaults]
                enabled_extensions = []
            "#,
            &[("../pwned", "---\nname: pwned\n---\nbody")],
            &[],
        );

        let mut host = ExtensionHost::new();
        let report = install(
            &mut host,
            personas_dest.path().to_str().unwrap(),
            "/nonexistent/bastion.toml",
            "alice",
            pack_root.path().to_str().unwrap(),
        )
        .await;

        assert!(report.contains("unsafe member name"), "{report}");
        assert!(
            !outside_marker.exists(),
            "path traversal must never write outside the destination root"
        );
    }

    #[test]
    fn copy_dir_refuses_to_follow_symlinks() {
        let src = TempDir::new().unwrap();
        let dest = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "top secret").unwrap();

        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), src.path().join("escape")).unwrap();
        #[cfg(not(unix))]
        return; // symlink construction is unix-specific; nothing to assert elsewhere.

        let result = copy_dir(src.path(), &dest.path().join("copied"));
        assert!(result.is_err(), "copy_dir must refuse a symlinked entry");
        assert!(!dest.path().join("copied/escape/secret.txt").exists());
    }
}
