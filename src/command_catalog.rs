//! Fase 3.1 (`docs/revamp` plan `lexical-orbiting-hoare.md` — "UX CLI/TUI"):
//! single source of truth for every real Bastion slash command.
//!
//! Before this module, the same information was duplicated in THREE places
//! that had already drifted apart: `agent::command::KNOWN_COMMANDS` (which
//! commands the daemon even recognizes), `main.rs`'s local
//! `REMOTE_ALLOWED_COMMANDS` const (which of those are reachable over
//! webhook/Telegram), and `tui.rs`'s `COMMANDS` table (which ones the local
//! autocomplete offers, with its own independent `remote: bool` per entry
//! that had already gone stale for `/model`/`/connect`/`/backend`). This
//! module replaces all three: one `CATALOG`, with the derived views below
//! doing exactly what each old list did, so they can never disagree again.
//!
//! Scope legend:
//! - `Remote` — reachable from the console AND over webhook/Telegram
//!   (`main.rs`'s inbound_rx arm allowlists these).
//! - `ConsoleOnly` — a real daemon command, but only ever dispatched from the
//!   local stdin console (never over a channel) — the risk of each one is
//!   documented at its original allowlist decision point in `main.rs`, not
//!   relitigated here.
//! - `TuiLocal` — never reaches the daemon at all; `tui.rs` answers these
//!   entirely client-side (`/pet`, `/theme`) before a turn is ever sent.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scope {
    /// Local TUI only — intercepted client-side, never sent to the daemon.
    TuiLocal,
    /// Reachable from the console AND over webhook/Telegram.
    Remote,
    /// A real daemon command, but console (stdin) only.
    ConsoleOnly,
}

pub struct CommandSpec {
    pub name: &'static str,
    pub usage: &'static str,
    pub desc: &'static str,
    pub scope: Scope,
    /// Alternate spellings that route to the exact same behavior (today only
    /// `/model`/`/models` and `/backend`/`/backends`). Aliases are full,
    /// independent entries in `CATALOG` too (so autocomplete/prefix-matching
    /// treats them like any other command) — this field exists for the
    /// handful of callers that need to resolve "is this name a synonym of
    /// that other one" (`help_text`'s alias-line suppression,
    /// `closest_command`'s candidate set).
    pub aliases: &'static [&'static str],
}

