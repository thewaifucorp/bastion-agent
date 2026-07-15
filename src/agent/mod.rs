pub mod command;
pub mod skills;

// M3: the M2 re-export shim that used to live here is gone — every consumer
// now names its real crate directly (`bastion_runtime::agent::*`,
// `bastion_cognition::agent::*`). `default_context_providers` below still
// needs `context`/`identity`/`memory_rag`/`procedural` in scope; these are
// private imports (not `pub use`) so they no longer leak old paths.
use bastion_cognition::agent::{identity, memory_rag, procedural};
use bastion_memory::SharedMemory;
use bastion_runtime::agent::context;

/// Product-side composition of the default SEAM #2 context providers (M2 step
/// 3b, decision D2): moved VERBATIM out of `AgentLoop::new`, which no longer
/// instantiates cognition types (`IdentityProvider`, `MemoryRagProvider`,
/// `ProceduralBeliefProvider`). The composition root (`main.rs`, and every
/// test fixture that previously relied on the constructor doing this) builds
/// this `Vec` and passes it to the constructor's `context_providers` argument.
///
/// Ordering is load-bearing (D-12/D-14b byte-stable prompt-cache prefix — see
/// `AgentLoop::build_system_prompt`): `IdentityProvider` FIRST (turn-invariant
/// stable prefix), then the turn-scoped providers.
pub fn default_context_providers(
    memory: &SharedMemory,
) -> Vec<Box<dyn context::TurnContextProvider>> {
    let mut providers: Vec<Box<dyn context::TurnContextProvider>> = Vec::new();

    // M1: registrar IdentityProvider para injeção do bloco de identidade via SEAM #2.
    // No primeiro uso retorna o ONBOARDING_PROMPT; nos subsequentes retorna o bloco gravado.
    providers.push(Box::new(identity::IdentityProvider::new(memory.clone())));

    // SEAM #2 — MemoryRagProvider: recall de beliefs por injeção (perna "RAG" do
    // BIG-1, decisão de híbrido ainda pendente → opt-in). Funciona com qualquer
    // provider — incluindo terminal-agents (PROV-09) que nunca emitem tool_calls —
    // e é egress-safe: blocos separados por tier, build_system_prompt derruba
    // por bloco. Default-off porque providers com function-calling já recebem as
    // tools de memória (injetar também duplicaria exposição e cresce o prompt).
    let memory_rag_on = std::env::var("BASTION_MEMORY_RAG")
        .is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
    if memory_rag_on {
        providers.push(Box::new(memory_rag::MemoryRagProvider::new(memory.clone())));
        tracing::info!(event = "memory_rag_enabled");
    }

    // LEARN-03 — ProceduralBeliefProvider: recall de beliefs PROCEDURAIS (kind=
    // 'procedural') por injeção de contexto, mesma mecânica de MemoryRagProvider
    // (tier-split, egress-safe por bloco). Always-on (não gated por env, ao
    // contrário do BASTION_MEMORY_RAG acima): procedural é entregável de primeira
    // classe da Fase 7, não uma perna experimental do RAG híbrido do BIG-1.
    providers.push(Box::new(procedural::ProceduralBeliefProvider::new(
        memory.clone(),
    )));
    tracing::info!(event = "procedural_belief_provider_enabled");

    providers
}
