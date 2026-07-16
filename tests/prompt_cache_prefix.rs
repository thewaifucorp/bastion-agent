//! D-12/D-13/D-14b — byte-stable-prefix regression test.
//!
//! `AgentLoop::build_system_prompt` assembles the system prompt from
//! `DEFAULT_SYSTEM_PROMPT` + an ordered `Vec<Box<dyn TurnContextProvider>>`. The first
//! two providers (`DEFAULT_SYSTEM_PROMPT` itself + `IdentityProvider`'s block) are
//! turn-invariant; everything after (`ProceduralBeliefProvider`, the opt-in
//! `MemoryRagProvider`, the post-construction `MeshSliceProvider`) is turn-scoped and
//! legitimately varies. This test proves that STABLE prefix stays byte-identical across
//! two turns even when the volatile portion genuinely differs — the regression guard
//! RESEARCH.md flagged as missing (a test, not just a hope).
//!
//! Because `build_system_prompt` itself is private to `src/agent/loop_.rs`, and
//! `#[cfg(test)]` items are invisible to integration test binaries (they link the
//! crate's normal, non-`cfg(test)` build — same limitation already hit by Plan 08-08's
//! `fallback_resolver_override`), this test uses the `pub` seam
//! `AgentLoop::build_system_prompt_parts` which returns the pre-join `Vec<String>`.
//!
//! RESEARCH.md Open Question 3 diagnostic (Task 3) lives in this same file — it reuses
//! the stable-prefix bytes this test isolates.

use async_trait::async_trait;
use bastion_cognition::goal::{GoalEngine, ScoringConfig};
use bastion_mcp::McpClient;
use bastion_memory::sqlite::SqliteMemory;
use bastion_memory::{BeliefDraft, Memory, PrivacyTier, SharedMemory};
use bastion_personas::persona::{Persona, PersonaRegistry, PersonaResponder};
use bastion_providers::{Provider, SharedProvider};
use bastion_runtime::agent::loop_::{AgentLoop, DEFAULT_OWNER};
use bastion_runtime::capability::approval::SqliteApprovalGate;
use bastion_runtime::session::SessionManager;
use bastion_types::{CallConfig, LlmResponse, Message, TokenUsage};
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::NamedTempFile;
use tokio::sync::RwLock;

/// Number of stable (turn-invariant) entries at the front of `build_system_prompt_parts`'
/// return value: index 0 = `DEFAULT_SYSTEM_PROMPT`, index 1 = `IdentityProvider`'s block.
/// See the rustdoc on `AgentLoop::build_system_prompt` for the full ordering contract.
const STABLE_PREFIX_LEN: usize = 2;

/// Zero-network mock: always returns a fixed response, never touches the network.
struct MockProvider;

#[async_trait]
impl Provider for MockProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _config: &CallConfig,
    ) -> anyhow::Result<LlmResponse> {
        Ok(LlmResponse {
            text: "mock-response".into(),
            tool_calls: None,
            usage: TokenUsage::default(),
        })
    }
    async fn complete_simple(&self, _prompt: &str) -> anyhow::Result<String> {
        Ok("mock-simple".into())
    }
    fn context_limit(&self) -> usize {
        8192
    }
    fn model_name(&self) -> &str {
        "mock-model"
    }
    fn name(&self) -> &'static str {
        "mock"
    }
}

fn make_registry() -> PersonaRegistry {
    let mut personas = HashMap::new();
    personas.insert(
        "TestPersona".to_string(),
        Persona {
            name: "TestPersona".to_string(),
            description: Some("Test persona".to_string()),
            system_prompt: "You are TestPersona.".to_string(),
            tier: PrivacyTier::CloudOk,
            weight: 0.8,
            skills: vec![],
        },
    );
    PersonaRegistry::new_from_map(personas)
}

async fn make_agent(db_path: &str) -> (AgentLoop, SharedMemory) {
    let session = SessionManager::new(db_path);
    session.init_schema().await.expect("init_schema");
    session.create_session().await.expect("create_session");

    let memory: SharedMemory = Arc::new(RwLock::new(
        Box::new(SqliteMemory::new(db_path)) as Box<dyn Memory>
    ));

    // connect_from_config with an empty map returns a zero-tool client — no
    // network I/O, mirrors the existing test convention in src/agent/loop_.rs::tests.
    let mcp = Arc::new(
        McpClient::connect_from_config(&std::collections::HashMap::new())
            .await
            .expect("empty MCP config"),
    );

    let provider: SharedProvider =
        Arc::new(RwLock::new(Box::new(MockProvider) as Box<dyn Provider>));

    let agent = AgentLoop::new(
        provider,
        SessionManager::new(db_path),
        Arc::new(bastion_mcp::McpToolSource::new(mcp)),
        session.create_session().await.expect("create_session 2"),
        10.0,
        Arc::new(PersonaResponder::new(make_registry())),
        memory.clone(),
        Some(Arc::new(GoalEngine::new(db_path, ScoringConfig::default()))),
        vec![],
        Arc::new(SqliteApprovalGate::new(db_path)),
        Arc::new(bastion_cognition::eval::failure_sink::EvalFailureSink),
        bastion::agent::default_context_providers(&memory),
        Arc::new(bastion_providers::registry::RegistryProviderResolver),
        Some(Arc::new(bastion_cognition::agent::dream::DreamFlush::new(
            memory.clone(),
        ))),
        Some(Arc::new(bastion::agent::skills::SkillReloadObserver)),
    );

    (agent, memory)
}

