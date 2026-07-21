//! A4.5: LLM routing by deterministic CALL-SITE CLASS — not semantic
//! classification. Every LLM call the daemon makes belongs to exactly one of
//! five classes, known statically at the call site: `chat_turn` (interactive
//! conversation turns), `pursue_task` (delegated coding tasks),
//! `cabinet` (multi-persona deliberation), `reflection` (the offline
//! Reflector), `compaction` (history summarization). A routing rule maps a
//! class to a model id; the effective rule per class is the config-store
//! override (`routing.rules`, latest row) else bastion.toml's `[routing]`
//! table else nothing.
//!
//! HONEST v1 — bastion-core is pinned by git rev and this crate never edits
//! it, so a class is only `supported` when the agent can actually push a
//! model into that call site through an EXISTING knob:
//!
//! - `chat_turn` — SUPPORTED (hot): the loop's `SharedProvider` is
//!   hot-swappable between turns, the exact mechanism `/model` uses
//!   (`agent/command.rs::switch_model`). Startup applies the rule when
//!   constructing the provider (main.rs); an approved `routing_config`
//!   proposal swaps it live.
//! - `reflection` — SUPPORTED (next restart): the Reflector's model is a
//!   constructor argument resolved once at daemon start
//!   (`resolve_reflector_provider`, main.rs's reflector block); the routing
//!   rule overrides `[reflector].model` there. The spawned Reflector holds
//!   its provider privately, so a runtime change lands on the NEXT restart.
//! - `pursue_task` — NOT supported: delegated tasks run inside an external
//!   agent runtime (Codex/Claude CLI harness); the `SessionSpec` the
//!   executor builds (`adaptive/exec.rs`) has a `runtime_id` but NO model
//!   field — the harness picks its own model.
//!   TODO(core seam): add a model hint to `SessionSpec`/`TaskInput` in
//!   bastion-core's `bastion-agent-runtime` and thread it through
//!   `RuntimeTaskExecutor::execute`.
//! - `cabinet` — NOT supported: Cabinet legs run on the SAME
//!   `SharedProvider` as chat turns (bastion-personas `persona/runner.rs`
//!   receives the loop's handle) — swapping it would re-route chat too,
//!   never class-scoped.
//!   TODO(core seam): a per-mode provider override on `PersonaResponder` /
//!   the Cabinet orchestrator in bastion-core.
//! - `compaction` — NOT supported: `AutoCompact::compact` is called with
//!   the loop's live provider inside the kernel turn
//!   (bastion-runtime `agent/loop_.rs`) with no injection point.
//!   TODO(core seam): a dedicated compaction provider field on `AgentLoop`
//!   in bastion-core.
//!
//! Rules for unsupported classes are still validated, persisted and
//! reported (`supported: false` on `GET /routing`) so the configuration is
//! already in place the day the core seam lands — persisted, never lied
//! about.

use std::collections::HashMap;

use serde::Serialize;

use crate::config_store::{routing_rules_from_value_json, ConfigStore, KEY_ROUTING_RULES};

/// The five deterministic call-site classes, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RouteClass {
    ChatTurn,
    PursueTask,
    Cabinet,
    Reflection,
    Compaction,
}

impl RouteClass {
    /// Every class, in the stable order `GET /routing` reports.
    pub const ALL: [RouteClass; 5] = [
        RouteClass::ChatTurn,
        RouteClass::PursueTask,
        RouteClass::Cabinet,
        RouteClass::Reflection,
        RouteClass::Compaction,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            RouteClass::ChatTurn => "chat_turn",
            RouteClass::PursueTask => "pursue_task",
            RouteClass::Cabinet => "cabinet",
            RouteClass::Reflection => "reflection",
            RouteClass::Compaction => "compaction",
        }
    }

    /// Strict parse — the validation gate `routing_config` proposals and the
    /// toml loader share. Unknown names are never silently invented into
    /// classes.
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.as_str() == s)
    }

    /// Whether the agent can push a model into this class's call site TODAY
    /// through an existing knob (module docs list the knob per class and the
    /// core seam each unsupported class waits on). Hard-coded reachability,
    /// asserted in tests — never derived from config.
    pub fn supported(&self) -> bool {
        matches!(self, RouteClass::ChatTurn | RouteClass::Reflection)
    }

    /// Whether an applied rule takes effect without a restart. Only
    /// meaningful for supported classes: `chat_turn` hot-swaps the live
    /// `SharedProvider`; `reflection` is read once at daemon start.
    pub fn hot(&self) -> bool {
        matches!(self, RouteClass::ChatTurn)
    }
}

