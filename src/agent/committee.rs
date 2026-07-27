//! `/committee <pergunta>` — M4 of the hedge-fund-committee backlog item:
//! the "Estágio 2" dissent-gate the original design called for, on top of
//! M3's simpler "just use `/cabinet`" pass.
//!
//! This is the first place in `bastion-agent` that calls
//! `bastion_cognition::cabinet::{build_table, orchestrator::deliberate,
//! synth::synthesize}` DIRECTLY, composing Cabinet twice in one turn instead
//! of once via the automatic router/`forced_cabinet` path — confirmed via
//! research before writing this that no precedent for that composition
//! existed anywhere in the codebase. Every `RouterDecision` built here is
//! hand-constructed (never from the LLM router), tagged
//! `ConveneReason::ManualOverride` — the variant that exists precisely for
//! a decision the router didn't produce.
//!
//! Pipeline:
//! 1. Stage 1 — convene the 4 signal personas (fundamentalist, contrarian,
//!    macro, growth) for exactly 1 round (Position only, no reply round —
//!    they must NOT see each other yet). Parse each turn's `{signal,
//!    confidence, reasoning}` JSON.
//! 2. Disagreement gate — if the parsed `signal` values don't all agree,
//!    convene a SECOND, narrower Cabinet with only the personas that
//!    produced a signal (a real multi-round debate). If they already
//!    agree, skip straight to stage 3 with stage 1's signals as the
//!    candidate.
//! 3. Stage 3 — Risk Manager, dispatched via a hand-built `Single`-mode
//!    decision (NOT Cabinet) — one persona, one call, no debate.
//! 4. Stage 4 — Portfolio Manager, same single-dispatch shape, folding in
//!    the Risk Manager's output.
//!
//! Egress: `orchestrator::deliberate` and `runner::run`'s `Single` path both
//! call `check_egress` internally per-persona-per-call already — the ONE
//! gate that's on this module to add itself is the same one
//! `PersonaResponder::respond` adds before calling `synth::synthesize`
//! (which does not gate internally): see the stage-2 synthesis call below.

use crate::agent::committee_store::{self, SignalRecord};
use bastion_cognition::cabinet::{build_table, orchestrator, synth};
use bastion_memory::{BeliefDraft, Outcome, PrivacyTier, SharedMemory};
use bastion_personas::persona::{runner, PersonaRegistry};
use bastion_providers::SharedProvider;
use bastion_types::{
    CallConfig, ConveneReason, Message, MessageContent, ResponseMode, Role, RouterDecision,
};
use std::collections::HashSet;

/// Hardcoded to this one pack's roster — same honesty precedent as
/// `extension_command.rs`'s `GIT_CAPABILITY_CRATE_NAME` ("a second
/// consumer would warrant a real registry; with exactly one, a hardcoded
/// match is honest about the actual state of the mechanism").
const SIGNAL_PERSONAS: &[&str] = &["fundamentalist", "contrarian", "macro", "growth"];
const RISK_MANAGER: &str = "risk-manager";
const PORTFOLIO_MANAGER: &str = "portfolio-manager";
/// Real debate needs more than 1 round (unlike stage 1, which is
/// deliberately Position-only) — clamped by `orchestrator::MAX_ROUNDS`
/// regardless.
const STAGE2_ROUNDS: u8 = 2;

/// M6: content prefix marking the one procedural belief per (owner, persona)
/// that stands in for "this signal persona's committee track record" — the
/// thing `reinforce_persona_belief`/`weaken_persona_belief` actually move.
/// Distinguishes it from any other persona-tagged procedural belief a future
/// mechanism might store under the same (owner, persona_tag).
const TRUST_MARKER: &str = "committee persona trust score";
/// Flat per-graded-outcome adjustment — larger than the background
/// `procedural_outcome::REINFORCE_DELTA` (0.1) since a `/committee outcome`
/// call is a rarer, higher-signal, human-graded event, not an automatic
/// per-turn nudge. Symmetric: the same magnitude rewards or punishes.
///
/// Deliberately SMALLER than the belief's starting weight (1.0, set by
/// `store_procedural_belief`): `weaken_persona_belief` floors at 0.0, and
/// `retrieve_tagged`'s `weight > 0` gate makes a fully-depleted belief
/// invisible to `persona_trust_weight` — a delta equal to 1.0 would erase a
/// persona's trust display after exactly one wrong call, indistinguishable
/// from never having been graded at all. 0.3 means it takes ~4 consecutive
/// wrong calls to fall out of view, not 1.
const PERSONA_TRUST_DELTA: f64 = 0.3;

