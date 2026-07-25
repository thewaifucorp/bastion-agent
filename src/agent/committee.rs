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

use bastion_cognition::cabinet::{build_table, orchestrator, synth};
use bastion_personas::persona::{runner, PersonaRegistry};
use bastion_providers::SharedProvider;
use bastion_types::{
    CallConfig, ConveneReason, Message, MessageContent, ResponseMode, RouterDecision, Role,
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
pub async fn handle(
    registry: &PersonaRegistry,
    provider: SharedProvider,
    capability_registry: &mut bastion_runtime::capability::CapabilityRegistry,
    arg: Option<&str>,
    owner: &str,
) -> anyhow::Result<String> {
    let question = arg.unwrap_or("").trim();
    if question.is_empty() {
        return Ok("usage: /committee <pergunta>".to_string());
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
    let stage1_transcript =
        orchestrator::deliberate(&stage1_table, provider.clone(), 1, capability_registry, question)
            .await?;

    report.push_str("## Estágio 1 — sinais independentes\n");
    let mut signals: Vec<(String, Signal)> = Vec::new();
    for turn in &stage1_transcript {
        match parse_signal(&turn.text) {
            Some(sig) => {
                report.push_str(&format!(
                    "- **{}**: {} (confiança: {})\n",
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

    let candidate_text: String = if disagree {
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
        let stage2_table =
            build_table(|name| registry.get(name).cloned(), &stage2_decision, None)?;
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
            report.push_str(&format!("[{}] ({:?}): {}\n\n", turn.persona, turn.kind, turn.text));
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
                report.push_str(&format!("\n**Síntese do debate**: {}\n", verdict.recommendation));
                if !verdict.dissents.is_empty() {
                    report.push_str("Dissidências remanescentes:\n");
                    for d in &verdict.dissents {
                        report.push_str(&format!("  - {}: {}\n", d.persona, d.position));
                    }
                }
                verdict.recommendation
            }
            Err(e) => {
                report.push_str(&format!(
                    "\n(síntese do debate falhou: {e} — seguindo com as posições brutas do debate)\n"
                ));
                stage2_transcript
                    .iter()
                    .map(|t| format!("[{}]: {}", t.persona, t.text))
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        }
    } else {
        let agreed_signal = normalized.iter().next().cloned().unwrap_or_default();
        report.push_str(&format!(
            "\nSinais concordam ({agreed_signal} — {} persona(s)) — pula direto pro Risk Manager, \
             Estágio 2 não foi acionado.\n",
            signals.len()
        ));
        signals
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
            .join("\n")
    };

    // --- Stage 3: Risk Manager, single dispatch (not Cabinet). ---
    report.push_str("\n## Estágio 3 — Risk Manager\n");
    let risk_prompt = format!(
        "Pergunta original do comitê: {question}\n\nCandidato(s) produzido(s) pelo comitê até \
         aqui:\n{candidate_text}\n\nAplique seu filtro de risco (limite de posição) sobre a(s) \
         proposta(s) acima."
    );
    let risk_text = run_single_persona(registry, provider.clone(), RISK_MANAGER, &risk_prompt).await?;
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
        other => anyhow::bail!("expected RunnerOutput::Single for a Single-mode decision, got {other:?}"),
    }
}
