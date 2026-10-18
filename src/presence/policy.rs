//! `[[presence.rules]]` — Bastion's decision on a host agent's tool call.
//!
//! First matching rule wins; no match means Bastion has no opinion and the
//! host decides as it would without the pack. `remote` is the one decision
//! that involves the rest of Bastion: the hub opens an approval every surface
//! can answer (see `hub::PresenceHub::open_approval`).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The fields of a tool call kept anywhere in presence, in the order the
/// main input is picked from (`main_input`).
pub const TOOL_INPUT_FIELDS: &[&str] = &[
    "command",
    "file_path",
    "url",
    "pattern",
    "query",
    "description",
];

/// Each kept field is cut to this many characters for display and storage.
pub const TOOL_INPUT_MAX: usize = 200;

/// Rules see the whole field up to this size (never the display cut, or a
/// command padded past 200 characters would slip past a pattern). A larger
/// field is not evaluated at all: the host asks the person
/// (`RuleCheck::TooLong`).
pub const RULE_INPUT_MAX: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleDecision {
    Allow,
    Deny,
    Ask,
    Remote,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PresenceRule {
    /// Glob on the tool name (`*`, `?`): `"Bash"`, `"mcp__*"`, `"*"`.
    pub tool: String,
    /// Optional glob on the call's main input (`main_input`). Absent matches
    /// every input, including a call with none.
    #[serde(default)]
    pub input: Option<String>,
    pub decision: RuleDecision,
}

/// The first rule matching `tool` and `input`.
pub fn evaluate<'a>(
    rules: &'a [PresenceRule],
    tool: &str,
    input: Option<&str>,
) -> Option<&'a PresenceRule> {
    rules.iter().find(|rule| {
        glob_match(&rule.tool, tool)
            && match (&rule.input, input) {
                (None, _) => true,
                (Some(pattern), Some(text)) => glob_match(pattern, text),
                (Some(_), None) => false,
            }
    })
}

/// Only the whitelisted fields of a tool call, each cut to
/// [`TOOL_INPUT_MAX`]. `None` when nothing is left.
pub fn sanitize_tool_input(input: Option<&Value>) -> Option<Value> {
    select_tool_input(input, TOOL_INPUT_MAX)
}

/// The whitelisted fields, each cut to `max` characters.
pub fn select_tool_input(input: Option<&Value>, max: usize) -> Option<Value> {
    let object = input?.as_object()?;
    let mut out = serde_json::Map::new();
    for field in TOOL_INPUT_FIELDS {
        if let Some(text) = object.get(*field).and_then(Value::as_str) {
            if !text.is_empty() {
                out.insert((*field).to_string(), Value::String(cut(text, max)));
            }
        }
    }
    (!out.is_empty()).then_some(Value::Object(out))
}

/// The main input as a rule must see it.
#[derive(Debug, PartialEq, Eq)]
pub enum RuleInput {
    None,
    Full(String),
    /// Longer than [`RULE_INPUT_MAX`]: not evaluated.
    TooLong,
}

/// The untruncated main input, for rule evaluation only.
pub fn rule_input(input: Option<&Value>) -> RuleInput {
    let Some(object) = input.and_then(Value::as_object) else {
        return RuleInput::None;
    };
    let text = TOOL_INPUT_FIELDS.iter().find_map(|field| {
        object
            .get(*field)
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
    });
    match text {
        None => RuleInput::None,
        Some(text) if text.chars().count() >= RULE_INPUT_MAX => RuleInput::TooLong,
        Some(text) => RuleInput::Full(text.to_string()),
    }
}

/// The one field a rule and a surface look at: the command of a shell call,
/// the path of a file call, the URL of a fetch, and so on.
pub fn main_input(input: Option<&Value>) -> Option<String> {
    let object = input?.as_object()?;
    TOOL_INPUT_FIELDS.iter().find_map(|field| {
        object
            .get(*field)
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(|text| cut(text, TOOL_INPUT_MAX))
    })
}

