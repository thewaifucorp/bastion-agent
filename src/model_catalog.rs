//! A4 S2: the model catalog — the daemon's own answer to "which models can
//! I pick, and which provider serves each one".
//!
//! Nothing anywhere (here or in bastion-core) previously enumerated models:
//! `bastion_providers::registry::resolve_provider` routes any model NAME to
//! a provider by prefix, and every UI so far made the operator type a bare
//! id. This module gives `GET /models` / `GET /providers` (src/loadout.rs)
//! a curated STATIC table of reasonable, current model ids per provider
//! kind, then merges in whatever bastion.toml / the config-store overrides
//! actually name (`default_model`, `fallback_models`) so a custom or
//! brand-new model id always appears — the static table is a convenience,
//! never a gate (exactly like the registry itself: unknown ids still route
//! by prefix).
//!
//! Classification is delegated to
//! [`bastion_providers::registry::resolve_provider_kind`] — the SAME prefix
//! rules `resolve_provider` uses — so the catalog can never drift from what
//! the runtime would actually do with an id.
//!
//! OpenRouter note: OpenRouter is a passthrough — ANY `vendor/model[:tag]`
//! slug routes to it, so its static entries below are a small courtesy
//! sample, not an inventory; the merge step is what surfaces the slugs an
//! operator actually configured.

use bastion_providers::registry::resolve_provider_kind;
use serde::Serialize;

/// One catalog row. `provider_kind` always agrees with
/// [`resolve_provider_kind`] over `id` (asserted in tests).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelEntry {
    pub id: String,
    pub provider_kind: String,
    pub display_name: String,
}

/// One API-key provider: its stable id (also its `resolve_provider_kind`
/// name), the env key its bastion-core provider constructor reads, and the
/// human name `GET /providers` reports (S4 cleanup: previously a frontend
/// mirror in `web/src/views/Providers.tsx` — the daemon's whitelist is now
/// the single source).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApiKeyProvider {
    pub id: &'static str,
    pub env_key: &'static str,
    pub display_name: &'static str,
}

/// The API-key providers the registry can route to, in display order. The
/// env keys mirror what each `bastion-providers` constructor reads
/// (`AnthropicProvider::new` → `ANTHROPIC_API_KEY`, ...). Ollama is absent
/// on purpose: it is local (no key), reported with kind `local` by
/// `GET /providers`.
pub const API_KEY_PROVIDERS: &[ApiKeyProvider] = &[
    ApiKeyProvider {
        id: "anthropic",
        env_key: "ANTHROPIC_API_KEY",
        display_name: "Anthropic",
    },
    ApiKeyProvider {
        id: "openai",
        env_key: "OPENAI_API_KEY",
        display_name: "OpenAI",
    },
    ApiKeyProvider {
        id: "gemini",
        env_key: "GEMINI_API_KEY",
        display_name: "Google Gemini",
    },
    ApiKeyProvider {
        id: "groq",
        env_key: "GROQ_API_KEY",
        display_name: "Groq",
    },
    ApiKeyProvider {
        id: "openrouter",
        env_key: "OPENROUTER_API_KEY",
        display_name: "OpenRouter",
    },
];

/// Human name of the local (keyless) provider — `GET /providers`' ollama row.
pub const OLLAMA_DISPLAY_NAME: &str = "Ollama";

/// Provider kinds in the order `GET /models` groups them.
pub const PROVIDER_KIND_ORDER: &[&str] =
    &["anthropic", "openai", "gemini", "groq", "openrouter", "ollama"];

/// Env key an API-key provider reads, by provider id (`None` for ollama /
/// unknown ids). Also the whitelist `secret_set` proposals validate against.
pub fn env_key_for_provider(provider_id: &str) -> Option<&'static str> {
    API_KEY_PROVIDERS
        .iter()
        .find(|p| p.id == provider_id)
        .map(|p| p.env_key)
}

/// Whether `env_key` is one of the known provider env keys — `secret_set`
/// proposals may only ever target these files, never arbitrary names.
pub fn is_known_provider_env_key(env_key: &str) -> bool {
    API_KEY_PROVIDERS.iter().any(|p| p.env_key == env_key)
}

