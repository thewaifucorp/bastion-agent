//! Bridge durable Pursue attempts to the owner-scoped procedural-memory loop.
use bastion_cognition::agent::procedural_outcome::ProceduralLearner;
use bastion_memory::{BeliefKind, PrivacyTier, SharedMemory};
use bastion_runtime::task::{BeliefRef, TaskCase, TaskCaseId, TaskStore, VerificationStatus};
use serde_json::{json, Value};
use std::sync::Arc;

const MAX_BELIEFS: usize = 4;
const MIN_TERM_LEN: usize = 4;

fn overlap(query: &str, content: &str) -> usize {
    let content = content.to_lowercase();
    query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|term| term.chars().count() >= MIN_TERM_LEN)
        .filter(|term| content.contains(&term.to_lowercase()))
        .count()
}

fn score(weight: f64, utility: f64, confidence: f64) -> f64 {
    weight * (1.0 + utility * confidence)
}

/// Select only CloudOk procedural beliefs before a delegated runtime starts.
pub async fn state_for_pursue(memory: &SharedMemory, owner: &str, objective: &str) -> Value {
    let beliefs = {
        let mem = memory.read().await;
        match mem.retrieve_tagged(owner, None).await {
            Ok(beliefs) => beliefs,
            Err(error) => {
                tracing::warn!(event = "pursue_procedural_retrieve_failed", %error);
                return Value::Null;
            }
        }
    };
    let mut beliefs: Vec<_> = beliefs
        .into_iter()
        .filter(|belief| belief.kind == BeliefKind::Procedural)
        .filter(|belief| belief.tier == Some(PrivacyTier::CloudOk))
        .collect();
    beliefs.sort_by(|a, b| {
        overlap(objective, &b.content)
            .cmp(&overlap(objective, &a.content))
            .then_with(|| {
                score(b.weight, b.utility(), b.confidence())
                    .partial_cmp(&score(a.weight, a.utility(), a.confidence()))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then(b.id.cmp(&a.id))
    });
    beliefs.truncate(MAX_BELIEFS);
    if beliefs.is_empty() {
        return Value::Null;
    }
    json!({
        "procedural_belief_refs": beliefs.iter().map(|b| b.id.to_string()).collect::<Vec<_>>(),
        "procedural_guidance": beliefs.iter().map(|b| format!("- [belief {}] {}", b.id, b.content)).collect::<Vec<_>>().join("\n"),
    })
}

pub fn belief_refs(case: &TaskCase) -> Vec<BeliefRef> {
    case.business_state
        .0
        .get("procedural_belief_refs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|id| BeliefRef(id.to_string()))
        .collect()
}

pub fn prompt(case: &TaskCase) -> String {
    match case
        .business_state
        .0
        .get("procedural_guidance")
        .and_then(Value::as_str)
    {
        Some(guidance) => format!(
            "{}\n\n<procedural_guidance>\n{}\n</procedural_guidance>",
            case.frame.objective, guidance
        ),
        None => case.frame.objective.clone(),
    }
}

/// Attribute only final verified attempt: a failed retry is not rewarded by a later success.
pub async fn attribute_terminal(
    memory: &SharedMemory,
    store: &Arc<dyn TaskStore>,
    owner: &str,
    task: &TaskCaseId,
) -> anyhow::Result<()> {
    let Some(mut case) = store.load_case(owner, task).await? else {
        return Ok(());
    };
    if !case.is_terminal()
        || case
            .business_state
            .0
            .get("procedural_attributed")
            .and_then(Value::as_bool)
            == Some(true)
    {
        return Ok(());
    }
    let Some(attempt) = store
        .list_attempts_for_case(owner, task)
        .await?
        .into_iter()
        .rev()
        .find(|a| a.verdict.is_some())
    else {
        return Ok(());
    };
    let Some(verdict) = attempt.verdict else {
        return Ok(());
    };
    if verdict.status == VerificationStatus::Unverified {
        return Ok(());
    }
    let ids = attempt
        .belief_refs
        .iter()
        .filter_map(|r| r.as_str().parse().ok())
        .collect::<Vec<i64>>();
    ProceduralLearner::new(memory.clone())
        .attribute(owner, &ids, verdict.status)
        .await?;

    // A restarted driver may observe the same terminal task again. Persisting
    // this marker makes reinforcement exactly-once per durable task.
    if let Some(state) = case.business_state.0.as_object_mut() {
        state.insert("procedural_attributed".to_string(), json!(true));
    } else {
        case.business_state.0 = json!({ "procedural_attributed": true });
    }
    store.update_case(&case, case.revision).await?;
    Ok(())
}