#[derive(Debug, serde::Deserialize)]
struct Signal {
    signal: String,
    /// Loose on purpose: real testing saw both `0.75` (number) and `"0%"`
    /// (string) from the same persona prompt across different calls —
    /// `confidence` is for the operator to read, not machine-compared, so
    /// accepting either shape beats failing to parse an otherwise-valid
    /// signal over a formatting quirk.
    confidence: serde_json::Value,
    #[allow(dead_code)]
    reasoning: String,
}

fn format_confidence(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Extract the outermost `{...}` from text that may be wrapped in markdown
/// fences or prose — same approach as `persona::router::extract_json`
/// (private to that module, not reusable from here, so re-implemented; see
/// this module's own doc comment / the M4 research that confirmed no
/// shared utility exists for this in bastion-core today).
fn extract_json(s: &str) -> &str {
    match (s.find('{'), s.rfind('}')) {
        (Some(a), Some(b)) if b > a => &s[a..=b],
        _ => s.trim(),
    }
}

fn parse_signal(raw: &str) -> Option<Signal> {
    serde_json::from_str(extract_json(raw)).ok()
}

/// Handle `/committee <pergunta>`.
#[allow(clippy::too_many_arguments)]
pub async fn handle(
    registry: &PersonaRegistry,
    provider: SharedProvider,
    capability_registry: &mut bastion_runtime::capability::CapabilityRegistry,
    arg: Option<&str>,
    owner: &str,
    db_path: &str,
    memory: SharedMemory,
) -> anyhow::Result<String> {
    let question = arg.unwrap_or("").trim();
    if question.is_empty() {
        return Ok(
            "usage: /committee <pergunta>\n       /committee outcome <id> <helpful|harmful|neutral>"
                .to_string(),
        );
    }

    let mut report = String::new();

    // --- Stage 1: independent signals, 1 round, no reply round. ---
    let stage1_decision = RouterDecision {
        personas: SIGNAL_PERSONAS.iter().map(|s| (*s).to_string()).collect(),
        owner: owner.to_string(),
        mode: ResponseMode::Cabinet,
        convene_reason: Some(ConveneReason::ManualOverride),
    };
    let stage1_table = build_table(|name| registry.get(name).cloned(), &stage1_decision, None)?;
    let stage1_transcript = orchestrator::deliberate(
        &stage1_table,
        provider.clone(),
        1,
        capability_registry,
        question,
    )
    .await?;

    report.push_str("## Estágio 1 — sinais independentes\n");
    let mut signals: Vec<(String, Signal)> = Vec::new();
    for turn in &stage1_transcript {
        match parse_signal(&turn.text) {
            Some(sig) => {
                // M6: informational only -- surfaces the persona's current
                // committee track record, does NOT bias the disagreement gate
                // or synthesis below. See this module's doc comment.
                let trust_note = match persona_trust_weight(&memory, owner, &turn.persona).await {
                    Some(w) => format!(", peso de confiança: {w:.1}"),
                    None => String::new(),
                };
                report.push_str(&format!(
                    "- **{}**: {} (confiança: {}{trust_note})\n",
                    turn.persona,
                    sig.signal,
                    format_confidence(&sig.confidence)
                ));
                signals.push((turn.persona.clone(), sig));
            }
            None => {
                report.push_str(&format!(
                    "- **{}**: (sem sinal utilizável — falha na chamada ou resposta não estruturada)\n",
                    turn.persona
                ));
            }
        }
    }

    if signals.is_empty() {
        report.push_str(
            "\nNenhuma persona de sinal respondeu com sucesso — não dá pra seguir pro Risk \
             Manager. Tente de novo (provável rate limit do provider).\n",
        );
        return Ok(report);
    }

    // --- Disagreement gate. ---
    let normalized: HashSet<String> = signals
        .iter()
        .map(|(_, s)| s.signal.trim().to_uppercase())
        .collect();
    let disagree = normalized.len() > 1 && signals.len() >= 2;

    // M5/M6: which signal personas' stage-1 vote matched the candidate the
    // committee actually carried forward -- the attribution `/committee
    // outcome` later rewards or punishes per persona.
    let (candidate_text, signal_records): (String, Vec<SignalRecord>) = if disagree {
        report.push_str(&format!(
            "\n**Divergência real detectada** ({} sinais distintos entre {} personas) — \
             convocando debate (Estágio 2)...\n\n",
            normalized.len(),
            signals.len()
        ));

        let dissenting_names: Vec<String> = signals.iter().map(|(name, _)| name.clone()).collect();
        let stage2_decision = RouterDecision {
            personas: dissenting_names,
            owner: owner.to_string(),
            mode: ResponseMode::Cabinet,
            convene_reason: Some(ConveneReason::ManualOverride),
        };
        let stage2_table = build_table(|name| registry.get(name).cloned(), &stage2_decision, None)?;
        let stage2_transcript = orchestrator::deliberate(
            &stage2_table,
            provider.clone(),
            STAGE2_ROUNDS,
            capability_registry,
            question,
        )
        .await?;

        report.push_str("## Estágio 2 — debate entre quem discordou\n");
        for turn in &stage2_transcript {
            report.push_str(&format!(
                "[{}] ({:?}): {}\n\n",
                turn.persona, turn.kind, turn.text
            ));
        }

        // CR-02 precedent (responder.rs's own Cabinet arm does the same
        // before calling synthesize, which does not gate internally):
        // fail-closed egress on the transcript before it reaches synthesis.
        let synth_provider_name = provider.read().await.name().to_owned();
        bastion_runtime::hooks::egress::check_egress(
            Some(stage2_table.tier),
            &synth_provider_name,
        )?;

        let synth_result = {
            let guard = provider.read().await;
            synth::synthesize(&**guard, &stage2_transcript, capability_registry).await
        };
        match synth_result {
            Ok(verdict) => {
                report.push_str(&format!(
                    "\n**Síntese do debate**: {}\n",
                    verdict.recommendation
                ));
                if !verdict.dissents.is_empty() {
                    report.push_str("Dissidências remanescentes:\n");
                    for d in &verdict.dissents {
                        report.push_str(&format!("  - {}: {}\n", d.persona, d.position));
                    }
                }
                // Aligned = the persona's position survived synthesis (i.e. it
                // is NOT among verdict.dissents).
                let dissent_names: HashSet<&str> = verdict
                    .dissents
                    .iter()
                    .map(|d| d.persona.as_str())
                    .collect();
                let records = signals
                    .iter()
                    .map(|(name, sig)| SignalRecord {
                        persona: name.clone(),
                        signal: sig.signal.clone(),
                        aligned: !dissent_names.contains(name.as_str()),
                    })
                    .collect();
                (verdict.recommendation, records)
            }
            Err(e) => {
                report.push_str(&format!(
                    "\n(síntese do debate falhou: {e} — seguindo com as posições brutas do debate)\n"
                ));
                let text = stage2_transcript
                    .iter()
                    .map(|t| format!("[{}]: {}", t.persona, t.text))
                    .collect::<Vec<_>>()
                    .join("\n");
                // No verdict => no reliable per-persona attribution data; an
                // empty Vec means /committee outcome later applies no
                // reinforcement for this run rather than guessing.
                (text, Vec::new())
            }
        }
    } else {
        let agreed_signal = normalized.iter().next().cloned().unwrap_or_default();
        report.push_str(&format!(
            "\nSinais concordam ({agreed_signal} — {} persona(s)) — pula direto pro Risk Manager, \
             Estágio 2 não foi acionado.\n",
            signals.len()
        ));
        let text = signals
            .iter()
            .map(|(name, s)| {
                format!(
                    "[{name}] signal={} confidence={} reasoning={}",
                    s.signal,
                    format_confidence(&s.confidence),
                    s.reasoning
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        // Unanimous stage 1 => every signal persona is aligned with the
        // candidate by definition.
        let records = signals
            .iter()
            .map(|(name, sig)| SignalRecord {
                persona: name.clone(),
                signal: sig.signal.clone(),
                aligned: true,
            })
            .collect();
        (text, records)
    };

    // --- Stage 3: Risk Manager, single dispatch (not Cabinet). ---
    report.push_str("\n## Estágio 3 — Risk Manager\n");
    let risk_prompt = format!(
        "Pergunta original do comitê: {question}\n\nCandidato(s) produzido(s) pelo comitê até \
         aqui:\n{candidate_text}\n\nAplique seu filtro de risco (limite de posição) sobre a(s) \
         proposta(s) acima."
    );
    let risk_text =
        run_single_persona(registry, provider.clone(), RISK_MANAGER, &risk_prompt).await?;
    report.push_str(&risk_text);
    report.push('\n');

    // --- Stage 4: Portfolio Manager, single dispatch (not Cabinet). ---
    report.push_str("\n## Estágio 4 — Portfolio Manager (decisão final)\n");
    let pm_prompt = format!(
        "Pergunta original do comitê: {question}\n\nCandidato(s) do comitê:\n{candidate_text}\n\n\
         Parecer do Risk Manager:\n{risk_text}\n\nDê a decisão final do comitê, com veredito \
         auditável e veto explícito se for o caso."
    );
    let pm_text = run_single_persona(registry, provider, PORTFOLIO_MANAGER, &pm_prompt).await?;
    report.push_str(&pm_text);

    // --- M5: persist the run so an operator can grade it later. ---
    match committee_store::insert(db_path, owner, question, &signal_records, &pm_text).await {
        Ok(record_id) => {
            report.push_str(&format!(
                "\n\n---\nregistro **#{record_id}** salvo. Quando souber o resultado real, rode \
                 `/committee outcome {record_id} helpful|harmful|neutral` — isso ajusta o peso de \
                 confiança de cada persona de sinal (M6)."
            ));
        }
        Err(e) => {
            report.push_str(&format!(
                "\n\n(não foi possível salvar o registro deste comitê para avaliação futura: {e})"
            ));
        }
    }

    Ok(report)
}

/// M6, informational only: current stigmergy weight of the ONE procedural
/// belief standing in for `persona`'s committee track record (see
/// `TRUST_MARKER`), or `None` if that persona has no recorded outcome yet.
/// Never influences the disagreement gate or synthesis above — see this
/// module's doc comment for why that's a deliberate, separate decision.
async fn persona_trust_weight(memory: &SharedMemory, owner: &str, persona: &str) -> Option<f64> {
    let mem = memory.read().await;
    let beliefs = mem.retrieve_tagged(owner, Some(persona)).await.ok()?;
    beliefs
        .iter()
        .find(|b| b.content.starts_with(TRUST_MARKER))
        .map(|b| b.weight)
}

/// Handle `/committee outcome <id> <helpful|harmful|neutral>` — M5's grading
/// half. Reads back a run persisted by `handle()`, applies M6's per-persona
/// stigmergy reinforcement/weaken based on each signal persona's recorded
/// `aligned` flag, and marks the run as graded (exactly once — see
/// `committee_store::record_outcome`'s reject-double-record guard).
pub async fn handle_outcome(
    db_path: &str,
    memory: SharedMemory,
    owner: &str,
    arg: Option<&str>,
) -> anyhow::Result<String> {
    let arg = arg.unwrap_or("").trim();
    let mut parts = arg.splitn(2, char::is_whitespace);
    let id: i64 =
        parts.next().unwrap_or("").parse().map_err(|_| {
            anyhow::anyhow!("uso: /committee outcome <id> <helpful|harmful|neutral>")
        })?;
    let outcome_norm = parts.next().unwrap_or("").trim().to_lowercase();
    if !matches!(outcome_norm.as_str(), "helpful" | "harmful" | "neutral") {
        anyhow::bail!("resultado inválido: '{outcome_norm}' — use helpful, harmful ou neutral");
    }

    let row = committee_store::get(db_path, owner, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("registro #{id} não encontrado para este owner"))?;
    if let Some(existing) = &row.outcome {
        anyhow::bail!("registro #{id} já tem um resultado registrado ({existing})");
    }

    let mut report = String::new();
    if row.signals.is_empty() {
        report.push_str(
            "(nenhum dado de alinhamento por persona foi salvo nesse registro — a síntese do \
             Estágio 2 falhou na hora, sem atribuição possível — nenhum peso será ajustado.)\n",
        );
    } else if outcome_norm == "neutral" {
        report.push_str("resultado neutro — nenhum ajuste de peso de confiança foi aplicado.\n");
    } else {
        let mem = memory.write().await;
        report.push_str("ajustes de peso de confiança:\n");
        for sig in &row.signals {
            let existing = mem.retrieve_tagged(owner, Some(&sig.persona)).await?;
            let belief_id = match existing
                .iter()
                .find(|b| b.content.starts_with(TRUST_MARKER))
            {
                Some(b) => b.id,
                None => {
                    mem.store_procedural_belief(BeliefDraft {
                        owner_id: owner.to_string(),
                        persona_tag: Some(sig.persona.clone()),
                        issue: None,
                        insight: format!("{TRUST_MARKER}: {}", sig.persona),
                        keywords: vec![],
                        session_id: "committee".to_string(),
                        source: "committee-outcome".to_string(),
                        tier: Some(PrivacyTier::LocalOnly),
                    })
                    .await?
                }
            };
            // Reward when the persona's stage-1 signal matched a candidate that
            // turned out helpful, OR its signal was the minority position that
            // dissented from a candidate that turned out harmful. Punish the
            // opposite pairing in both cases.
            let reward = (outcome_norm == "helpful") == sig.aligned;
            if reward {
                mem.reinforce_persona_belief(owner, belief_id, PERSONA_TRUST_DELTA)
                    .await?;
                mem.record_belief_outcome(owner, belief_id, Outcome::Helpful)
                    .await?;
                report.push_str(&format!("  - {}: +{PERSONA_TRUST_DELTA:.1}\n", sig.persona));
            } else {
                mem.weaken_persona_belief(owner, belief_id, PERSONA_TRUST_DELTA)
                    .await?;
                mem.record_belief_outcome(owner, belief_id, Outcome::Harmful)
                    .await?;
                report.push_str(&format!("  - {}: -{PERSONA_TRUST_DELTA:.1}\n", sig.persona));
            }
        }
    }

    committee_store::record_outcome(db_path, owner, id, &outcome_norm).await?;
    report.push_str(&format!(
        "\nregistro #{id} marcado como **{outcome_norm}**."
    ));
    Ok(report)
}

/// Hand-builds a `Single`-mode `RouterDecision` for exactly one named
/// persona and runs it through `runner::run` — NOT a Cabinet call. Egress
/// gating is handled internally by `runner::run`'s `Single` path
/// (`run_single`'s own `check_egress` call), so this function doesn't need
/// to gate itself.
async fn run_single_persona(
    registry: &PersonaRegistry,
    provider: SharedProvider,
    persona_name: &str,
    prompt: &str,
) -> anyhow::Result<String> {
    let decision = RouterDecision {
        personas: vec![persona_name.to_string()],
        owner: bastion_runtime::agent::loop_::DEFAULT_OWNER.to_string(),
        mode: ResponseMode::Single,
        convene_reason: None,
    };
    let history = vec![Message {
        role: Role::User,
        content: MessageContent::Text(prompt.to_string()),
    }];
    let config = CallConfig {
        max_tokens: 2048,
        ..Default::default()
    };
    match runner::run(decision, registry, provider, &history, &config).await? {
        runner::RunnerOutput::Single(_, response) => Ok(response.text),
        other => {
            anyhow::bail!("expected RunnerOutput::Single for a Single-mode decision, got {other:?}")
        }
    }
}

// ---------------------------------------------------------------------------
// Tests for handle_outcome (M5/M6) — offline, no LLM: bypasses handle()'s
// pipeline entirely by inserting committee_store rows directly, matching the
// exact shape a real run would have produced.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod outcome_tests {
    use super::*;
    use bastion_memory::sqlite::SqliteMemory;
    use bastion_memory::Memory;
    use std::sync::Arc;
    use tempfile::NamedTempFile;
    use tokio::sync::RwLock;

    async fn make_env() -> (NamedTempFile, String, SharedMemory) {
        let f = NamedTempFile::new().unwrap();
        let db_path = f.path().to_str().unwrap().to_owned();
        bastion_runtime::session::SessionManager::new(&db_path)
            .init_schema()
            .await
            .expect("init session schema");
        committee_store::init_schema(&db_path)
            .await
            .expect("init committee schema");
        let memory: SharedMemory = Arc::new(RwLock::new(
            Box::new(SqliteMemory::new(&db_path)) as Box<dyn Memory>
        ));
        (f, db_path, memory)
    }

    async fn trust_weight(memory: &SharedMemory, owner: &str, persona: &str) -> Option<f64> {
        persona_trust_weight(memory, owner, persona).await
    }

    #[tokio::test]
    async fn helpful_rewards_aligned_and_punishes_dissenting() {
        let (_f, db_path, memory) = make_env().await;
        let signals = vec![
            SignalRecord {
                persona: "fundamentalist".into(),
                signal: "BUY".into(),
                aligned: true,
            },
            SignalRecord {
                persona: "contrarian".into(),
                signal: "SELL".into(),
                aligned: false,
            },
        ];
        let id = committee_store::insert(&db_path, "alice", "AAPL?", &signals, "comprar 1%")
            .await
            .unwrap();

        let report = handle_outcome(
            &db_path,
            memory.clone(),
            "alice",
            Some(&format!("{id} helpful")),
        )
        .await
        .expect("handle_outcome");
        assert!(report.contains("marcado como **helpful**"), "{report}");

        assert_eq!(
            trust_weight(&memory, "alice", "fundamentalist").await,
            Some(1.3)
        );
        assert_eq!(
            trust_weight(&memory, "alice", "contrarian").await,
            Some(0.7)
        );
    }

    #[tokio::test]
    async fn harmful_rewards_dissenting_and_punishes_aligned() {
        let (_f, db_path, memory) = make_env().await;
        let signals = vec![
            SignalRecord {
                persona: "fundamentalist".into(),
                signal: "BUY".into(),
                aligned: true,
            },
            SignalRecord {
                persona: "contrarian".into(),
                signal: "SELL".into(),
                aligned: false,
            },
        ];
        let id = committee_store::insert(&db_path, "alice", "AAPL?", &signals, "comprar 1%")
            .await
            .unwrap();

        handle_outcome(
            &db_path,
            memory.clone(),
            "alice",
            Some(&format!("{id} harmful")),
        )
        .await
        .expect("handle_outcome");

        assert_eq!(
            trust_weight(&memory, "alice", "fundamentalist").await,
            Some(0.7)
        );
        assert_eq!(
            trust_weight(&memory, "alice", "contrarian").await,
            Some(1.3)
        );
    }

    #[tokio::test]
    async fn repeated_wrong_calls_eventually_fall_out_of_view_not_after_one() {
        // PERSONA_TRUST_DELTA (0.3) is deliberately smaller than the belief's
        // starting weight (1.0) -- one wrong call must NOT erase the trust
        // display; it should take several.
        let (_f, db_path, memory) = make_env().await;
        for i in 0..3 {
            let signals = vec![SignalRecord {
                persona: "growth".into(),
                signal: "BUY".into(),
                aligned: true,
            }];
            let id = committee_store::insert(&db_path, "alice", &format!("q{i}"), &signals, "r")
                .await
                .unwrap();
            handle_outcome(
                &db_path,
                memory.clone(),
                "alice",
                Some(&format!("{id} harmful")),
            )
            .await
            .unwrap();
        }
        // 1.0 - 3*0.3 ≈ 0.1 -- still visible after 3 wrong calls in a row.
        let w = trust_weight(&memory, "alice", "growth")
            .await
            .expect("still visible");
        assert!((w - 0.1).abs() < 1e-9, "expected ≈0.1, got {w}");
    }

    #[tokio::test]
    async fn neutral_applies_no_weight_change() {
        let (_f, db_path, memory) = make_env().await;
        let signals = vec![SignalRecord {
            persona: "macro".into(),
            signal: "HOLD".into(),
            aligned: true,
        }];
        let id = committee_store::insert(&db_path, "alice", "q", &signals, "r")
            .await
            .unwrap();

        let report = handle_outcome(
            &db_path,
            memory.clone(),
            "alice",
            Some(&format!("{id} neutral")),
        )
        .await
        .expect("handle_outcome");
        assert!(report.contains("nenhum ajuste"), "{report}");
        assert_eq!(
            trust_weight(&memory, "alice", "macro").await,
            None,
            "neutral must not even create a trust belief"
        );
    }

    #[tokio::test]
    async fn empty_signals_skip_reinforcement_entirely() {
        // Mirrors the synth-failure path in handle(): an empty Vec means no
        // attribution data, not "everyone was wrong."
        let (_f, db_path, memory) = make_env().await;
        let id = committee_store::insert(&db_path, "alice", "q", &[], "r")
            .await
            .unwrap();

        let report = handle_outcome(
            &db_path,
            memory.clone(),
            "alice",
            Some(&format!("{id} helpful")),
        )
        .await
        .expect("handle_outcome");
        assert!(report.contains("nenhum dado de alinhamento"), "{report}");
    }

    #[tokio::test]
    async fn second_grading_of_same_record_is_rejected() {
        let (_f, db_path, memory) = make_env().await;
        let signals = vec![SignalRecord {
            persona: "growth".into(),
            signal: "BUY".into(),
            aligned: true,
        }];
        let id = committee_store::insert(&db_path, "alice", "q", &signals, "r")
            .await
            .unwrap();

        handle_outcome(
            &db_path,
            memory.clone(),
            "alice",
            Some(&format!("{id} helpful")),
        )
        .await
        .expect("first grading");
        let weight_after_first = trust_weight(&memory, "alice", "growth").await;

        let second = handle_outcome(
            &db_path,
            memory.clone(),
            "alice",
            Some(&format!("{id} harmful")),
        )
        .await;
        assert!(
            second.is_err(),
            "grading the same record twice must be rejected"
        );
        assert_eq!(
            trust_weight(&memory, "alice", "growth").await,
            weight_after_first,
            "the rejected second grading must not have moved the weight"
        );
    }

    #[tokio::test]
    async fn rejects_invalid_outcome_string() {
        let (_f, db_path, memory) = make_env().await;
        let id = committee_store::insert(&db_path, "alice", "q", &[], "r")
            .await
            .unwrap();

        let res = handle_outcome(&db_path, memory, "alice", Some(&format!("{id} maybe"))).await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn rejects_unknown_id() {
        let (_f, db_path, memory) = make_env().await;
        let res = handle_outcome(&db_path, memory, "alice", Some("999 helpful")).await;
        assert!(res.is_err());
    }
}