/// (id, display_name) — kind is derived via [`resolve_provider_kind`], so a
/// typo'd id that would route elsewhere fails the agreement test instead of
/// shipping a lie.
const STATIC_MODELS: &[(&str, &str)] = &[
    // anthropic — the Claude family (prefix `claude`)
    ("claude-opus-4-5", "Claude Opus 4.5"),
    ("claude-sonnet-4-5", "Claude Sonnet 4.5"),
    ("claude-haiku-4-5", "Claude Haiku 4.5"),
    // openai — GPT/o-series (prefixes `gpt`/`o1`/`o3`)
    ("gpt-5.1", "GPT-5.1"),
    ("gpt-5", "GPT-5"),
    ("gpt-5-mini", "GPT-5 mini"),
    ("gpt-4.1", "GPT-4.1"),
    ("o3", "OpenAI o3"),
    // gemini (prefix `gemini`)
    ("gemini-3-pro-preview", "Gemini 3 Pro (preview)"),
    ("gemini-2.5-pro", "Gemini 2.5 Pro"),
    ("gemini-2.5-flash", "Gemini 2.5 Flash"),
    ("gemini-2.5-flash-lite", "Gemini 2.5 Flash Lite"),
    // groq-hosted (prefix `groq/`, stripped before the upstream call)
    ("groq/llama-3.3-70b-versatile", "Llama 3.3 70B (Groq)"),
    ("groq/openai/gpt-oss-120b", "GPT-OSS 120B (Groq)"),
    ("groq/qwen/qwen3-32b", "Qwen3 32B (Groq)"),
    ("groq/moonshotai/kimi-k2-instruct", "Kimi K2 (Groq)"),
    // openrouter — passthrough sample (any other `vendor/model[:tag]` slug)
    ("deepseek/deepseek-chat-v3.1:free", "DeepSeek Chat v3.1 (free)"),
    ("meta-llama/llama-3.3-70b-instruct:free", "Llama 3.3 70B Instruct (free)"),
    ("qwen/qwen3-coder:free", "Qwen3 Coder (free)"),
    // ollama — common local pulls (any bare id with no known prefix)
    ("llama3.2", "Llama 3.2 (local)"),
    ("qwen3", "Qwen3 (local)"),
    ("gemma3", "Gemma 3 (local)"),
    ("mistral", "Mistral (local)"),
    ("deepseek-r1", "DeepSeek R1 (local)"),
];

/// The curated static table, classified through the registry's own rules.
pub fn static_catalog() -> Vec<ModelEntry> {
    STATIC_MODELS
        .iter()
        .map(|(id, display_name)| ModelEntry {
            id: (*id).to_string(),
            provider_kind: resolve_provider_kind(id).to_string(),
            display_name: (*display_name).to_string(),
        })
        .collect()
}

/// Static catalog + every `configured` model id not already in it, so the
/// ids bastion.toml / the config-store overrides actually name (custom,
/// niche, or newer than this table) always appear. Blank ids are skipped;
/// duplicates keep the first (static) entry; the `claude_code`/`opencode`
/// bridge names are excluded — the registry classifies them
/// `agent_runtime` (`terminal_agent` on newer core revs) and refuses to
/// resolve them as model providers, so they are not /model-selectable.
pub fn merged_catalog<'a>(configured: impl IntoIterator<Item = &'a str>) -> Vec<ModelEntry> {
    let mut entries = static_catalog();
    for id in configured {
        let id = id.trim();
        if id.is_empty() || entries.iter().any(|e| e.id == id) {
            continue;
        }
        let kind = resolve_provider_kind(id);
        if kind == "agent_runtime" || kind == "terminal_agent" {
            continue;
        }
        entries.push(ModelEntry {
            id: id.to_string(),
            provider_kind: kind.to_string(),
            // No curated pretty name for a custom id — the id IS the name.
            display_name: id.to_string(),
        });
    }
    entries
}

/// How many catalog models a provider kind serves — `GET /providers`'s
/// `models_count`.
pub fn count_for_kind(entries: &[ModelEntry], kind: &str) -> usize {
    entries.iter().filter(|e| e.provider_kind == kind).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_catalog_kinds_agree_with_the_registry() {
        for entry in static_catalog() {
            assert_eq!(
                entry.provider_kind,
                resolve_provider_kind(&entry.id),
                "static entry '{}' claims a kind the registry would not route it to",
                entry.id
            );
            assert!(
                entry.provider_kind != "agent_runtime" && entry.provider_kind != "terminal_agent",
                "the runtime bridge names must never be catalog entries"
            );
        }
    }

    #[test]
    fn static_catalog_covers_every_provider_kind_once_at_least() {
        let entries = static_catalog();
        for kind in PROVIDER_KIND_ORDER {
            assert!(
                count_for_kind(&entries, kind) > 0,
                "no static model for provider kind '{kind}'"
            );
        }
    }

    #[test]
    fn merge_appends_configured_ids_not_in_the_static_table() {
        let entries = merged_catalog(["my-org/custom-model:free", "llama3.2"]);
        let custom = entries
            .iter()
            .find(|e| e.id == "my-org/custom-model:free")
            .expect("custom id must be merged in");
        assert_eq!(custom.provider_kind, "openrouter");
        assert_eq!(custom.display_name, "my-org/custom-model:free");
        // llama3.2 is already static — no duplicate row.
        assert_eq!(entries.iter().filter(|e| e.id == "llama3.2").count(), 1);
    }

    #[test]
    fn merge_skips_blank_and_terminal_agent_ids() {
        let base = static_catalog().len();
        let entries = merged_catalog(["", "   ", "claude_code", "opencode"]);
        assert_eq!(entries.len(), base);
    }

    #[test]
    fn env_key_mapping_is_the_secret_set_whitelist() {
        assert_eq!(env_key_for_provider("anthropic"), Some("ANTHROPIC_API_KEY"));
        assert_eq!(
            env_key_for_provider("openrouter"),
            Some("OPENROUTER_API_KEY")
        );
        assert_eq!(env_key_for_provider("ollama"), None);
        assert!(is_known_provider_env_key("GEMINI_API_KEY"));
        assert!(!is_known_provider_env_key("PATH"));
        assert!(!is_known_provider_env_key("MY_RANDOM_KEY"));
    }
}