/// Which layer supplied the effective rule for a class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleSource {
    /// The config-store `routing.rules` override (latest audited row).
    Override,
    /// bastion.toml's `[routing]` table.
    Toml,
}

/// One resolved row of the table — the shape `GET /routing` serializes.
#[derive(Debug, Clone, Serialize)]
pub struct RouteReport {
    pub class: &'static str,
    /// Effective model id, `None` when neither layer names one.
    pub model: Option<String>,
    pub source: Option<RuleSource>,
    pub supported: bool,
}

/// The effective routing table: one entry per class, override-else-toml.
#[derive(Debug, Clone, Default)]
pub struct RoutingTable {
    rules: HashMap<RouteClass, (String, RuleSource)>,
}

impl RoutingTable {
    /// Resolve the effective table. Precedence per class: `override_rules`
    /// (the config-store `routing.rules` value, when an override row exists)
    /// else `toml_rules` (`[routing]`) else no rule. Blank models and
    /// unknown class names in either layer are dropped with a warning —
    /// tolerated, never propagated.
    pub fn resolve(
        toml_rules: &HashMap<String, String>,
        override_rules: Option<&HashMap<String, String>>,
    ) -> Self {
        let mut rules = HashMap::new();
        for (layer, source) in [
            (Some(toml_rules), RuleSource::Toml),
            (override_rules, RuleSource::Override),
        ] {
            let Some(layer) = layer else { continue };
            for (class, model) in layer {
                let model = model.trim();
                let Some(parsed) = RouteClass::parse(class) else {
                    tracing::warn!(
                        event = "routing_rule_unknown_class",
                        class = %class,
                        source = ?source,
                        "not a call-site class — rule ignored",
                    );
                    continue;
                };
                if model.is_empty() {
                    continue;
                }
                // Later layers overwrite earlier ones: override wins.
                rules.insert(parsed, (model.to_string(), source));
            }
        }
        Self { rules }
    }

    /// Effective model for a class, if any layer names one.
    pub fn model_for(&self, class: RouteClass) -> Option<&str> {
        self.rules.get(&class).map(|(m, _)| m.as_str())
    }

    /// Which layer supplied `class`'s rule.
    pub fn source_for(&self, class: RouteClass) -> Option<RuleSource> {
        self.rules.get(&class).map(|(_, s)| *s)
    }

    /// All five classes in stable order — `GET /routing`'s `items`.
    pub fn report(&self) -> Vec<RouteReport> {
        RouteClass::ALL
            .into_iter()
            .map(|class| RouteReport {
                class: class.as_str(),
                model: self.model_for(class).map(str::to_string),
                source: self.source_for(class),
                supported: class.supported(),
            })
            .collect()
    }
}