/// `*` matches any run of characters (empty included), `?` exactly one;
/// everything else literally. Case-sensitive, like tool names.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, ti));
            pi += 1;
        } else if let Some((star_p, star_t)) = star {
            pi = star_p + 1;
            ti = star_t + 1;
            star = Some((star_p, star_t + 1));
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// At most `max` characters, on a character boundary, with `…` when cut.
pub fn cut(text: &str, max: usize) -> String {
    let mut chars = text.chars();
    let head: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() && max > 0 {
        let mut shorter: String = head.chars().take(max - 1).collect();
        shorter.push('…');
        shorter
    } else {
        head
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rule(tool: &str, input: Option<&str>, decision: RuleDecision) -> PresenceRule {
        PresenceRule {
            tool: tool.into(),
            input: input.map(Into::into),
            decision,
        }
    }

    #[test]
    fn glob_handles_stars_and_question_marks() {
        assert!(glob_match("*", ""));
        assert!(glob_match("mcp__*", "mcp__github__create_issue"));
        assert!(glob_match("git push*", "git push origin main"));
        assert!(glob_match("*rm -rf*", "sudo rm -rf /tmp/x"));
        assert!(glob_match("Ba?h", "Bash"));
        assert!(!glob_match("Bash", "bash"));
        assert!(!glob_match("git push*", "git pull"));
        assert!(!glob_match("a*b", "a"));
    }

    #[test]
    fn first_matching_rule_wins_and_input_patterns_need_an_input() {
        let rules = vec![
            rule("Bash", Some("git push*"), RuleDecision::Remote),
            rule("Bash", Some("*rm -rf /*"), RuleDecision::Deny),
            rule("Read", None, RuleDecision::Allow),
        ];
        let decision =
            |tool: &str, input: Option<&str>| evaluate(&rules, tool, input).map(|r| r.decision);
        assert_eq!(
            decision("Bash", Some("git push origin main")),
            Some(RuleDecision::Remote)
        );
        assert_eq!(decision("Bash", Some("rm -rf /")), Some(RuleDecision::Deny));
        assert_eq!(decision("Bash", Some("ls")), None);
        assert_eq!(decision("Bash", None), None);
        assert_eq!(decision("Read", None), Some(RuleDecision::Allow));
        assert_eq!(decision("Write", Some("x")), None);
    }

    #[test]
    fn rules_deserialize_from_toml() {
        #[derive(Deserialize)]
        struct Wrapper {
            rules: Vec<PresenceRule>,
        }
        let parsed: Wrapper = toml::from_str(
            r#"
            [[rules]]
            tool = "Bash"
            input = "git push*"
            decision = "remote"
            "#,
        )
        .expect("rules parse");
        assert_eq!(parsed.rules[0].decision, RuleDecision::Remote);
    }

    #[test]
    fn tool_input_keeps_only_whitelisted_fields_cut_to_size() {
        let long = "x".repeat(500);
        let input = json!({ "command": long, "content": "secret file body", "timeout": 5 });
        let kept = sanitize_tool_input(Some(&input)).expect("command kept");
        let object = kept.as_object().unwrap();
        assert_eq!(object.len(), 1);
        assert_eq!(
            object["command"].as_str().unwrap().chars().count(),
            TOOL_INPUT_MAX
        );
        assert!(sanitize_tool_input(Some(&json!({ "content": "x" }))).is_none());
        assert_eq!(
            main_input(Some(&json!({ "file_path": "/a/b.rs", "description": "d" }))),
            Some("/a/b.rs".into())
        );
    }

    #[test]
    fn rules_see_the_whole_input_not_the_display_cut() {
        let padded = format!("{}; rm -rf /", "x".repeat(300));
        let rules = vec![rule("Bash", Some("*rm -rf /*"), RuleDecision::Deny)];
        let input = json!({ "command": padded });
        let RuleInput::Full(full) = rule_input(Some(&input)) else {
            panic!("full input expected");
        };
        assert_eq!(
            evaluate(&rules, "Bash", Some(&full)).map(|r| r.decision),
            Some(RuleDecision::Deny)
        );
        // The display cut alone would have missed it.
        let shown = main_input(Some(&input)).unwrap();
        assert!(evaluate(&rules, "Bash", Some(&shown)).is_none());
        let huge = json!({ "command": "y".repeat(RULE_INPUT_MAX) });
        assert_eq!(rule_input(Some(&huge)), RuleInput::TooLong);
        assert_eq!(rule_input(None), RuleInput::None);
    }

    #[test]
    fn cut_respects_char_boundaries() {
        assert_eq!(cut("ação", 10), "ação");
        assert_eq!(cut("ação rápida", 4), "açã…");
        assert_eq!(cut("", 3), "");
    }
}
