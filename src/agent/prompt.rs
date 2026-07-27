//! Generic interactive-prompt primitive for the daemon's stdin REPL.
//!
//! Confirmed before building this: there is NO interactive/confirm/modal
//! mechanism anywhere in the codebase today — the stdin REPL
//! (`main.rs`'s `tokio::select!` loop) processes one line per iteration and
//! every command handler returns a single string; the ratatui TUI (`tui.rs`)
//! is event-driven over SSE with no request/response channel for this at
//! all, and `/extension` isn't even wired into it (`ConsoleOnly`).
//!
//! Scope decision (agreed before implementation): build ONLY this generic
//! primitive — a command handler that already owns exclusive access to the
//! REPL's stdin `Lines` stream (the same way `/backend`/`/task`/`/credential`/
//! `/extension` are already special-cased in `main.rs`, ahead of the generic
//! `CommandHandler` port, precisely so they can take extra arguments a
//! trait-object port can't carry) can borrow it here to ask a follow-up
//! question mid-command. NOT wired to persona selection yet:
//! `bastion-extension-protocol`'s `PackManifest.personas` is a flat
//! `Vec<String>` (every listed persona always installs) — there is no
//! "optional persona" concept to choose between in ANY pack.toml today, and
//! adding one is a `bastion-core` change (a different repo, pinned by rev),
//! out of scope for this task. See `extension_command.rs::install`'s own
//! comment for the exact future hook.
//!
//! Generic over `AsyncBufRead` (not hardcoded to `tokio::io::Stdin`) so tests
//! exercise it against an in-memory reader instead of real stdin.

use tokio::io::{AsyncBufRead, Lines};

/// Ask a yes/no question, reading one line from `lines`. Anything other than
/// `y`/`yes` (case-insensitive) — including EOF (a closed/non-interactive
/// stdin) or an empty line — answers `false`: a prompt fails closed, never
/// open, matching this codebase's fail-closed default elsewhere (egress,
/// permission gates, tool authority).
pub async fn ask_confirm<R: AsyncBufRead + Unpin>(
    lines: &mut Lines<R>,
    question: &str,
) -> anyhow::Result<bool> {
    println!("{question} [y/N]");
    match lines.next_line().await? {
        Some(line) => Ok(matches!(
            line.trim().to_ascii_lowercase().as_str(),
            "y" | "yes"
        )),
        None => Ok(false),
    }
}

/// Ask the operator to pick zero or more of `options` (1-indexed in the
/// printed prompt). Reads one line: `all`, `none`/empty, or comma-separated
/// indices (`1,3`). EOF (closed stdin) answers "none" — same fail-closed
/// default as [`ask_confirm`], never silently picks "all".
pub async fn ask_choice<R: AsyncBufRead + Unpin>(
    lines: &mut Lines<R>,
    question: &str,
    options: &[String],
) -> anyhow::Result<Vec<String>> {
    println!("{question}");
    for (i, opt) in options.iter().enumerate() {
        println!("  {}. {opt}", i + 1);
    }
    println!("Enter numbers separated by commas, 'all', or 'none':");
    let Some(line) = lines.next_line().await? else {
        return Ok(Vec::new());
    };
    let line = line.trim();
    if line.is_empty() || line.eq_ignore_ascii_case("none") {
        return Ok(Vec::new());
    }
    if line.eq_ignore_ascii_case("all") {
        return Ok(options.to_vec());
    }
    let mut chosen = Vec::with_capacity(options.len());
    for part in line.split(',') {
        let part = part.trim();
        let idx: usize = part
            .parse()
            .map_err(|_| anyhow::anyhow!("'{part}' is not a number (or 'all'/'none')"))?;
        let Some(opt) = idx.checked_sub(1).and_then(|i| options.get(i)) else {
            anyhow::bail!("{idx} is out of range — choices are 1..={}", options.len());
        };
        chosen.push(opt.clone());
    }
    Ok(chosen)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, BufReader};

    fn lines_of(input: &'static str) -> Lines<BufReader<&'static [u8]>> {
        BufReader::new(input.as_bytes()).lines()
    }

    #[tokio::test]
    async fn ask_confirm_accepts_y_and_yes_case_insensitive() {
        for input in ["y\n", "Y\n", "yes\n", "YES\n", "Yes\n"] {
            let mut lines = lines_of(input);
            assert!(
                ask_confirm(&mut lines, "continue?").await.unwrap(),
                "input {input:?} must confirm"
            );
        }
    }

    #[tokio::test]
    async fn ask_confirm_rejects_anything_else_including_empty_and_eof() {
        for input in ["n\n", "no\n", "\n", "sure\n"] {
            let mut lines = lines_of(input);
            assert!(!ask_confirm(&mut lines, "continue?").await.unwrap());
        }
        let mut eof = lines_of("");
        assert!(
            !ask_confirm(&mut eof, "continue?").await.unwrap(),
            "EOF (closed stdin) must fail closed, not panic or hang"
        );
    }

    fn opts(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[tokio::test]
    async fn ask_choice_all_selects_every_option() {
        let mut lines = lines_of("all\n");
        let options = opts(&["a", "b", "c"]);
        assert_eq!(
            ask_choice(&mut lines, "pick", &options).await.unwrap(),
            options
        );
    }

    #[tokio::test]
    async fn ask_choice_none_and_empty_and_eof_select_nothing() {
        for input in ["none\n", "\n"] {
            let mut lines = lines_of(input);
            let options = opts(&["a", "b"]);
            assert!(ask_choice(&mut lines, "pick", &options)
                .await
                .unwrap()
                .is_empty());
        }
        let mut eof = lines_of("");
        assert!(ask_choice(&mut eof, "pick", &opts(&["a"]))
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn ask_choice_parses_comma_separated_1_indexed_selection() {
        let mut lines = lines_of("1, 3\n");
        let options = opts(&["a", "b", "c"]);
        assert_eq!(
            ask_choice(&mut lines, "pick", &options).await.unwrap(),
            vec!["a".to_string(), "c".to_string()]
        );
    }

    #[tokio::test]
    async fn ask_choice_rejects_out_of_range_index() {
        let mut lines = lines_of("5\n");
        let err = ask_choice(&mut lines, "pick", &opts(&["a", "b"]))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("out of range"));
    }

    #[tokio::test]
    async fn ask_choice_rejects_non_numeric_garbage() {
        let mut lines = lines_of("banana\n");
        let err = ask_choice(&mut lines, "pick", &opts(&["a"]))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not a number"));
    }
}