/// The true set of Bastion slash commands, mirrored from `src/agent/command.rs`'s
/// former `KNOWN_COMMANDS` (daemon-reachable commands) plus `src/tui.rs`'s
/// former `COMMANDS` table (`/pet`, `/theme` — never reach the daemon).
pub const CATALOG: &[CommandSpec] = &[
    CommandSpec {
        name: "/pet",
        usage: "/pet <action>",
        desc: "care for and configure the companion — type space to see actions",
        scope: Scope::TuiLocal,
        aliases: &[],
    },
    CommandSpec {
        name: "/theme",
        usage: "/theme <nome|#RRGGBB>",
        desc: "switch the TUI colors instantly — type space to see themes",
        scope: Scope::TuiLocal,
        aliases: &[],
    },
    CommandSpec {
        name: "/help",
        usage: "/help",
        desc: "show this help",
        scope: Scope::Remote,
        aliases: &[],
    },
    CommandSpec {
        name: "/contest",
        usage: "/contest <id>",
        desc: "revoke a belief by ID (D-14)",
        scope: Scope::Remote,
        aliases: &[],
    },
    CommandSpec {
        name: "/task",
        usage:
            "/task list | inspect <id> | pause <id> | resume <id> | steer <id> <text> | cancel <id>",
        desc: "inspect and control durable Pursue tasks (owner-scoped)",
        scope: Scope::Remote,
        aliases: &[],
    },
    CommandSpec {
        name: "/schedule",
        usage:
            "/schedule list | add every <secs> <intent> | add once <secs> <intent> | cancel <id>",
        desc: "schedule an authorized intent to fire once or on a recurrence (owner-scoped)",
        scope: Scope::Remote,
        aliases: &[],
    },
    CommandSpec {
        name: "/model",
        usage: "/model <name>",
        desc: "show/switch/reset the LLM provider+model — type space to browse",
        scope: Scope::Remote,
        aliases: &[],
    },
    CommandSpec {
        name: "/connect",
        usage: "/connect <provider>",
        desc: "show secure provider setup steps / live subscription status",
        scope: Scope::Remote,
        aliases: &[],
    },
    CommandSpec {
        name: "/backend",
        usage: "/backend [use <id>]",
        desc: "list/switch the conversation backend — model or a subscription runtime",
        scope: Scope::Remote,
        aliases: &[],
    },
    CommandSpec {
        name: "/logs",
        usage: "/logs",
        desc: "recent daemon ERROR/WARN log entries (timestamp/level/message only)",
        scope: Scope::Remote,
        aliases: &[],
    },
    CommandSpec {
        name: "/update",
        usage: "/update [status|apply]",
        desc: "show release status or request an explicit host update",
        scope: Scope::Remote,
        aliases: &[],
    },
    CommandSpec {
        name: "/as",
        usage: "/as <persona>",
        desc: "force a persona for the next turn — daemon-wide state",
        scope: Scope::ConsoleOnly,
        aliases: &[],
    },
    CommandSpec {
        name: "/cabinet",
        usage: "/cabinet [personas..]",
        desc: "convene the Cabinet with named personas",
        scope: Scope::ConsoleOnly,
        aliases: &[],
    },
    CommandSpec {
        name: "/credential",
        usage: "/credential list | issue <label> [scopes] | revoke <id>",
        desc: "issue/revoke Control Plane bearer credentials (token shown once, console only)",
        scope: Scope::ConsoleOnly,
        aliases: &[],
    },
    CommandSpec {
        name: "/stop",
        usage: "/stop",
        desc: "shut down the daemon",
        scope: Scope::ConsoleOnly,
        aliases: &[],
    },
    CommandSpec {
        name: "/connect-app",
        usage: "/connect-app <device>",
        desc: "pair a new device (one-time code for POST /auth/exchange)",
        scope: Scope::ConsoleOnly,
        aliases: &[],
    },
    CommandSpec {
        name: "/connect-app-composio",
        usage: "/connect-app-composio <toolkit>",
        desc: "start a Composio OAuth connection (SEC-03)",
        scope: Scope::ConsoleOnly,
        aliases: &[],
    },
];

/// Every command name the DAEMON's dispatch layer recognizes — replaces
/// `agent::command::KNOWN_COMMANDS`. Deliberately excludes `TuiLocal` names
/// (`/pet`, `/theme`): those never reach `handle_command` at all, `tui.rs`
/// answers them client-side.
pub fn known_daemon_commands() -> Vec<&'static str> {
    CATALOG
        .iter()
        .filter(|spec| spec.scope != Scope::TuiLocal)
        .map(|spec| spec.name)
        .collect()
}

/// True if `cmd` (the first whitespace-delimited token, e.g. `"/model"`) is a
/// real daemon command (`Remote` or `ConsoleOnly` scope) — replaces
/// `KNOWN_COMMANDS.contains(&c)`.
pub fn is_known(cmd: &str) -> bool {
    known_daemon_commands().into_iter().any(|name| name == cmd)
}

/// True if `cmd` is allowed over a channel (webhook/Telegram) — replaces
/// `main.rs`'s local `REMOTE_ALLOWED_COMMANDS` const. Fase 3.1 promotes
/// `/logs` into this set (`read_recent_log_errors`'s contract is already
/// timestamp/level/message only — see its rustdoc — so it was already safe,
/// just not wired up).
pub fn is_remote_allowed(cmd: &str) -> bool {
    CATALOG
        .iter()
        .any(|spec| spec.scope == Scope::Remote && spec.name == cmd)
}

