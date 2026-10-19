//! The context a host agent receives so it acts as this Bastion: the active
//! persona, the persona roster, core and relevant memories, and goals.
//!
//! A host agent is an external destination, so every piece passes the same
//! egress rule the MCP server applies (`check_egress(tier, "external")`): a
//! `local_only` belief or persona never leaves this machine through here.

use std::collections::HashSet;
use std::sync::Arc;

use bastion_cognition::goal::GoalEngine;
use bastion_memory::{Belief, SharedMemory};
use bastion_personas::persona::PersonaRegistry;
use bastion_runtime::hooks::egress::check_egress;

use super::policy::cut;

/// What the host asked for: the whole frame at session start, or only the
/// memories relevant to one prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextKind {
    Session,
    Prompt,
}

/// The persona tag journal entries are stored under (`local_only`, never
/// injected — listed here so the filter does not depend on that alone).
pub const JOURNAL_TAG: &str = "journal";

const SESSION_BELIEFS: usize = 8;
const PROMPT_BELIEFS: usize = 5;
const BELIEF_CHARS: usize = 280;

#[derive(Clone)]
pub struct ContextSources {
    pub memory: SharedMemory,
    pub personas: Arc<PersonaRegistry>,
    pub goals: GoalEngine,
}

fn egress_ok(tier: Option<bastion_memory::PrivacyTier>) -> bool {
    check_egress(tier, "external").is_ok()
}

/// The persona to act as: the chosen one when it still exists, else the
/// heaviest persona, else none.
pub fn resolve_persona(personas: &PersonaRegistry, chosen: Option<&str>) -> Option<String> {
    if let Some(name) = chosen.filter(|n| personas.get(n).is_some()) {
        return Some(name.to_string());
    }
    let mut names = personas.names();
    names.sort_unstable();
    names
        .into_iter()
        .filter_map(|n| personas.get(n).map(|p| (n, p.weight)))
        .fold(None::<(&str, f32)>, |best, (name, weight)| match best {
            Some((_, w)) if w >= weight => best,
            _ => Some((name, weight)),
        })
        .map(|(name, _)| name.to_string())
}

/// Lowercase words of 3+ letters, minus a few that match everything.
fn terms(text: &str) -> HashSet<String> {
    const STOP: &[&str] = &[
        "the", "and", "for", "with", "that", "this", "from", "you", "are", "was", "have", "not",
        "but", "what", "how", "can", "uma", "que", "com", "para", "por", "não", "mais", "como",
        "isso", "esse", "essa", "dos", "das",
    ];
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 3)
        .map(str::to_lowercase)
        .filter(|w| !STOP.contains(&w.as_str()))
        .collect()
}

/// Relevance of a belief to the prompt's terms: shared terms (keywords
/// count double) times the belief's weight. Zero = unrelated.
fn relevance(belief: &Belief, prompt: &HashSet<String>) -> f64 {
    let content = terms(&belief.content);
    let mut shared = content.intersection(prompt).count() as f64;
    shared += 2.0
        * belief
            .keywords
            .iter()
            .filter(|k| prompt.contains(&k.to_lowercase()))
            .count() as f64;
    shared * belief.weight.max(0.1)
}

/// Builds the context. Returns the text (empty when there is nothing worth
/// sending) and the persona acted as.
pub async fn build(
    sources: &ContextSources,
    owner: &str,
    chosen_persona: Option<&str>,
    kind: ContextKind,
    prompt: Option<&str>,
    max_chars: usize,
) -> (String, Option<String>) {
    let persona = resolve_persona(&sources.personas, chosen_persona);
    let beliefs: Vec<Belief> = {
        let memory = sources.memory.read().await;
        memory
            .retrieve_tagged(owner, persona.as_deref())
            .await
            .unwrap_or_default()
    };
    let beliefs: Vec<Belief> = beliefs
        .into_iter()
        .filter(|b| egress_ok(b.tier))
        .filter(|b| b.persona_tag.as_deref() != Some(JOURNAL_TAG))
        .collect();

    let text = match kind {
        ContextKind::Session => {
            let goals = sources.goals.list_goals(owner).await.unwrap_or_default();
            session_text(sources, persona.as_deref(), &beliefs, &goals)
        }
        ContextKind::Prompt => prompt_text(&beliefs, prompt.unwrap_or_default()),
    };
    (cut(&text, max_chars), persona)
}