/// Load the effective table: config-store `routing.rules` override (latest
/// row; the empty-map sentinel and malformed JSON both mean "no override")
/// overlaid on bastion.toml's `[routing]`. Store read errors degrade to the
/// toml base with a warning — a broken DB must never take routing reads
/// down with it.
pub async fn load_table(
    store: &ConfigStore,
    toml_rules: &HashMap<String, String>,
) -> RoutingTable {
    let override_rules = match store.latest(KEY_ROUTING_RULES).await {
        Ok(raw) => raw.as_deref().and_then(routing_rules_from_value_json),
        Err(e) => {
            tracing::warn!(event = "routing_override_read_failed", error = %e);
            None
        }
    };
    RoutingTable::resolve(toml_rules, override_rules.as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn class_names_roundtrip_and_unknown_is_refused() {
        for class in RouteClass::ALL {
            assert_eq!(RouteClass::parse(class.as_str()), Some(class));
        }
        assert_eq!(RouteClass::parse("chat-turn"), None);
        assert_eq!(RouteClass::parse("semantic"), None);
        assert_eq!(RouteClass::parse(""), None);
    }

    #[test]
    fn supported_is_exactly_the_reachable_knobs() {
        // Reachability is a statement about the pinned core rev — if this
        // test needs changing, the module docs' knob table changes with it.
        let supported: Vec<&str> = RouteClass::ALL
            .into_iter()
            .filter(RouteClass::supported)
            .map(|c| c.as_str())
            .collect();
        assert_eq!(supported, vec!["chat_turn", "reflection"]);
        // And only chat_turn applies hot (reflection is startup-read).
        assert!(RouteClass::ChatTurn.hot());
        assert!(!RouteClass::Reflection.hot());
    }

    #[test]
    fn override_wins_over_toml_per_class() {
        let toml = rules(&[("chat_turn", "gemini-2.5-flash"), ("reflection", "llama3.2")]);
        let over = rules(&[("chat_turn", "gpt-5-mini")]);
        let table = RoutingTable::resolve(&toml, Some(&over));

        assert_eq!(table.model_for(RouteClass::ChatTurn), Some("gpt-5-mini"));
        assert_eq!(
            table.source_for(RouteClass::ChatTurn),
            Some(RuleSource::Override)
        );
        // reflection has no override entry → the toml rule stays effective.
        assert_eq!(table.model_for(RouteClass::Reflection), Some("llama3.2"));
        assert_eq!(
            table.source_for(RouteClass::Reflection),
            Some(RuleSource::Toml)
        );
        assert_eq!(table.model_for(RouteClass::Cabinet), None);
    }

    #[test]
    fn blank_models_and_unknown_classes_are_dropped() {
        let toml = rules(&[
            ("chat_turn", "   "),
            ("not_a_class", "gpt-5"),
            ("compaction", "llama3.2"),
        ]);
        let table = RoutingTable::resolve(&toml, None);
        assert_eq!(table.model_for(RouteClass::ChatTurn), None);
        assert_eq!(table.model_for(RouteClass::Compaction), Some("llama3.2"));
    }

    #[test]
    fn report_always_lists_all_five_classes_in_order() {
        let toml = rules(&[("reflection", "llama3.2")]);
        let report = RoutingTable::resolve(&toml, None).report();
        let classes: Vec<&str> = report.iter().map(|r| r.class).collect();
        assert_eq!(
            classes,
            vec!["chat_turn", "pursue_task", "cabinet", "reflection", "compaction"]
        );
        let reflection = &report[3];
        assert_eq!(reflection.model.as_deref(), Some("llama3.2"));
        assert_eq!(reflection.source, Some(RuleSource::Toml));
        assert!(reflection.supported);
        let cabinet = &report[2];
        assert_eq!(cabinet.model, None);
        assert_eq!(cabinet.source, None);
        assert!(!cabinet.supported);
    }

    #[test]
    fn rule_source_serializes_snake_case() {
        assert_eq!(
            serde_json::to_value(RuleSource::Override).unwrap(),
            serde_json::json!("override")
        );
        assert_eq!(
            serde_json::to_value(RuleSource::Toml).unwrap(),
            serde_json::json!("toml")
        );
    }

    #[tokio::test]
    async fn load_table_overlays_store_on_toml() {
        let f = tempfile::NamedTempFile::new().unwrap();
        let store = ConfigStore::new(f.path().to_str().unwrap().to_owned());
        store.init_schema().await.unwrap();
        let toml = rules(&[("chat_turn", "gemini-2.5-flash")]);

        // No override row → toml passes through.
        let table = load_table(&store, &toml).await;
        assert_eq!(
            table.model_for(RouteClass::ChatTurn),
            Some("gemini-2.5-flash")
        );

        store
            .apply(
                KEY_ROUTING_RULES,
                &crate::config_store::routing_rules_value_json(&rules(&[(
                    "chat_turn",
                    "llama3.2",
                )])),
                "web",
                None,
            )
            .await
            .unwrap();
        let table = load_table(&store, &toml).await;
        assert_eq!(table.model_for(RouteClass::ChatTurn), Some("llama3.2"));
        assert_eq!(
            table.source_for(RouteClass::ChatTurn),
            Some(RuleSource::Override)
        );

        // Empty rules map = the cleared sentinel → back to toml.
        store
            .apply(
                KEY_ROUTING_RULES,
                &crate::config_store::routing_rules_value_json(&HashMap::new()),
                "web",
                None,
            )
            .await
            .unwrap();
        let table = load_table(&store, &toml).await;
        assert_eq!(
            table.model_for(RouteClass::ChatTurn),
            Some("gemini-2.5-flash")
        );
        assert_eq!(
            table.source_for(RouteClass::ChatTurn),
            Some(RuleSource::Toml)
        );
    }
}
