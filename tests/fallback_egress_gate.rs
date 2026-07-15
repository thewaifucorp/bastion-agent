//! Regression tests for WR-04: check_egress gate in provider fallback path.
//! These are integration-level smoke tests that verify the public egress contract.
//! The full (tier × provider) matrix is in src/hooks/egress.rs unit tests.

use bastion_memory::PrivacyTier;
use bastion_runtime::hooks::egress::check_egress;

#[test]
fn fallback_egress_gate_local_only_blocks_cloud() {
    // WR-04: LocalOnly + cloud provider must be blocked (fail-closed)
    let cloud_providers = ["anthropic", "openai", "gemini", "openrouter"];
    for provider in cloud_providers {
        let result = check_egress(Some(PrivacyTier::LocalOnly), provider);
        assert!(
            result.is_err(),
            "LocalOnly should block cloud provider '{}' but got Ok",
            provider
        );
    }
}

#[test]
fn fallback_egress_gate_local_only_allows_ollama() {
    // WR-04: LocalOnly + ollama is the only permitted combination for local-only personas
    let result = check_egress(Some(PrivacyTier::LocalOnly), "ollama");
    assert!(
        result.is_ok(),
        "LocalOnly should allow ollama but got Err: {:?}",
        result
    );
}

#[test]
fn fallback_egress_gate_cloud_ok_allows_all() {
    // WR-04: CloudOk tier allows any provider (persona explicitly consented to cloud)
    let all_providers = ["ollama", "anthropic", "openai", "gemini", "openrouter"];
    for provider in all_providers {
        let result = check_egress(Some(PrivacyTier::CloudOk), provider);
        assert!(
            result.is_ok(),
            "CloudOk should allow provider '{}' but got Err: {:?}",
            provider,
            result
        );
    }
}

#[test]
fn fallback_egress_gate_none_tier_blocks_all() {
    // WR-04: None tier (unknown / untagged — default when no forced persona) is fail-closed
    // This covers the case where run_provider_fallback is called with no active persona.
    let all_providers = ["ollama", "anthropic", "openai", "gemini", "openrouter"];
    for provider in all_providers {
        let result = check_egress(None, provider);
        assert!(
            result.is_err(),
            "None tier should block all providers (fail-closed) but '{}' returned Ok",
            provider
        );
    }
}
