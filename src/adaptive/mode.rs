//! US-201 — deterministic selection of the smallest capable execution mode.
//!
//! The plan is explicit: pick `Respond`/`Act`/`Pursue` on the turn's existing
//! first inference using deterministic heuristics, and NEVER spend a separate
//! LLM classifier call. So this is a pure function over the raw request text:
//!
//! - **Respond** — the default: answer from knowledge/beliefs, no side effect.
//! - **Act** — one bounded side effect, no continuity beyond the turn.
//! - **Pursue** — multiple dependent effects, out-of-turn duration, recurrence,
//!   decomposition or adaptation (the only mode that gets a durable `TaskCase`).
//!
//! Explicit overrides (the user naming the mode) always win over heuristics.
//! An `Act` classification can still be promoted to `Pursue` later by the
//! runtime when durability/branching actually appears — this only picks the
//! entry mode cheaply.
//!
//! The exact heuristic thresholds are an intentionally-open parameter in the
//! plan (§2.3); these are the initial defaults, kept deliberately conservative
//! (bias toward the cheaper mode) so simple messages never pay for a durable
//! lifecycle.

use bastion_runtime::task::ExecutionMode;

/// Where a mode decision came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeSource {
    /// The user explicitly named the mode ("só responda", "persiga até concluir").
    Override,
    /// Inferred from request features.
    Heuristic,
}

/// The chosen mode plus why — surfaced so the user can inspect and override
/// the choice (US-201 acceptance).
#[derive(Debug, Clone)]
pub struct ModeDecision {
    pub mode: ExecutionMode,
    pub source: ModeSource,
    pub reason: &'static str,
}

/// Phrases that force a specific mode (checked case-insensitively, PT + EN).
const RESPOND_OVERRIDES: &[&str] = &[
    "só responda",
    "apenas responda",
    "só me responda",
    "responda apenas",
    "just answer",
    "just respond",
    "only answer",
];
const ACT_OVERRIDES: &[&str] = &[
    "só faça",
    "apenas faça",
    "faça isso",
    "just do it",
    "do it now",
    "just run",
];
const PURSUE_OVERRIDES: &[&str] = &[
    "persiga até concluir",
    "persiga",
    "continue até",
    "até terminar",
    "até concluir",
    "keep going until",
    "pursue until",
    "see it through",
    "don't stop until",
];

/// Cues that a request spans multiple dependent steps, recurs, or runs beyond
/// the turn — i.e. warrants a durable `Pursue`.
const PURSUE_CUES: &[&str] = &[
    "todo dia",
    "toda vez",
    "a cada",
    "monitore",
    "monitorar",
    "acompanhe",
    "fique de olho",
    "continuamente",
    "every day",
    "each time",
    "monitor",
    "keep track",
    "until it",
    "build an app",
    "build me an app",
    "crie um app",
    "construa um app",
    "e depois",
    "and then",
    "step by step",
    "várias etapas",
];

/// Verbs that name a bounded side effect (single `Act`) when no `Pursue` cue
/// is present.
const ACT_CUES: &[&str] = &[
    "crie ",
    "criar ",
    "delete",
    "apague",
    "remova",
    "envie",
    "enviar",
    "mande",
    "commit",
    "rode ",
    "rodar ",
    "execute ",
    "abra ",
    "baixe",
    "download",
    "instale",
    "escreva ",
    "send ",
    "create ",
    "open ",
    "run ",
    "delete ",
    "write ",
    "install ",
];

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| haystack.contains(n))
}

/// Pick the smallest capable execution mode for `input`. Pure and
/// deterministic — no model call.
pub fn select_mode(input: &str) -> ModeDecision {
    let lc = input.to_lowercase();

    // 1. Explicit overrides win, most-specific first (Pursue phrases can
    //    contain "faça"-like fragments, so check Pursue before Act).
    if contains_any(&lc, PURSUE_OVERRIDES) {
        return ModeDecision {
            mode: ExecutionMode::Pursue,
            source: ModeSource::Override,
            reason: "explicit pursue override",
        };
    }
    if contains_any(&lc, RESPOND_OVERRIDES) {
        return ModeDecision {
            mode: ExecutionMode::Respond,
            source: ModeSource::Override,
            reason: "explicit respond override",
        };
    }
    if contains_any(&lc, ACT_OVERRIDES) {
        return ModeDecision {
            mode: ExecutionMode::Act,
            source: ModeSource::Override,
            reason: "explicit act override",
        };
    }

    // 2. Heuristics: durability cues promote to Pursue; a bounded side-effect
    //    verb picks Act; otherwise Respond.
    if contains_any(&lc, PURSUE_CUES) {
        return ModeDecision {
            mode: ExecutionMode::Pursue,
            source: ModeSource::Heuristic,
            reason: "multi-step / recurring / out-of-turn cue",
        };
    }
    if contains_any(&lc, ACT_CUES) {
        return ModeDecision {
            mode: ExecutionMode::Act,
            source: ModeSource::Heuristic,
            reason: "bounded side-effect verb",
        };
    }
    ModeDecision {
        mode: ExecutionMode::Respond,
        source: ModeSource::Heuristic,
        reason: "no side-effect or durability cue",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_question_is_respond() {
        let d = select_mode("what is the capital of France?");
        assert_eq!(d.mode, ExecutionMode::Respond);
        assert_eq!(d.source, ModeSource::Heuristic);
    }

    #[test]
    fn bounded_verb_is_act() {
        let d = select_mode("create a file called notes.txt");
        assert_eq!(d.mode, ExecutionMode::Act);
        assert_eq!(d.source, ModeSource::Heuristic);
    }

    #[test]
    fn recurring_cue_is_pursue() {
        let d = select_mode("monitor the site every day and tell me if it changes");
        assert_eq!(d.mode, ExecutionMode::Pursue);
    }

    #[test]
    fn build_app_is_pursue() {
        let d = select_mode("build me an app that tracks my runs");
        assert_eq!(d.mode, ExecutionMode::Pursue);
    }

    #[test]
    fn respond_override_beats_action_verb() {
        // Even with "create", an explicit respond override wins.
        let d = select_mode("just answer: how would you create a REST API?");
        assert_eq!(d.mode, ExecutionMode::Respond);
        assert_eq!(d.source, ModeSource::Override);
    }

    #[test]
    fn pursue_override_wins() {
        let d = select_mode("fix the failing tests and pursue until they pass");
        assert_eq!(d.mode, ExecutionMode::Pursue);
        assert_eq!(d.source, ModeSource::Override);
    }

    #[test]
    fn portuguese_recurring_cue_is_pursue() {
        let d = select_mode("monitore o repositório e me avise a cada push");
        assert_eq!(d.mode, ExecutionMode::Pursue);
    }
}
