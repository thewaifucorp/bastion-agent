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
//! itself describes the split). Skills are copied into the SAME directory
//! `SkillsLoader::load_all` scans at boot and `skill-writer`'s own MCP tools
//! read from (`skills_dir()`, `SKILLS_DIR` env var) — in the Docker
//! deployment specifically, `core`'s `/skills` mount is read-only by design
//! (D-10: only `skill-writer` writes skills), so a pack's skill members
//! fail to copy there today with a clear permission error rather than
//! silently landing somewhere unwatched; running natively (no `:ro` mount)
//! they copy and get picked up on the next boot-time scan or a
//! `skill-writer` reload signal.
//!
//! Optional persona selection: a pack's `pack.toml` may declare
//! `[personas_selection] required = [...]` — any name in `personas` not listed there is optional,
//! shown as a numbered menu, chosen interactively by the operator via
//! [`crate::agent::console_prompt::PendingConsolePrompt`]. Mirrors the exact
//! pattern `crate::extension::mcp_reconciler::parse_mcp_dependencies` already
//! established for `mcp_dependencies`: `bastion_extension_protocol::
//! PackManifest` is NOT modified to model this (a `bastion-core` change,
//! deliberately out of scope) — parsed directly from the pack's raw TOML
//! text instead. A pack with no `[personas_selection]` table (every pack
//! that exists today: `software-sdlc`, `finance-research`, `agent-payments`)
//! installs with EXACTLY the same behavior as before this feature existed —
//! `install` commits immediately, no prompt, `required` defaults to every
//! name in `personas`.
//!
//! `/extension revoke` reuses the exact same preview/commit split and the
//! same `PendingConsolePrompt` mechanism (`revoke_preview`/`revoke_commit`,
//! `HandleOutcome::AwaitingRevokeConfirmation`) — the second real consumer,
//! validating the mechanism generalizes past the one it was originally
//! built for.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bastion_extension_protocol::{
    Entrypoint, ExtensionError, ExtensionKind, ExtensionManifest, PackManifest, PermissionSet,
};

use crate::extension::declarative::DeclarativeExtension;
use crate::extension::persistence::{PersistedExtension, ReconstructKind, SqliteExtensionStore};
use crate::extension::{CliCapability, ExtensionHost, ExtensionInstance, HostFacade};

/// Result of `/extension <sub>`. `install` can produce `AwaitingPersonaSelection`
/// (when the pack declares `[personas_selection]` and has at least one optional
/// persona); `revoke` on an id that's actually installed produces
/// `AwaitingRevokeConfirmation` — everything else (including `revoke` on an
/// unknown id, and `install` on a pack with nothing optional to choose)
/// produces `Done`.
///
/// `AwaitingRevokeConfirmation` is the SECOND real consumer of
/// [`crate::agent::console_prompt::PendingConsolePrompt`] (the first being
/// `AwaitingPersonaSelection`) — deliberately built to validate that the
/// pending-prompt mechanism generalizes past its original single use case,
/// rather than deferring that question. Revoke is a real, already-existing,
/// already-destructive action (it deactivates a capability a persona may be
/// relying on) — not a
/// prompt invented just to have a second consumer.
#[derive(Debug, PartialEq)]
pub enum HandleOutcome {
    Done(String),
    AwaitingPersonaSelection {
        report: String,
        pack_dir: PathBuf,
        required: Vec<String>,
        optional: Vec<String>,
    },
    AwaitingRevokeConfirmation {
        report: String,
        id: String,
    },
}

