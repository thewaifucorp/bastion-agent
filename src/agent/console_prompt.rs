//! Extensible "the console asked something, this line answers it" state for
//! the daemon's stdin REPL (`main.rs`'s `tokio::select!` loop).
//!
//! Backlog: "mecanismo de prompt interativo no console (+ seletor de persona
//! no /extension install)". Confirmed before building this (see
//! `src/agent/prompt.rs`'s own module doc): the REPL processes one line per
//! loop iteration, every command handler returns a single string, and there
//! was no pending-operation state anywhere — every existing command
//! (`/task`, `/credential`, `/extension`, `/proposal`, `/schedule`,
//! `/backend`) is single-shot.
//!
//! [`PendingConsolePrompt`] is the loop-level state that changes that: the
//! REPL now holds `Option<PendingConsolePrompt>` (`main.rs`, declared beside
//! `stdin_open`/`sigterm`), checked at the top of the stdin arm, BEFORE the
//! empty-line and `/`-dispatch checks — a line answering a pending prompt is
//! never treated as a new command or a chat message.
//!
//! Deliberately an enum, not a single hardcoded shape: a second, unrelated
//! future console command that wants to ask something can add its own
//! variant without the REPL loop's state type or dispatch site changing
//! shape — that's the "general, reusable mechanism" part of the request
//! (Mario: "é o que outros agentes... fazem... temos que melhorar nosso
//! mecanismo"). First (and so far only) consumer: `/extension install`'s
//! persona-selection menu (`extension_command.rs`).

use std::path::PathBuf;

use crate::agent::extension_command::{install_commit, parse_persona_selection};
use crate::extension::ExtensionHost;

/// One console command's mid-flight question, alive between two REPL loop
/// iterations.
pub enum PendingConsolePrompt {
    ExtensionInstallPersonaSelection {
        pack_dir: PathBuf,
        required: Vec<String>,
        optional: Vec<String>,
    },
}

/// Resolve a pending prompt against the operator's reply line. A free
/// function (not inlined in `main.rs`'s loop) specifically so the
/// variant-to-handler dispatch is unit-testable on its own, even though the
/// REPL `select!` wiring around it isn't (see `main.rs`'s own comment at the
/// call site).
pub async fn resolve(
    prompt: PendingConsolePrompt,
    reply: &str,
    host: &mut ExtensionHost,
    personas_dir: &str,
    bastion_toml_path: &str,
    owner: &str,
) -> String {
    match prompt {
        PendingConsolePrompt::ExtensionInstallPersonaSelection {
            pack_dir,
            required,
            optional,
        } => {
            let result = parse_persona_selection(reply, &optional);
            let mut report = install_commit(
                host,
                personas_dir,
                bastion_toml_path,
                owner,
                &pack_dir,
                &required,
                &result.selected,
            )
            .await;
            if !result.ignored.is_empty() {
                report.push_str(&format!(
                    "\n  ! ignored (not a valid number or persona name): {}",
                    result.ignored.join(", ")
                ));
            }
            report
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_pack(root: &std::path::Path) {
        std::fs::write(
            root.join("pack.toml"),
            r#"
                id = "acme/hedge-fund-committee"
                version = "1.0.0"
                extensions = []
                skills = []
                personas = ["risk-manager", "burry", "ackman"]

                [personas_selection]
                required = ["risk-manager"]

                [defaults]
                enabled_extensions = []
            "#,
        )
        .unwrap();
        for name in ["risk-manager", "burry", "ackman"] {
            let dir = root.join("personas").join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("SOUL.md"), format!("---\nname: {name}\n---\nbody")).unwrap();
        }
    }

    #[tokio::test]
    async fn resolve_extension_install_commits_the_selected_personas() {
        let pack_root = TempDir::new().unwrap();
        let personas_dest = TempDir::new().unwrap();
        write_pack(pack_root.path());

        let prompt = PendingConsolePrompt::ExtensionInstallPersonaSelection {
            pack_dir: pack_root.path().to_path_buf(),
            required: vec!["risk-manager".to_string()],
            optional: vec!["burry".to_string(), "ackman".to_string()],
        };

        let mut host = ExtensionHost::new();
        let report = resolve(
            prompt,
            "burry",
            &mut host,
            personas_dest.path().to_str().unwrap(),
            "/nonexistent/bastion.toml",
            "alice",
        )
        .await;

        assert!(report.contains("risk-manager"), "{report}");
        assert!(report.contains("burry"), "{report}");
        assert!(personas_dest.path().join("risk-manager").exists());
        assert!(personas_dest.path().join("burry").exists());
        assert!(
            !personas_dest.path().join("ackman").exists(),
            "ackman was never selected"
        );
    }

    #[tokio::test]
    async fn resolve_extension_install_reports_ignored_tokens() {
        let pack_root = TempDir::new().unwrap();
        let personas_dest = TempDir::new().unwrap();
        write_pack(pack_root.path());

        let prompt = PendingConsolePrompt::ExtensionInstallPersonaSelection {
            pack_dir: pack_root.path().to_path_buf(),
            required: vec!["risk-manager".to_string()],
            optional: vec!["burry".to_string(), "ackman".to_string()],
        };

        let mut host = ExtensionHost::new();
        let report = resolve(
            prompt,
            "burry, dalio",
            &mut host,
            personas_dest.path().to_str().unwrap(),
            "/nonexistent/bastion.toml",
            "alice",
        )
        .await;

        assert!(report.contains("burry"), "{report}");
        assert!(
            report.contains("ignored") && report.contains("dalio"),
            "{report}"
        );
        assert!(personas_dest.path().join("burry").exists());
    }
}