/// The scope of `cmd`, if it names a real command (any scope, including
/// `TuiLocal`) — used by `tui.rs` to derive the suggestion-panel tag and to
/// answer `ConsoleOnly` commands honestly without a network round trip.
pub fn scope_of(cmd: &str) -> Option<Scope> {
    CATALOG
        .iter()
        .find(|spec| spec.name == cmd)
        .map(|spec| spec.scope)
}

/// True if `name` is only ever listed as a synonym of some OTHER entry
/// (today: `/models` of `/model`, `/backends` of `/backend`) — used by
/// `help_text` to print one merged line per command instead of a redundant
/// alias line.
fn is_pure_alias(spec: &CommandSpec) -> bool {
    CATALOG
        .iter()
        .any(|other| other.name != spec.name && other.aliases.contains(&spec.name))
}

/// Replaces the hardcoded `/help` block (former `command.rs:411-424`).
/// `remote_caller: true` renders only what a webhook/Telegram caller can
/// actually invoke (`Remote` scope) — an honest subset instead of listing
/// commands that would just bounce with "console-only — not allowed
/// remotely". `remote_caller: false` (the console's own `/help`) lists
/// everything, labeling `ConsoleOnly` and `TuiLocal` commands so the console
/// operator knows their real scope instead of guessing.
pub fn help_text(remote_caller: bool) -> String {
    let mut lines = vec!["Available commands:".to_string()];
    for spec in CATALOG {
        if is_pure_alias(spec) {
            continue;
        }
        if remote_caller && spec.scope != Scope::Remote {
            continue;
        }
        let alias_note = spec
            .aliases
            .first()
            .map(|a| format!(" (alias: {a})"))
            .unwrap_or_default();
        let scope_label = match spec.scope {
            Scope::Remote => "",
            Scope::ConsoleOnly => " (console only)",
            Scope::TuiLocal => " (local TUI)",
        };
        lines.push(format!(
            " {:<24} {}{}{}",
            spec.usage, spec.desc, alias_note, scope_label
        ));
    }
    lines.join("\n")
}

/// Hand-rolled ordered-subsequence check (Fase 3.3): true if every char of
/// `needle` appears in `haystack`, in order, not necessarily contiguous
/// (`/mdl` matches `/model`). Case-insensitive. This is the fallback tier —
/// prefix matching is tried first by callers, subsequence only when the
/// prefix tier comes back empty.
pub fn subsequence_match(needle: &str, haystack: &str) -> bool {
    let mut hay = haystack.chars().flat_map(char::to_lowercase);
    needle
        .chars()
        .flat_map(char::to_lowercase)
        .all(|n| hay.by_ref().any(|h| h == n))
}

/// Fase 3.3 "did you mean": the closest command name/alias to `input` by
/// Jaro-Winkler similarity, if any candidate scores >= 0.75. Considers every
/// name and alias in `CATALOG` regardless of scope — this is purely an
/// informational hint, not a dispatch decision.
pub fn closest_command(input: &str) -> Option<&'static str> {
    let input_lower = input.to_lowercase();
    let mut best: Option<(&'static str, f64)> = None;
    for spec in CATALOG {
        for candidate in std::iter::once(spec.name).chain(spec.aliases.iter().copied()) {
            let score = strsim::jaro_winkler(&input_lower, &candidate.to_lowercase());
            if score >= 0.75 && best.is_none_or(|(_, best_score)| score > best_score) {
                best = Some((candidate, score));
            }
        }
    }
    best.map(|(name, _)| name)
}