/// D-12/D-13/D-14b: the stable prefix (`DEFAULT_SYSTEM_PROMPT` + `IdentityProvider`'s
/// block) must be byte-identical across two turns even when a genuinely new procedural
/// belief (turn-scoped `ProceduralBeliefProvider` content) is injected between the two
/// calls, and the two full turn messages differ.
#[tokio::test]
async fn stable_prefix_byte_identical_across_turns_with_different_volatile_content() {
    let f = NamedTempFile::new().unwrap();
    let path = f.path().to_str().unwrap().to_owned();
    let (agent, memory) = make_agent(&path).await;

    // Turn 1: no procedural beliefs exist yet — ProceduralBeliefProvider returns [].
    let parts1 = agent
        .build_system_prompt_parts(DEFAULT_OWNER, "what is the weather today", None)
        .await;

    // Inject a NEW procedural belief between the two calls — this is the turn-scoped
    // "recall/RAG result" the plan requires to genuinely differ between calls.
    {
        let mem = memory.read().await;
        mem.store_procedural_belief(BeliefDraft {
            owner_id: DEFAULT_OWNER.to_string(),
            persona_tag: None,
            issue: None,
            insight: "When asked about deployments, always confirm the target environment first."
                .to_string(),
            keywords: vec!["deploy".to_string()],
            session_id: "prompt-cache-prefix-test".to_string(),
            source: "test".to_string(),
            tier: Some(PrivacyTier::CloudOk),
        })
        .await
        .expect("store_procedural_belief");
    }

    // Turn 2: a completely different user message, AND a procedural belief now exists —
    // the volatile portion of the prompt must differ from turn 1's.
    let parts2 = agent
        .build_system_prompt_parts(DEFAULT_OWNER, "please deploy the new release", None)
        .await;

    assert!(
        parts1.len() >= STABLE_PREFIX_LEN && parts2.len() >= STABLE_PREFIX_LEN,
        "both turns must produce at least the stable prefix; got parts1={parts1:?} parts2={parts2:?}"
    );

    // The STABLE prefix must be byte-identical across both turns.
    assert_eq!(
        parts1[0..STABLE_PREFIX_LEN],
        parts2[0..STABLE_PREFIX_LEN],
        "stable prefix (DEFAULT_SYSTEM_PROMPT + IdentityProvider block) must be byte-identical \
         across turns with different volatile content — this is the D-12/D-14b cache-hit guarantee"
    );

    // Sanity: the FULL joined prompt must actually differ — otherwise this test would be
    // vacuously true (no volatile content ever varied).
    assert_ne!(
        parts1.join("\n\n"),
        parts2.join("\n\n"),
        "full system prompt must differ once a new procedural belief is injected — otherwise \
         this test isn't proving anything about the volatile/stable split"
    );

    // The new procedural belief's content must appear only in turn 2's volatile tail.
    assert!(
        !parts1
            .join("\n\n")
            .contains("always confirm the target environment"),
        "turn 1 (before the belief was stored) must not contain the not-yet-stored belief"
    );
    assert!(
        parts2
            .join("\n\n")
            .contains("always confirm the target environment"),
        "turn 2 (after the belief was stored) must contain the newly stored procedural belief"
    );
}

/// RESEARCH.md Open Question 3 diagnostic: is the combined stable prefix (default prompt +
/// identity block) large enough to cross Anthropic's ~1024-token cacheable minimum?
///
/// This is a DIAGNOSTIC, not a gate — it never fails on a below-threshold count. It exists
/// so a below-threshold prefix on Anthropic (which would silently manifest as
/// `cache_read == 0` even when correctly wired) is a KNOWN, logged condition instead of a
/// mystery to re-investigate later.
#[tokio::test]
async fn stable_prefix_token_count_diagnostic() {
    let f = NamedTempFile::new().unwrap();
    let path = f.path().to_str().unwrap().to_owned();
    let (agent, _memory) = make_agent(&path).await;

    let parts = agent
        .build_system_prompt_parts(DEFAULT_OWNER, "hello", None)
        .await;
    assert!(
        parts.len() >= STABLE_PREFIX_LEN,
        "expected at least the stable prefix"
    );

    let stable_prefix = parts[0..STABLE_PREFIX_LEN].join("\n\n");
    let count = approx_token_count(&stable_prefix);

    const ANTHROPIC_CACHEABLE_MIN_TOKENS: usize = 1024;
    eprintln!(
        "stable prefix approx_token_count={count} (Anthropic cacheable min ~{ANTHROPIC_CACHEABLE_MIN_TOKENS})"
    );
    if count < ANTHROPIC_CACHEABLE_MIN_TOKENS {
        eprintln!(
            "WARN: stable prefix likely below Anthropic's cacheable minimum — cache_read may \
             read 0 on Anthropic even when correctly wired; this is expected, not a bug — see \
             RESEARCH.md Open Question 3"
        );
    }

    // Diagnostic only: never fails on the count itself. Only assert the function ran and
    // produced a sane usize (no panic on empty/unicode input, per Task 3's behavior spec).
    assert_eq!(approx_token_count(""), 0);
    let _ = approx_token_count("héllo wörld — ünïcödé");
}

/// Rough English-text heuristic: ~4 chars/token. NOT a real tokenizer and NOT used for
/// billing/budget accuracy (that remains the provider's own reported `usage.input_tokens`)
/// — this is a diagnostic-only approximation for RESEARCH.md Open Question 3.
fn approx_token_count(s: &str) -> usize {
    s.len() / 4
}