fn session_text(
    sources: &ContextSources,
    persona: Option<&str>,
    beliefs: &[Belief],
    goals: &[bastion_types::Goal],
) -> String {
    let mut out = String::from(
        "<bastion>\nYou are running in Bastion mode: you are this person's Bastion. The persona, \
         memory and goals below are Bastion's, and you act as Bastion while you work here. \
         Bastion's tool rules apply to your tool calls.\n",
    );

    match persona.and_then(|name| sources.personas.get(name).map(|p| (name, p))) {
        Some((name, p)) if egress_ok(Some(p.tier)) => {
            out.push_str(&format!(
                "\n## Persona: {name}\n{}\n",
                p.system_prompt.trim()
            ));
        }
        Some((name, _)) => out.push_str(&format!(
            "\n## Persona: {name}\nThis persona is local-only: its instructions stay on this \
             machine. Act in its name with a neutral voice.\n"
        )),
        None => {}
    }

    let mut others: Vec<&str> = sources
        .personas
        .names()
        .into_iter()
        .filter(|n| Some(*n) != persona)
        .collect();
    others.sort_unstable();
    if !others.is_empty() {
        out.push_str("\n## Other personas (switch with /bastion:persona <name>)\n");
        for name in others {
            let description = sources
                .personas
                .get(name)
                .filter(|p| egress_ok(Some(p.tier)))
                .and_then(|p| p.description.clone())
                .unwrap_or_default();
            if description.is_empty() {
                out.push_str(&format!("- {name}\n"));
            } else {
                out.push_str(&format!("- {name}: {}\n", cut(description.trim(), 120)));
            }
        }
    }

    let mut ranked: Vec<&Belief> = beliefs.iter().collect();
    ranked.sort_by(|a, b| {
        b.is_core.cmp(&a.is_core).then(
            b.weight
                .partial_cmp(&a.weight)
                .unwrap_or(std::cmp::Ordering::Equal),
        )
    });
    if !ranked.is_empty() {
        out.push_str("\n## What Bastion remembers\n");
        for belief in ranked.into_iter().take(SESSION_BELIEFS) {
            out.push_str(&format!("- {}\n", cut(belief.content.trim(), BELIEF_CHARS)));
        }
    }

    if !goals.is_empty() {
        out.push_str("\n## Goals\n");
        for goal in goals {
            match &goal.metric {
                Some(metric) if !metric.is_empty() => {
                    out.push_str(&format!("- {} ({metric})\n", goal.description.trim()))
                }
                _ => out.push_str(&format!("- {}\n", goal.description.trim())),
            }
        }
    }

    out.push_str(
        "\n## Memory\nWhen you learn something durable about this person or project, save it \
         with the `memory_store` tool of the `bastion` MCP server (tier \"cloud_ok\"). Never \
         store secrets or credentials.\n</bastion>",
    );
    out
}

fn prompt_text(beliefs: &[Belief], prompt: &str) -> String {
    let prompt_terms = terms(prompt);
    if prompt_terms.is_empty() {
        return String::new();
    }
    let mut scored: Vec<(f64, &Belief)> = beliefs
        .iter()
        .filter(|b| !b.is_core)
        .map(|b| (relevance(b, &prompt_terms), b))
        .filter(|(score, _)| *score > 0.0)
        .collect();
    if scored.is_empty() {
        return String::new();
    }
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut out = String::from("<bastion-memory>\nRelevant to this request:\n");
    for (_, belief) in scored.into_iter().take(PROMPT_BELIEFS) {
        out.push_str(&format!("- {}\n", cut(belief.content.trim(), BELIEF_CHARS)));
    }
    out.push_str("</bastion-memory>");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn belief(content: &str, keywords: &[&str], weight: f64) -> Belief {
        let raw = serde_json::json!({
            "id": 1,
            "owner_id": "_local",
            "persona_tag": null,
            "content": content,
            "weight": weight,
            "is_core": false,
            "tier": "cloud-ok",
            "kind": "factual",
            "keywords": keywords,
            "issue": null,
            "helpful_count": 0,
            "harmful_count": 0,
            "neutral_count": 0,
        });
        serde_json::from_value(raw).expect("belief fixture matches bastion-types")
    }

    #[test]
    fn prompt_context_lists_only_related_beliefs() {
        let beliefs = vec![
            belief("The deploy pipeline runs on Coolify", &["deploy"], 1.0),
            belief("Mario prefers tea in the afternoon", &[], 1.0),
        ];
        let text = prompt_text(&beliefs, "fix the deploy pipeline please");
        assert!(text.contains("Coolify"));
        assert!(!text.contains("tea"));
        assert!(prompt_text(&beliefs, "ok").is_empty());
        assert!(prompt_text(&beliefs, "unrelated words entirely").is_empty());
    }

    #[test]
    fn terms_drop_short_and_stop_words() {
        let t = terms("Fix the CI job for the web app");
        assert!(t.contains("job"));
        assert!(t.contains("web"));
        assert!(!t.contains("the"));
        assert!(!t.contains("ci"));
    }
}