/// A trailing `" Did you mean {x}?"` hint for an unknown-command message, or
/// an empty string when nothing scores high enough to suggest.
pub fn did_you_mean_suffix(input: &str) -> String {
    closest_command(input)
        .map(|name| format!(" Did you mean {name}?"))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const OLD_KNOWN_COMMANDS: &[&str] = &[
        "/connect-app",
        "/connect-app-composio",
        "/connect",
        "/model",
        "/backend",
        "/stop",
        "/as",
        "/cabinet",
        "/contest",
        "/logs",
        "/update",
        "/help",
        // Adaptive Execution (US-202): durable-task cockpit.
        "/task",
        // Adaptive Execution (US-205): personal scheduler cockpit.
        "/schedule",
        // Observability frontend: Control Plane credential cockpit.
        "/credential",
    ];

    const OLD_REMOTE_ALLOWED: &[&str] = &[
        "/help", "/contest", "/connect", "/model", "/backend", "/update",
    ];

    #[test]
    fn known_daemon_commands_matches_old_known_commands_exactly() {
        let known = known_daemon_commands();
        assert_eq!(known.len(), OLD_KNOWN_COMMANDS.len());
        for name in OLD_KNOWN_COMMANDS {
            assert!(
                known.contains(name),
                "missing from known_daemon_commands: {name}"
            );
        }
        for name in &known {
            assert!(
                is_known(name),
                "is_known must agree with known_daemon_commands: {name}"
            );
        }
        // TuiLocal commands never reach the daemon.
        assert!(!known.contains(&"/pet"));
        assert!(!known.contains(&"/theme"));
    }

    #[test]
    fn is_remote_allowed_is_the_old_set_plus_logs() {
        for name in OLD_REMOTE_ALLOWED {
            assert!(is_remote_allowed(name), "must stay remote-allowed: {name}");
        }
        // Fase 3.1 promotion.
        assert!(
            is_remote_allowed("/logs"),
            "/logs must be promoted to Remote"
        );
        // Console-only and TUI-local commands must never be remote-allowed.
        for name in [
            "/as",
            "/cabinet",
            "/stop",
            "/connect-app",
            "/connect-app-composio",
            "/pet",
            "/theme",
        ] {
            assert!(
                !is_remote_allowed(name),
                "{name} must not be remote-allowed"
            );
        }
    }

    #[test]
    fn scope_of_covers_every_catalog_entry() {
        for spec in CATALOG {
            assert_eq!(scope_of(spec.name), Some(spec.scope));
        }
        assert_eq!(scope_of("/does-not-exist"), None);
    }

    #[test]
    fn subsequence_match_finds_ordered_non_contiguous_chars() {
        assert!(subsequence_match("/mdl", "/model"));
        assert!(subsequence_match("/bkd", "/backend"));
        assert!(!subsequence_match("/xyz", "/model"));
        // Order matters — reversed letters must not match.
        assert!(!subsequence_match("ledom", "/model"));
    }

    #[test]
    fn closest_command_suggests_model_for_a_typo() {
        assert_eq!(closest_command("/modle"), Some("/model"));
    }

    #[test]
    fn did_you_mean_suffix_empty_for_nothing_close() {
        assert_eq!(did_you_mean_suffix("/zzzzzzzzzz"), "");
        assert!(did_you_mean_suffix("/modle").contains("/model"));
    }

    #[test]
    fn help_text_remote_excludes_console_only_and_tui_local() {
        let text = help_text(true);
        assert!(text.contains("/model"));
        assert!(text.contains("/backend"));
        assert!(text.contains("/logs"));
        assert!(!text.contains("/stop"));
        assert!(!text.contains("/as "));
        assert!(!text.contains("/pet"));
        assert!(!text.contains("/theme"));
        // Alias lines are suppressed even in the remote view.
        assert!(!text.contains("alias de"));
    }

    #[test]
    fn help_text_console_includes_everything_with_honest_labels() {
        let text = help_text(false);
        assert!(text.contains("/stop"));
        assert!(text.contains("(console only)"));
        assert!(text.contains("/pet"));
        assert!(text.contains("/theme"));
        assert!(text.contains("(local TUI)"));
        assert!(text.contains("/model"));
        assert!(text.contains("/backend"));
        // The plural aliases were dropped entirely — only /model and /backend.
        assert!(!text.contains("/models"));
        assert!(!text.contains("/backends"));
    }
}