/// Handle `/extension <sub> [args]`.
pub async fn handle(
    host: &mut ExtensionHost,
    store: &SqliteExtensionStore,
    personas_dir: &str,
    bastion_toml_path: &str,
    arg: Option<&str>,
    owner: &str,
) -> anyhow::Result<HandleOutcome> {
    let arg = arg.unwrap_or("").trim();
    let (sub, rest) = match arg.split_once(char::is_whitespace) {
        Some((s, r)) => (s, r.trim()),
        None => (arg, ""),
    };
    match sub {
        "" | "list" => Ok(HandleOutcome::Done(list(host))),
        "install" => Ok(
            match install(host, store, personas_dir, bastion_toml_path, owner, rest).await {
                InstallOutcome::Done(msg) => HandleOutcome::Done(msg),
                InstallOutcome::AwaitingPersonaSelection {
                    report,
                    pack_dir,
                    required,
                    optional,
                } => HandleOutcome::AwaitingPersonaSelection {
                    report,
                    pack_dir,
                    required,
                    optional,
                },
            },
        ),
        "revoke" => Ok(revoke_preview(host, rest)),
        other => Ok(HandleOutcome::Done(format!(
            "unknown /extension subcommand '{other}'. Use: install <path> | list | revoke <id>"
        ))),
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

/// Outcome of the "preview" phase (`install`, below). `Done` needs no further
/// input — either nothing was optional, or the install failed outright.
/// `AwaitingPersonaSelection` means the report was already printed (the
/// menu) and NO file was copied yet; the caller (`main.rs`) is expected to
/// hold `pack_dir`/`required`/`optional` until the operator's next line
/// answers it, then call [`install_commit`] itself.
#[derive(Debug, PartialEq)]
enum InstallOutcome {
    Done(String),
    AwaitingPersonaSelection {
        report: String,
        pack_dir: PathBuf,
        required: Vec<String>,
        optional: Vec<String>,
    },
}

/// Preview phase: parse the pack, decide whether there's anything optional
/// to ask about. A pack with no `[personas_selection]` table — every pack
/// that exists today — has `optional` come out empty and goes straight to
/// [`install_commit`], byte-for-byte the same report shape this function
/// produced before persona selection existed.
async fn install(
    host: &mut ExtensionHost,
    store: &SqliteExtensionStore,
    personas_dir: &str,
    bastion_toml_path: &str,
    owner: &str,
    path: &str,
) -> InstallOutcome {
    if path.is_empty() {
        return InstallOutcome::Done("usage: /extension install <path>".to_string());
    }
    let pack_dir = Path::new(path);
    let pack_toml_path = pack_dir.join("pack.toml");
    let raw = match std::fs::read_to_string(&pack_toml_path) {
        Ok(s) => s,
        Err(e) => {
            return InstallOutcome::Done(format!("cannot read {}: {e}", pack_toml_path.display()))
        }
    };
    let pack: PackManifest = match toml::from_str(&raw) {
        Ok(p) => p,
        Err(e) => {
            return InstallOutcome::Done(format!(
                "invalid pack.toml at {}: {e}",
                pack_toml_path.display()
            ))
        }
    };

    // Table absent/malformed -> every persona is required, exactly today's
    // behavior — this is what keeps every existing pack.toml (none of which
    // declare this table) installing unchanged.
    let required = parse_personas_selection(&raw).unwrap_or_else(|| pack.personas.clone());
    let optional: Vec<String> = pack
        .personas
        .iter()
        .filter(|p| !required.contains(p))
        .cloned()
        .collect();

    if optional.is_empty() {
        let report = install_commit(
            host,
            store,
            personas_dir,
            bastion_toml_path,
            owner,
            pack_dir,
            &required,
            &[],
        )
        .await;
        return InstallOutcome::Done(report);
    }

    let mut report = format!("installing {} v{}\n", pack.id, pack.version);
    report.push_str(&format!(
        "  required (always installed): {}\n",
        if required.is_empty() {
            "(none)".to_string()
        } else {
            required.join(", ")
        }
    ));
    report.push_str("  optional — pick which to also install:\n");
    for (i, name) in optional.iter().enumerate() {
        report.push_str(&format!("    {}. {name}\n", i + 1));
    }
    report.push_str(
        "  reply with numbers and/or names, comma-separated (e.g. \"1,3\" or \"burry,ackman\"), \
         \"all\", or \"none\"",
    );

    InstallOutcome::AwaitingPersonaSelection {
        report,
        pack_dir: pack_dir.to_path_buf(),
        required,
        optional,
    }
}

/// Commit phase: re-reads and re-parses `pack.toml` from `pack_dir` (cheap,
/// and avoids needing to carry a whole `PackManifest` across the pending-
/// prompt boundary between `install`'s preview and this call) and copies
/// `required` ∪ `selection` — this is the body `install` itself used to run
/// unconditionally before persona selection existed.
#[allow(clippy::too_many_arguments)] // command-glue bag, same precedent as loadout::router
pub(crate) async fn install_commit(
    host: &mut ExtensionHost,
    store: &SqliteExtensionStore,
    personas_dir: &str,
    bastion_toml_path: &str,
    owner: &str,
    pack_dir: &Path,
    required: &[String],
    selection: &[String],
) -> String {
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

    let mut personas_to_install: Vec<String> = required.to_vec();
    for name in selection {
        if !personas_to_install.contains(name) {
            personas_to_install.push(name.clone());
        }
    }

    let personas_copied =
        copy_pack_members(&pack_dir.join("personas"), &personas_to_install, |name| {
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

    // Target the SAME directory `SkillsLoader::load_all`/`skill-writer`'s own
    // MCP tools read (`skills_dir()`, `SKILLS_DIR` env var, default
    // `/skills`) — a bare relative "skills" here previously resolved
    // against the daemon's cwd, which only ever matched `/skills` in the
    // Docker deployment by accident (cwd there happens to be `/`) and never
    // matched it running natively. In Docker, `core`'s own `/skills` mount
    // is read-only by design (D-10: only `skill-writer` writes skills) —
    // this now fails honestly with a clear permission error there instead
    // of silently writing to the wrong place, until installing a pack's
    // skills is routed through `skill-writer` instead of `core` itself.
    let skills_target_dir = crate::agent::skills::skills_dir();
    let skills_copied = copy_pack_members(&pack_dir.join("skills"), &pack.skills, |name| {
        Path::new(&skills_target_dir).join(name)
    });
    for (name, error) in &skills_copied.failed {
        report.push_str(&format!("  ! skill {name}: failed to copy — {error}\n"));
    }
    if !skills_copied.ok.is_empty() {
        report.push_str(&format!(
            "  skills copied: {} (picked up by the next boot-time scan or a skill-writer \
             reload signal, same as any other skill under {skills_target_dir})\n",
            skills_copied.ok.join(", ")
        ));
    }

    let manifests = load_extension_manifests(&pack_dir.join("extensions"));
    for (ext_id, _version_req) in &pack.extensions {
        report.push_str(
            &install_one_extension(host, store, owner, &manifests, bastion_toml_path, ext_id).await,
        );
    }

    report.trim_end().to_string()
}

/// One `pack.toml`'s `[personas_selection]` table. `bastion_extension_protocol::
/// PackManifest` doesn't model this at all (deliberately — see module doc);
/// parsed straight from the raw TOML text, mirroring
/// `crate::extension::mcp_reconciler::parse_mcp_dependencies`'s exact
/// pattern for the same reason.
#[derive(serde::Deserialize, Default)]
struct ManifestPersonasSelection {
    #[serde(default)]
    personas_selection: Option<PersonasSelectionTable>,
}

#[derive(serde::Deserialize, Default)]
struct PersonasSelectionTable {
    #[serde(default)]
    required: Vec<String>,
}

/// Extracts `[personas_selection].required` from a raw `pack.toml` string.
/// `None` — the table is absent, or the raw text doesn't parse at all under
/// this narrower shape — means "every persona is required", not "zero
/// personas required": callers must NOT treat `None` and `Some(vec![])` the
/// same way. A malformed table is never the reason a whole install fails.
fn parse_personas_selection(raw_pack_toml: &str) -> Option<Vec<String>> {
    toml::from_str::<ManifestPersonasSelection>(raw_pack_toml)
        .ok()
        .and_then(|m| m.personas_selection)
        .map(|t| t.required)
}

/// What the operator's reply line resolved to: which optional personas were
/// selected, and which tokens in the reply didn't match anything (reported
/// back, never silently dropped).
#[derive(Debug, PartialEq, Default)]
pub(crate) struct PersonaSelectionResult {
    pub selected: Vec<String>,
    pub ignored: Vec<String>,
}

/// Parses an operator's reply to the persona-selection menu against
/// `optional` (the exact list `install`'s menu was numbered from — indices
/// are always relative to THIS list, 1-indexed). Accepts, per token: a
/// 1-indexed number, or a persona name (case-insensitive), comma-separated;
/// the whole reply may instead be `all` or `none`/empty (case-insensitive).
/// A duplicate selection (same persona picked twice, by number and by name)
/// is de-duplicated, not reported as an error.
pub(crate) fn parse_persona_selection(reply: &str, optional: &[String]) -> PersonaSelectionResult {
    let reply = reply.trim();
    if reply.is_empty() || reply.eq_ignore_ascii_case("none") {
        return PersonaSelectionResult::default();
    }
    if reply.eq_ignore_ascii_case("all") {
        return PersonaSelectionResult {
            selected: optional.to_vec(),
            ignored: Vec::new(),
        };
    }

    let mut result = PersonaSelectionResult::default();
    for token in reply.split(',') {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        let matched = if let Ok(idx) = token.parse::<usize>() {
            idx.checked_sub(1).and_then(|i| optional.get(i)).cloned()
        } else {
            optional
                .iter()
                .find(|name| name.eq_ignore_ascii_case(token))
                .cloned()
        };
        match matched {
            Some(name) => {
                if !result.selected.contains(&name) {
                    result.selected.push(name);
                }
            }
            None => result.ignored.push(token.to_string()),
        }
    }
    result
}

/// `bastion/git-capability`'s `crate_name` — the one `native_crate` mapping
/// this install flow recognizes today. A second native_crate consumer would
/// warrant a real registry (`crate_name` -> constructor); with exactly one,
/// a hardcoded match is honest about the actual state of the mechanism
/// (`docs/en/...` should say so too — see the pack's own README note).
const GIT_CAPABILITY_CRATE_NAME: &str = "bastion/git-capability";

async fn install_one_extension(
    host: &mut ExtensionHost,
    store: &SqliteExtensionStore,
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
            report.push_str(&install_git_capability(host, store, owner, manifest).await);
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
            Ok(()) => {
                persist_or_warn(
                    store,
                    owner,
                    manifest,
                    ReconstructKind::Declarative,
                    ext_id,
                    format!("  + {ext_id}: installed\n"),
                )
                .await
            }
            Err(e) => format!("  ! {ext_id}: install failed — {e}\n"),
        },
    );
    report
}

/// Persists a just-activated extension — called only after `host.install`
/// already succeeded, so a persistence failure here means it just won't
/// survive the next restart, not that the install itself is broken.
/// Downgrades `ok_line`'s success message into a warning instead of
/// silently swallowing the error. Shared by `install_one_extension` and
/// `install_git_capability`, which previously each hand-rolled this exact
/// save-then-report shape.
async fn persist_or_warn(
    store: &SqliteExtensionStore,
    owner: &str,
    manifest: &ExtensionManifest,
    kind: ReconstructKind,
    id_for_log: &str,
    ok_line: String,
) -> String {
    match store.save(owner, manifest, kind).await {
        Ok(()) => ok_line,
        Err(e) => {
            tracing::error!(
                event = "extension_persist_failed",
                id = %id_for_log,
                error = %e,
            );
            format!(
                "{} (warning: failed to persist — won't survive a restart: {e})\n",
                ok_line.trim_end()
            )
        }
    }
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
    store: &SqliteExtensionStore,
    owner: &str,
    manifest: &ExtensionManifest,
) -> String {
    let workspace = git_capability_workspace();
    let instance: Arc<dyn ExtensionInstance> = Arc::new(GitCliExtension {
        manifest: manifest.clone(),
        workspace: workspace.clone(),
    });
    let ceiling = PermissionSet {
        capabilities: vec!["git".to_string()],
        ..PermissionSet::none()
    };
    match host.install(instance, owner, &ceiling).await {
        Ok(()) => {
            persist_or_warn(
                store,
                owner,
                manifest,
                ReconstructKind::GitCapability,
                GIT_CAPABILITY_CRATE_NAME,
                format!(
                    "  + {GIT_CAPABILITY_CRATE_NAME}: installed (workspace: {})\n",
                    workspace.display()
                ),
            )
            .await
        }
        Err(e) => format!("  ! {GIT_CAPABILITY_CRATE_NAME}: install failed — {e}\n"),
    }
}

/// The daemon's cwd, used as the git capability's workspace — see
/// `GitCliExtension`'s own doc comment: there is no separate "project
/// workspace" config concept yet, so cwd is the honest, documented default.
/// Shared by `install_git_capability` (live `/extension install`) and
/// `reload_persisted` (boot-time reactivation) so both resolve the same way.
fn git_capability_workspace() -> std::path::PathBuf {
    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
}

/// Preview phase: an unknown/empty id fails immediately (nothing to confirm).
/// A real installed id asks for confirmation instead of revoking on the
/// spot — revoke deactivates a capability that some persona's turn may
/// currently depend on, so a mistyped id or a slip of the finger shouldn't
/// be irreversible with zero chance to back out.
fn revoke_preview(host: &ExtensionHost, id: &str) -> HandleOutcome {
    if id.is_empty() {
        return HandleOutcome::Done("usage: /extension revoke <id>".to_string());
    }
    if !host.is_installed(id) {
        return HandleOutcome::Done(format!("cannot revoke {id}: not installed"));
    }
    HandleOutcome::AwaitingRevokeConfirmation {
        report: format!("revoke {id}? this deactivates it immediately. reply yes/no (or sim/não)"),
        id: id.to_string(),
    }
}

/// Commit phase: called from [`crate::agent::console_prompt::resolve`] once
/// the operator's reply is parsed. Only ever invoked after `revoke_preview`
/// already confirmed `id` is installed — a second check here is still
/// correct defense (nothing else can uninstall it in between on this
/// single-threaded REPL loop, but `host.revoke` is the actual source of
/// truth either way).
///
/// Fail-closed ordering: the persisted record is cleared FIRST, before the
/// in-memory `host.revoke`. If clearing the record fails, the extension
/// stays fully active (both in memory and on disk) and the operator is told
/// revoke did NOT happen — the previous ordering deactivated in memory
/// first and only warned on a persistence failure, which left a revoked
/// extension's record on disk to be silently reactivated by
/// `reload_persisted` on the next restart. This ordering can still leave
/// the opposite (safe) split — persisted record gone, in-memory revoke
/// itself fails — reported honestly below rather than claiming success.
pub(crate) async fn revoke_commit(
    host: &mut ExtensionHost,
    store: &SqliteExtensionStore,
    id: &str,
) -> String {
    if let Err(e) = store.remove(id).await {
        tracing::error!(event = "extension_persist_remove_failed", id = %id, error = %e);
        return format!(
            "cannot revoke {id}: failed to clear persisted record — {e}. Extension is still \
             active; nothing was changed."
        );
    }
    match host.revoke(id).await {
        Ok(()) => format!("extension {id} revoked."),
        Err(e) => {
            // The persisted record is already gone, so a restart will NOT
            // reactivate this extension — the fail-closed direction is
            // already secured. It just means the operator must know it's
            // still live in THIS process until the next restart clears it.
            tracing::error!(
                event = "extension_revoke_failed_after_persist_removed",
                id = %id,
                error = %e,
            );
            format!(
                "extension {id}: persisted record cleared, but deactivating it in this running \
                 process failed — {e}. It will not survive a restart, but is still active until \
                 then."
            )
        }
    }
}

/// Parses an operator's yes/no reply to a revoke confirmation. Anything that
/// isn't a recognized affirmative is treated as "no" — same v1 philosophy as
/// `parse_persona_selection`: a prompt is never re-asked on a bad reply, it
/// just resolves to the safe (non-destructive) outcome.
pub(crate) fn parse_revoke_confirmation(reply: &str) -> bool {
    matches!(
        reply.trim().to_ascii_lowercase().as_str(),
        "yes" | "y" | "sim" | "s"
    )
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

/// The SAME fixed ceiling `install_one_extension`/`install_git_capability`
/// pass to `host.install` for a live `/extension install`, indexed by
/// `ReconstructKind` — `reload_persisted` below reuses this instead of
/// `manifest.permissions.clone()`, which would check the manifest's
/// permissions against ITSELF (`X.is_subset_of(X)` is always true) and so
/// enforce no real ceiling at all. If a persisted row's `permissions` were
/// ever wider than what its kind is actually allowed (a corrupted row, or a
/// future bug elsewhere writing a bad record), the live-install path would
/// have rejected it — reload must reject it too, not wave every persisted
/// row through unchecked.
fn reload_ceiling_for(kind: &ReconstructKind) -> PermissionSet {
    match kind {
        ReconstructKind::Declarative => PermissionSet::none(),
        ReconstructKind::GitCapability => PermissionSet {
            capabilities: vec!["git".to_string()],
            ..PermissionSet::none()
        },
    }
}

/// Boot-time reload: reactivate every persisted extension against a fresh
/// `ExtensionHost`, reusing the EXACT SAME `Arc<dyn ExtensionInstance>`
/// construction `install_one_extension`/`install_git_capability` use for a
/// live `/extension install` — never a second activation path. Best-effort
/// per row: a row that fails to parse or reactivate is logged and skipped,
/// never fatal to daemon startup (one corrupt/stale record must not block
/// boot). The permission ceiling passed to `host.install` is
/// `reload_ceiling_for(kind)` — the same FIXED ceiling the original install
/// used, independent of whatever this row's own `manifest.permissions`
/// says, so a persisted row can never grant itself more authority than its
/// kind was ever allowed to have.
pub async fn reload_persisted(
    host: &mut ExtensionHost,
    store: &SqliteExtensionStore,
) -> Vec<String> {
    let mut lines = Vec::new();
    let persisted = match store.load_all_parsed().await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(event = "extension_persistence_load_failed", error = %e);
            lines.push(format!("  ! failed to read persisted extensions: {e}"));
            return lines;
        }
    };
    for PersistedExtension {
        owner,
        manifest,
        kind,
    } in persisted
    {
        let id = manifest.id.clone();
        let ceiling = reload_ceiling_for(&kind);
        let instance: Arc<dyn ExtensionInstance> = match kind {
            ReconstructKind::Declarative => Arc::new(DeclarativeExtension::new(manifest, vec![])),
            ReconstructKind::GitCapability => {
                let workspace = git_capability_workspace();
                Arc::new(GitCliExtension {
                    manifest,
                    workspace,
                })
            }
        };
        match host.install(instance, &owner, &ceiling).await {
            Ok(()) => lines.push(format!("  + {id}: reactivated from persisted loadout")),
            Err(e) => {
                tracing::warn!(event = "extension_reload_failed", id = %id, error = %e);
                lines.push(format!("  ! {id}: failed to reactivate — {e}"));
            }
        }
    }
    lines
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

    /// A fresh, isolated `SqliteExtensionStore` per test — persistence
    /// itself is covered by `extension::persistence`'s own tests; here it
    /// only needs to exist so `install`/`install_commit`/`revoke_commit`
    /// have somewhere to write.
    async fn test_store() -> (tempfile::NamedTempFile, SqliteExtensionStore) {
        let f = tempfile::NamedTempFile::new().unwrap();
        let s = SqliteExtensionStore::new(f.path().to_str().unwrap());
        s.init_schema().await.unwrap();
        (f, s)
    }

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
        let (_f, store) = test_store().await;
        let InstallOutcome::Done(report) = install(
            &mut host,
            &store,
            personas_dest.path().to_str().unwrap(),
            "/nonexistent/bastion.toml",
            "alice",
            pack_root.path().to_str().unwrap(),
        )
        .await
        else {
            panic!("pack has no [personas_selection] — must commit immediately")
        };

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
        let (_f, store) = test_store().await;
        let InstallOutcome::Done(report) = install(
            &mut host,
            &store,
            personas_dest.path().to_str().unwrap(),
            "/nonexistent/bastion.toml",
            "alice",
            pack_root.path().to_str().unwrap(),
        )
        .await
        else {
            panic!("pack has no [personas_selection] — must commit immediately")
        };

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
        let (_f, store) = test_store().await;
        let InstallOutcome::Done(report) = install(
            &mut host,
            &store,
            ".",
            "/nonexistent/bastion.toml",
            "alice",
            pack_root.path().to_str().unwrap(),
        )
        .await
        else {
            panic!("pack has no [personas_selection] — must commit immediately")
        };

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
        let (_f, store) = test_store().await;
        let InstallOutcome::Done(report) = install(
            &mut host,
            &store,
            ".",
            bastion_toml.to_str().unwrap(),
            "alice",
            pack_root.path().to_str().unwrap(),
        )
        .await
        else {
            panic!("pack has no [personas_selection] — must commit immediately")
        };

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
        let InstallOutcome::Done(report2) = install(
            &mut host2,
            &store,
            ".",
            bastion_toml.to_str().unwrap(),
            "alice",
            pack_root.path().to_str().unwrap(),
        )
        .await
        else {
            panic!("pack has no [personas_selection] — must commit immediately")
        };
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
        let (_f, store) = test_store().await;
        let InstallOutcome::Done(report) = install(
            &mut host,
            &store,
            ".",
            "/nonexistent/bastion.toml",
            "alice",
            empty.path().to_str().unwrap(),
        )
        .await
        else {
            panic!("missing pack.toml — must fail immediately, not await a prompt")
        };
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
        let (_f, store) = test_store().await;
        install(
            &mut host,
            &store,
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
            &store,
            ".",
            "/nonexistent/bastion.toml",
            Some("revoke acme/noop-mcp"),
            "alice",
        )
        .await
        .unwrap();
        let HandleOutcome::AwaitingRevokeConfirmation { report, id } = out else {
            panic!(
                "revoking an installed id must ask for confirmation first, not revoke immediately"
            )
        };
        assert_eq!(id, "acme/noop-mcp");
        assert!(report.contains("acme/noop-mcp"), "{report}");
        // Confirmation not yet given — still installed.
        assert_eq!(
            list(&host),
            "installed extensions:\n  acme/noop-mcp  v1.0.0"
        );

        let report = revoke_commit(&mut host, &store, &id).await;
        assert_eq!(report, "extension acme/noop-mcp revoked.");
        assert_eq!(list(&host), "no extensions installed.");
    }

    #[tokio::test]
    async fn revoke_of_unknown_id_fails_immediately_without_asking() {
        let mut host = ExtensionHost::new();
        let (_f, store) = test_store().await;
        let out = handle(
            &mut host,
            &store,
            ".",
            "/nonexistent/bastion.toml",
            Some("revoke nope"),
            "alice",
        )
        .await
        .unwrap();
        assert_eq!(
            out,
            HandleOutcome::Done("cannot revoke nope: not installed".to_string())
        );
    }

    #[test]
    fn revoke_confirmation_accepts_only_recognized_affirmatives() {
        assert!(parse_revoke_confirmation("yes"));
        assert!(parse_revoke_confirmation("Y"));
        assert!(parse_revoke_confirmation("sim"));
        assert!(parse_revoke_confirmation(" S "));
        assert!(!parse_revoke_confirmation("no"));
        assert!(!parse_revoke_confirmation("nao"));
        assert!(!parse_revoke_confirmation(""));
        assert!(!parse_revoke_confirmation("yesplease"));
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
        let (_f, store) = test_store().await;
        let InstallOutcome::Done(report) = install(
            &mut host,
            &store,
            personas_dest.path().to_str().unwrap(),
            "/nonexistent/bastion.toml",
            "alice",
            pack_root.path().to_str().unwrap(),
        )
        .await
        else {
            panic!("pack has no [personas_selection] — must commit immediately")
        };

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

    // ---------------------------------------------------------------------
    // Optional persona selection — parse_personas_selection, parse_persona_selection,
    // install_commit's exact copy set, and backward compatibility.
    // ---------------------------------------------------------------------

    #[test]
    fn parse_personas_selection_reads_the_required_list() {
        let raw = r#"
            id = "acme/hedge-fund-committee"
            personas = ["risk-manager", "portfolio-manager", "burry", "ackman"]

            [personas_selection]
            required = ["risk-manager", "portfolio-manager"]
        "#;
        assert_eq!(
            parse_personas_selection(raw),
            Some(vec![
                "risk-manager".to_string(),
                "portfolio-manager".to_string()
            ])
        );
    }

    #[test]
    fn parse_personas_selection_none_when_table_absent() {
        // None (not Some(vec![])) is load-bearing: it means "every persona
        // is required", not "zero personas required". Valid TOML, table
        // genuinely absent — distinct from the malformed-input test below.
        assert_eq!(
            parse_personas_selection("id = \"acme/thing\"\npersonas = [\"a\", \"b\"]"),
            None
        );
    }

    #[test]
    fn parse_personas_selection_none_on_malformed_toml() {
        assert_eq!(parse_personas_selection("this is not { valid toml"), None);
    }

    fn opt(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parse_persona_selection_all_and_none_and_empty() {
        let optional = opt(&["burry", "ackman", "wood"]);
        assert_eq!(parse_persona_selection("all", &optional).selected, optional);
        assert!(parse_persona_selection("none", &optional)
            .selected
            .is_empty());
        assert!(parse_persona_selection("", &optional).selected.is_empty());
        assert!(parse_persona_selection("   ", &optional)
            .selected
            .is_empty());
    }

    #[test]
    fn parse_persona_selection_by_number_1_indexed() {
        let optional = opt(&["burry", "ackman", "wood"]);
        let result = parse_persona_selection("1, 3", &optional);
        assert_eq!(result.selected, opt(&["burry", "wood"]));
        assert!(result.ignored.is_empty());
    }

    #[test]
    fn parse_persona_selection_by_name_case_insensitive() {
        let optional = opt(&["burry", "ackman", "wood"]);
        let result = parse_persona_selection("BURRY, Wood", &optional);
        assert_eq!(result.selected, opt(&["burry", "wood"]));
        assert!(result.ignored.is_empty());
    }

    #[test]
    fn parse_persona_selection_mixed_numbers_and_names() {
        let optional = opt(&["burry", "ackman", "wood"]);
        let result = parse_persona_selection("1, wood", &optional);
        assert_eq!(result.selected, opt(&["burry", "wood"]));
    }

    #[test]
    fn parse_persona_selection_reports_unrecognized_tokens_without_dropping_valid_ones() {
        let optional = opt(&["burry", "ackman", "wood"]);
        let result = parse_persona_selection("burry, dalio, 99", &optional);
        assert_eq!(result.selected, opt(&["burry"]));
        assert_eq!(result.ignored, vec!["dalio".to_string(), "99".to_string()]);
    }

    #[test]
    fn parse_persona_selection_deduplicates_a_persona_picked_twice() {
        let optional = opt(&["burry", "ackman"]);
        let result = parse_persona_selection("1, burry", &optional);
        assert_eq!(result.selected, opt(&["burry"]));
    }

    fn write_hedge_fund_style_pack(root: &Path) {
        write_pack(
            root,
            r#"
                id = "acme/hedge-fund-committee"
                version = "1.0.0"
                extensions = []
                skills = []
                personas = ["risk-manager", "portfolio-manager", "burry", "ackman"]

                [personas_selection]
                required = ["risk-manager", "portfolio-manager"]

                [defaults]
                enabled_extensions = []
            "#,
            &[
                ("risk-manager", "---\nname: risk-manager\n---\nbody"),
                (
                    "portfolio-manager",
                    "---\nname: portfolio-manager\n---\nbody",
                ),
                ("burry", "---\nname: burry\n---\nbody"),
                ("ackman", "---\nname: ackman\n---\nbody"),
            ],
            &[],
        );
    }

    #[tokio::test]
    async fn install_awaits_persona_selection_when_pack_declares_optional_personas() {
        let pack_root = TempDir::new().unwrap();
        let personas_dest = TempDir::new().unwrap();
        write_hedge_fund_style_pack(pack_root.path());

        let mut host = ExtensionHost::new();
        let (_f, store) = test_store().await;
        let outcome = install(
            &mut host,
            &store,
            personas_dest.path().to_str().unwrap(),
            "/nonexistent/bastion.toml",
            "alice",
            pack_root.path().to_str().unwrap(),
        )
        .await;

        let InstallOutcome::AwaitingPersonaSelection {
            report,
            pack_dir,
            required,
            optional,
        } = outcome
        else {
            panic!(
                "pack declares [personas_selection] with optional personas — must await a reply"
            );
        };
        assert_eq!(pack_dir, pack_root.path());
        assert_eq!(required, opt(&["risk-manager", "portfolio-manager"]));
        assert_eq!(optional, opt(&["burry", "ackman"]));
        assert!(report.contains("1. burry"), "{report}");
        assert!(report.contains("2. ackman"), "{report}");
        // Nothing copied yet — the preview phase must not touch disk.
        assert!(!personas_dest.path().join("risk-manager").exists());
        assert!(!personas_dest.path().join("burry").exists());
    }

    #[tokio::test]
    async fn install_commit_copies_exactly_required_union_selection() {
        let pack_root = TempDir::new().unwrap();
        let personas_dest = TempDir::new().unwrap();
        write_hedge_fund_style_pack(pack_root.path());

        let mut host = ExtensionHost::new();
        let (_f, store) = test_store().await;
        let required = opt(&["risk-manager", "portfolio-manager"]);
        let selection = opt(&["burry"]); // ackman intentionally NOT selected
        let report = install_commit(
            &mut host,
            &store,
            personas_dest.path().to_str().unwrap(),
            "/nonexistent/bastion.toml",
            "alice",
            pack_root.path(),
            &required,
            &selection,
        )
        .await;

        assert!(report.contains("risk-manager"), "{report}");
        assert!(report.contains("portfolio-manager"), "{report}");
        assert!(report.contains("burry"), "{report}");
        assert!(personas_dest.path().join("risk-manager/SOUL.md").exists());
        assert!(personas_dest
            .path()
            .join("portfolio-manager/SOUL.md")
            .exists());
        assert!(personas_dest.path().join("burry/SOUL.md").exists());
        assert!(
            !personas_dest.path().join("ackman").exists(),
            "ackman was never selected — must not be copied"
        );
    }

    #[tokio::test]
    async fn install_commit_with_empty_selection_installs_only_required() {
        let pack_root = TempDir::new().unwrap();
        let personas_dest = TempDir::new().unwrap();
        write_hedge_fund_style_pack(pack_root.path());

        let mut host = ExtensionHost::new();
        let (_f, store) = test_store().await;
        let required = opt(&["risk-manager", "portfolio-manager"]);
        install_commit(
            &mut host,
            &store,
            personas_dest.path().to_str().unwrap(),
            "/nonexistent/bastion.toml",
            "alice",
            pack_root.path(),
            &required,
            &[], // "none" reply
        )
        .await;

        assert!(personas_dest.path().join("risk-manager").exists());
        assert!(!personas_dest.path().join("burry").exists());
        assert!(!personas_dest.path().join("ackman").exists());
    }

    #[tokio::test]
    async fn existing_packs_without_personas_selection_are_unaffected_regression_check() {
        // Hard compatibility requirement: a pack with no
        // [personas_selection] table installs EXACTLY like before this
        // feature existed — Done immediately, every persona installed,
        // never AwaitingPersonaSelection.
        let pack_root = TempDir::new().unwrap();
        let personas_dest = TempDir::new().unwrap();
        write_pack(
            pack_root.path(),
            r#"
                id = "thewaifucorp/software-sdlc"
                version = "1.0.0"
                extensions = []
                skills = []
                personas = ["tech-lead", "implementer"]

                [defaults]
                enabled_extensions = []
            "#,
            &[
                ("tech-lead", "---\nname: tech-lead\n---\nbody"),
                ("implementer", "---\nname: implementer\n---\nbody"),
            ],
            &[],
        );

        let mut host = ExtensionHost::new();
        let (_f, store) = test_store().await;
        let outcome = install(
            &mut host,
            &store,
            personas_dest.path().to_str().unwrap(),
            "/nonexistent/bastion.toml",
            "alice",
            pack_root.path().to_str().unwrap(),
        )
        .await;

        let InstallOutcome::Done(report) = outcome else {
            panic!("a pack with no [personas_selection] must never await a prompt");
        };
        assert!(report.contains("tech-lead"), "{report}");
        assert!(report.contains("implementer"), "{report}");
        assert!(personas_dest.path().join("tech-lead").exists());
        assert!(personas_dest.path().join("implementer").exists());
    }

    // ---------------------------------------------------------------------
    // Persistence across a restart — the actual M4.2 acceptance criterion:
    // an extension installed before a restart reappears already active,
    // without the operator reinstalling it.
    // ---------------------------------------------------------------------

    #[tokio::test]
    async fn a_declarative_extension_survives_a_simulated_restart() {
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

        // "Before the restart": a real install, through the real command
        // path, into a real (temp-file-backed) store.
        let (_f, store) = test_store().await;
        let mut host = ExtensionHost::new();
        install(
            &mut host,
            &store,
            ".",
            "/nonexistent/bastion.toml",
            "alice",
            pack_root.path().to_str().unwrap(),
        )
        .await;
        assert!(host.is_installed("acme/noop-mcp"));

        // "The restart": a FRESH host, exactly what `main.rs` constructs at
        // boot — nothing carried over except what the store persisted.
        let mut fresh_host = ExtensionHost::new();
        assert!(!fresh_host.is_installed("acme/noop-mcp"));

        let lines = reload_persisted(&mut fresh_host, &store).await;

        assert!(
            fresh_host.is_installed("acme/noop-mcp"),
            "a persisted extension must reactivate on the next boot without \
             the operator reinstalling it"
        );
        assert!(
            lines.iter().any(|l| l.contains("acme/noop-mcp")),
            "{lines:?}"
        );
    }

    #[tokio::test]
    async fn revoking_an_extension_removes_it_from_persistence_too() {
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

        let (_f, store) = test_store().await;
        let mut host = ExtensionHost::new();
        install(
            &mut host,
            &store,
            ".",
            "/nonexistent/bastion.toml",
            "alice",
            pack_root.path().to_str().unwrap(),
        )
        .await;

        revoke_commit(&mut host, &store, "acme/noop-mcp").await;

        // A "restart" after the revoke must NOT bring it back.
        let mut fresh_host = ExtensionHost::new();
        reload_persisted(&mut fresh_host, &store).await;
        assert!(!fresh_host.is_installed("acme/noop-mcp"));
    }

    #[tokio::test]
    async fn reload_rejects_a_persisted_row_claiming_more_than_its_kind_is_ever_allowed() {
        // A row with permissions no `Declarative` install could ever have
        // produced (the real install path always uses `PermissionSet::none()`
        // as the ceiling for this kind) — simulates a corrupted row or a
        // future bug elsewhere writing one. Before the fix, `reload_persisted`
        // checked this manifest's `permissions` against ITSELF
        // (`X.is_subset_of(X)`, always true) and would have reactivated it
        // with the escalated authority intact.
        let (_f, store) = test_store().await;
        let mut escalated = manifest_for_persistence_test("acme/escalated");
        escalated.permissions = PermissionSet {
            capabilities: vec!["some-dangerous-capability".to_string()],
            ..PermissionSet::none()
        };
        store
            .save("alice", &escalated, ReconstructKind::Declarative)
            .await
            .unwrap();

        let mut host = ExtensionHost::new();
        let lines = reload_persisted(&mut host, &store).await;

        assert!(
            !host.is_installed("acme/escalated"),
            "a persisted row must never be granted more authority on reload than its \
             ReconstructKind's fixed ceiling allows, even if its own stored \
             `permissions` claims it"
        );
        assert!(
            lines.iter().any(|l| l.contains("acme/escalated")),
            "the rejection must be reported, not silently dropped: {lines:?}"
        );
    }

    #[test]
    fn reload_ceiling_matches_the_live_install_ceiling_for_each_kind() {
        // Pins `reload_ceiling_for` to the exact same ceilings
        // `install_one_extension`/`install_git_capability` pass to
        // `host.install` for a live `/extension install` — the whole point
        // of this function is that reload can never grant more than a live
        // install could have.
        assert_eq!(
            reload_ceiling_for(&ReconstructKind::Declarative),
            PermissionSet::none()
        );
        assert_eq!(
            reload_ceiling_for(&ReconstructKind::GitCapability),
            PermissionSet {
                capabilities: vec!["git".to_string()],
                ..PermissionSet::none()
            }
        );
    }

    #[tokio::test]
    async fn one_corrupt_persisted_row_does_not_block_the_others_from_reloading() {
        let (_f, store) = test_store().await;
        store.init_schema().await.unwrap();

        // A good row, through the real typed API.
        store
            .save(
                "alice",
                &manifest_for_persistence_test("acme/good-one"),
                ReconstructKind::Declarative,
            )
            .await
            .unwrap();

        // A row this build's `ReconstructKind` can't parse — simulates a
        // future format change or a hand-edited db — written directly with
        // rusqlite, bypassing `save`'s typed API on purpose. `_f` is the
        // same temp-file path `store` itself was constructed from.
        {
            use rusqlite::Connection;
            let conn = Connection::open(_f.path()).unwrap();
            conn.execute(
                "INSERT INTO installed_extension (id, owner, kind, manifest_json) \
                 VALUES ('acme/from-the-future', 'alice', 'wasm_v2', '{}')",
                [],
            )
            .unwrap();
        }

        let mut host = ExtensionHost::new();
        let lines = reload_persisted(&mut host, &store).await;

        // The good row reactivated — one bad row never stops the batch.
        assert!(
            host.is_installed("acme/good-one"),
            "a corrupt sibling row must not block a valid row from reloading"
        );
        assert!(!host.is_installed("acme/from-the-future"));
        assert!(lines.iter().any(|l| l.contains("acme/good-one")));
    }

    fn manifest_for_persistence_test(id: &str) -> ExtensionManifest {
        ExtensionManifest {
            id: id.to_string(),
            version: semver::Version::new(1, 0, 0),
            kind: ExtensionKind::Declarative,
            compat: semver::VersionReq::parse("*").unwrap(),
            provides: vec![],
            requires: vec![],
            permissions: PermissionSet::none(),
            secrets: vec![],
            entrypoint: Entrypoint::Declarative {
                artifact_path: "noop.json".into(),
            },
            migrations: vec![],
            signature: None,
        }
    }
}
