//! `GET /loadout` — the daemon's assembled composition, for the web app's
//! Loadout view ("your bastion, piece by piece").
//!
//! Bastion's identity is *authority explicit*: you should be able to SEE
//! what your agent is assembled from and what each piece may do. This route
//! answers with the composition snapshot taken at boot — personas loaded
//! from `./personas/`, tools in the shared `CapabilityRegistry`, coding
//! runtimes, enabled channels, configured MCP servers, and installed
//! extension packs (honest empty until the `ExtensionHost` is wired into
//! the daemon: mechanism exists, product wiring is backlog).
//!
//! Owner-token authenticated (same `resolve_owner_or_401` as `/webhook`):
//! the composition fingerprints an installation — it is for its operator,
//! not the network. Snapshot semantics: values are captured when the daemon
//! boots; `POST /lifecycle/reload` reloads personas from disk for command
//! validation but this snapshot refreshes on restart (field `captured_at`
//! makes staleness visible instead of pretending liveness).

use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use bastion_types::SecretResolver;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::tui::CompanionHandle;

use crate::channel::webhook::resolve_owner_or_401;
use crate::channel::OwnerMap;
use crate::config::{AuthConfig, AuthProfileEntry};
use crate::config_store::{
    fallback_models_from_value_json, model_from_value_json, ConfigStore, KEY_MODEL_FALLBACKS,
    KEY_MODEL_SELECTED,
};
use crate::model_catalog::{self, ModelEntry};
use crate::proposals::{self, PendingSecretValues, ProposalPayload, SqliteProposalStore};
use std::sync::Arc;

#[derive(Clone, Serialize)]
pub struct ChannelPiece {
    pub id: &'static str,
    pub enabled: bool,
}

#[derive(Clone, Serialize)]
pub struct RuntimePiece {
    pub id: String,
}

/// The assembled composition, captured at boot in `daemon_loop`.
#[derive(Clone, Serialize)]
pub struct LoadoutSnapshot {
    pub personas: Vec<String>,
    pub tools: Vec<String>,
    pub runtimes: Vec<RuntimePiece>,
    pub channels: Vec<ChannelPiece>,
    pub mcp_servers: Vec<String>,
    /// Always empty today: the sandboxed `ExtensionHost` mechanism exists
    /// (`src/extension/`) but nothing installs packs into the running
    /// daemon yet — reported honestly rather than omitted.
    pub extensions: Vec<String>,
    /// Nanoseconds since epoch when this snapshot was captured (boot time).
    pub captured_at: i64,
}

/// The declarative model config from bastion.toml — the base the config
/// store's `model.selected` / `model.fallbacks` overrides overlay.
#[derive(Clone)]
pub struct ModelDefaults {
    pub default_model: String,
    pub fallback_models: Vec<String>,
}

#[derive(Clone)]
struct LoadoutState {
    snapshot: Arc<LoadoutSnapshot>,
    owner_map: Arc<OwnerMap>,
    jwt_secret: String,
    /// A3: staged configuration proposals (web proposes, console approves).
    proposal_store: Arc<SqliteProposalStore>,
    /// A4-U S1: unified runtime config overrides — `GET /config/overrides`
    /// reports the effective overlay (latest row per key) with provenance.
    config_store: ConfigStore,
    /// A4 S2: bastion.toml `default_model`/`fallback_models`, the base for
    /// `GET /models`' effective values and the catalog merge.
    model_defaults: Arc<ModelDefaults>,
    /// A4 S2: the `[auth.*]` table — `GET /providers` probes its `HostCli`
    /// profiles live (exit-code booleans only, same as `/status`).
    auth: Arc<AuthConfig>,
    /// A4 S2: in-memory holding pen for `secret_set` values (see
    /// `proposals::PendingSecretValues`) — shared with the console approve.
    pending_secrets: PendingSecretValues,
    /// A4.5: bastion.toml's `[routing]` table — the declarative base
    /// `GET /routing` overlays with the config store's `routing.rules`.
    routing_toml: Arc<std::collections::HashMap<String, String>>,
    events_tx: broadcast::Sender<String>,
    /// A5 S5: the daemon's shared companion handle — the SAME instance
    /// `CompanionEventCapability` mutates through, so the daemon stays a
    /// single writer of `companion.json` (see `tui::CompanionHandle`'s doc
    /// comment for the residual TUI-process race this does NOT close).
    companion: CompanionHandle,
}

fn auth(
    state: &LoadoutState,
    headers: &HeaderMap,
    event: &'static str,
) -> Result<String, Box<axum::response::Response>> {
    resolve_owner_or_401(headers, &state.owner_map, &state.jwt_secret, event)
}

async fn loadout_handler(
    State(state): State<LoadoutState>,
    headers: HeaderMap,
) -> axum::response::Response {
    if let Err(resp) = auth(&state, &headers, "loadout_unauthorized") {
        return *resp;
    }
    Json(state.snapshot.as_ref().clone()).into_response()
}

async fn personas_list_handler(
    State(state): State<LoadoutState>,
    headers: HeaderMap,
) -> axum::response::Response {
    if let Err(resp) = auth(&state, &headers, "personas_unauthorized") {
        return *resp;
    }
    let slugs = proposals::list_persona_slugs(&proposals::personas_root()).await;
    Json(serde_json::json!({ "items": slugs })).into_response()
}

async fn persona_read_handler(
    State(state): State<LoadoutState>,
    headers: HeaderMap,
    AxumPath(slug): AxumPath<String>,
) -> axum::response::Response {
    if let Err(resp) = auth(&state, &headers, "personas_unauthorized") {
        return *resp;
    }
    match proposals::read_persona(&proposals::personas_root(), &slug).await {
        Ok(Some(content)) => {
            Json(serde_json::json!({ "slug": slug, "content": content })).into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, "no such persona").into_response(),
        Err(_) => (StatusCode::BAD_REQUEST, "invalid persona slug").into_response(),
    }
}

async fn proposals_list_handler(
    State(state): State<LoadoutState>,
    headers: HeaderMap,
) -> axum::response::Response {
    let owner = match auth(&state, &headers, "proposals_unauthorized") {
        Ok(o) => o,
        Err(resp) => return *resp,
    };
    match state.proposal_store.list_for_owner(&owner).await {
        Ok(items) => Json(serde_json::json!({ "items": items })).into_response(),
        Err(e) => {
            tracing::warn!(event = "proposals_list_failed", error = %e);
            (StatusCode::INTERNAL_SERVER_ERROR, "proposal store error").into_response()
        }
    }
}

/// A4-U S1: the effective runtime config overlay — one row per key, the
/// latest audited apply wins. `value` is the parsed `value_json` (the same
/// JSON shape the legacy selection files held); `origin`/`applied_at` are
/// the provenance the audit table records.
async fn config_overrides_handler(
    State(state): State<LoadoutState>,
    headers: HeaderMap,
) -> axum::response::Response {
    if let Err(resp) = auth(&state, &headers, "config_overrides_unauthorized") {
        return *resp;
    }
    match state.config_store.all_latest().await {
        Ok(items) => {
            let items: Vec<serde_json::Value> = items
                .into_iter()
                .map(|o| {
                    serde_json::json!({
                        "key": o.key,
                        "value": serde_json::from_str::<serde_json::Value>(&o.value_json)
                            .unwrap_or(serde_json::Value::Null),
                        "origin": o.origin,
                        "applied_at": o.applied_at,
                    })
                })
                .collect();
            Json(serde_json::json!({ "items": items })).into_response()
        }
        Err(e) => {
            tracing::warn!(event = "config_overrides_list_failed", error = %e);
            (StatusCode::INTERNAL_SERVER_ERROR, "config store error").into_response()
        }
    }
}

/// A4 S2: the effective model config — config-store override when present,
/// bastion.toml otherwise. Store read errors degrade to the TOML base (the
/// same value a fresh install would report) rather than failing the read
/// route.
async fn effective_models(state: &LoadoutState) -> (String, Vec<String>) {
    let default_model = state
        .config_store
        .latest(KEY_MODEL_SELECTED)
        .await
        .ok()
        .flatten()
        .as_deref()
        .and_then(model_from_value_json)
        .unwrap_or_else(|| state.model_defaults.default_model.clone());
    let fallback_models = state
        .config_store
        .latest(KEY_MODEL_FALLBACKS)
        .await
        .ok()
        .flatten()
        .as_deref()
        .and_then(fallback_models_from_value_json)
        .unwrap_or_else(|| state.model_defaults.fallback_models.clone());
    (default_model, fallback_models)
}

/// Catalog merged with every model id this installation actually names —
/// the TOML base AND the effective overrides — so a custom id always shows.
fn merged_catalog_for(state: &LoadoutState, effective: &(String, Vec<String>)) -> Vec<ModelEntry> {
    let configured = std::iter::once(state.model_defaults.default_model.as_str())
        .chain(state.model_defaults.fallback_models.iter().map(String::as_str))
        .chain(std::iter::once(effective.0.as_str()))
        .chain(effective.1.iter().map(String::as_str));
    model_catalog::merged_catalog(configured)
}

/// A4 S2 `GET /providers`: connection status per provider, booleans only.
///
/// - `api_key` providers: `connected` = the env var is set non-empty OR a
///   `BASTION_SECRETS_DIR/<ENV_KEY>` file resolves — probed through the same
///   `secret::EnvSecretResolver`/`MountedFileSecretResolver` the daemon
///   boots with; the resolved value is dropped immediately, never inspected
///   or echoed. `source` says which layer answered (`env` wins, matching
///   `LayeredSecretResolver` order).
/// - `subscription_cli` (`[auth.*]` HostCli profiles): live
///   `probe_host_cli` exit-code probe, same as `/status` and `/connect` —
///   never account detail. `source` is `auth_profile` when connected.
/// - `ollama` (`local`): no network probe from a GET (reachability is only
///   knowable at request time) — `connected` here means "configured": some
///   effective model (default or fallback) routes to ollama. `source` stays
///   null.
async fn providers_handler(
    State(state): State<LoadoutState>,
    headers: HeaderMap,
) -> axum::response::Response {
    if let Err(resp) = auth(&state, &headers, "providers_unauthorized") {
        return *resp;
    }
    let effective = effective_models(&state).await;
    let catalog = merged_catalog_for(&state, &effective);
    let secrets_dir = std::env::var("BASTION_SECRETS_DIR").ok();

    let mut items = Vec::new();
    for p in model_catalog::API_KEY_PROVIDERS {
        // STRICTLY boolean: resolve, discard the value, keep only which
        // layer (if any) had it.
        let source = if crate::secret::EnvSecretResolver.resolve(p.env_key).is_ok() {
            Some("env")
        } else if secrets_dir.as_deref().is_some_and(|dir| {
            crate::secret::MountedFileSecretResolver::new(dir)
                .resolve(p.env_key)
                .is_ok()
        }) {
            Some("secrets_dir")
        } else {
            None
        };
        items.push(serde_json::json!({
            "id": p.id,
            // S4 cleanup: name + env key come from the daemon's own
            // whitelist (`model_catalog::API_KEY_PROVIDERS`) — the web app
            // consumes these instead of mirroring the table.
            "display_name": p.display_name,
            "env_key": p.env_key,
            "kind": "api_key",
            "connected": source.is_some(),
            "source": source,
            "models_count": model_catalog::count_for_kind(&catalog, p.id),
        }));
    }

    let ollama_configured = std::iter::once(effective.0.as_str())
        .chain(effective.1.iter().map(String::as_str))
        .any(|m| bastion_providers::registry::resolve_provider_kind(m) == "ollama");
    items.push(serde_json::json!({
        "id": "ollama",
        "display_name": model_catalog::OLLAMA_DISPLAY_NAME,
        // Local provider: no key, no env var — honest null.
        "env_key": serde_json::Value::Null,
        "kind": "local",
        "connected": ollama_configured,
        "source": serde_json::Value::Null,
        "models_count": model_catalog::count_for_kind(&catalog, "ollama"),
    }));

    // Deterministic order for the UI: profiles sorted by id.
    let mut cli_profiles: Vec<(&String, &String)> = state
        .auth
        .profiles
        .iter()
        .filter_map(|(id, entry)| match entry {
            AuthProfileEntry::HostCli { cli } => Some((id, cli)),
            AuthProfileEntry::ApiKey { .. } => None,
        })
        .collect();
    cli_profiles.sort();
    for (profile_id, cli) in cli_profiles {
        let connected = crate::auth_profile_registry::probe_host_cli(cli)
            .await
            .is_ok();
        items.push(serde_json::json!({
            "id": profile_id,
            // Auth profiles are operator-named — the id IS the name (the
            // web app displayed exactly that before this field existed).
            "display_name": profile_id,
            "env_key": serde_json::Value::Null,
            "kind": "subscription_cli",
            "connected": connected,
            "source": connected.then_some("auth_profile"),
            // Subscription CLIs bring their own entitlement, not catalog
            // models — the runtime backend (`/backend`), not `/model`,
            // selects what they run.
            "models_count": 0,
        }));
    }

    Json(serde_json::json!({ "items": items })).into_response()
}

/// A4 S2 `GET /models`: the merged catalog grouped by provider kind, plus
/// the EFFECTIVE default/fallbacks (config-store override else bastion.toml).
async fn models_handler(
    State(state): State<LoadoutState>,
    headers: HeaderMap,
) -> axum::response::Response {
    if let Err(resp) = auth(&state, &headers, "models_unauthorized") {
        return *resp;
    }
    let effective = effective_models(&state).await;
    let catalog = merged_catalog_for(&state, &effective);
    let providers: Vec<serde_json::Value> = model_catalog::PROVIDER_KIND_ORDER
        .iter()
        .map(|kind| {
            let models: Vec<&ModelEntry> = catalog
                .iter()
                .filter(|e| e.provider_kind == *kind)
                .collect();
            serde_json::json!({ "provider_kind": kind, "models": models })
        })
        .collect();
    Json(serde_json::json!({
        "providers": providers,
        "default_model": effective.0,
        "fallback_models": effective.1,
    }))
    .into_response()
}

/// A4.5 `GET /routing`: the effective routing table — all five call-site
/// classes, always, each with its effective model (config-store override
/// else `[routing]` toml else null), the layer that supplied it, and the
/// honest `supported` flag (`crate::routing` docs the knob per class).
async fn routing_handler(
    State(state): State<LoadoutState>,
    headers: HeaderMap,
) -> axum::response::Response {
    if let Err(resp) = auth(&state, &headers, "routing_unauthorized") {
        return *resp;
    }
    let table = crate::routing::load_table(&state.config_store, &state.routing_toml).await;
    Json(serde_json::json!({ "items": table.report() })).into_response()
}

/// A5 S5 `GET /companion`: the companion's current snapshot — level, XP,
/// need percents, due cues, and a static representative frame. Read-only;
/// always fresh, since every daemon-side mutation (`POST /companion/care`,
/// `CompanionEventCapability`) saves to disk synchronously through the same
/// `CompanionHandle`.
async fn companion_handler(
    State(state): State<LoadoutState>,
    headers: HeaderMap,
) -> axum::response::Response {
    if let Err(resp) = auth(&state, &headers, "companion_unauthorized") {
        return *resp;
    }
    Json(state.companion.snapshot()).into_response()
}

#[derive(Deserialize)]
struct CareRequest {
    action: String,
}

/// A5 S5 `POST /companion/care`: applies a care action through the shared
/// `CompanionHandle` (single writer while the daemon runs), persists, and
/// broadcasts `companion.updated` on `/events` — then answers with the
/// updated snapshot so the web Buddy view can render it without a second
/// round trip.
async fn companion_care_handler(
    State(state): State<LoadoutState>,
    headers: HeaderMap,
    Json(req): Json<CareRequest>,
) -> axum::response::Response {
    if let Err(resp) = auth(&state, &headers, "companion_care_unauthorized") {
        return *resp;
    }
    match state.companion.care(&req.action, &state.events_tx) {
        Ok(snapshot) => Json(snapshot).into_response(),
        Err(e) => {
            tracing::warn!(event = "companion_care_failed", error = %e);
            (StatusCode::BAD_REQUEST, e.to_string()).into_response()
        }
    }
}

/// `event`/`source` — same field names and validation contract as
/// `CompanionEventCapability`'s schema (`^[A-Za-z0-9._-]+$`, 1-32 chars for
/// `source`; `event` one of `session-start`/`activity`/`session-stop`),
/// enforced inside `CompanionHandle::record_event` so both the in-process
/// capability and this route reject the exact same bad input the exact same
/// way.
#[derive(Deserialize)]
struct CompanionEventRequest {
    event: String,
    source: String,
}

/// S6 `POST /companion/event`: the CLI (`bastion companion event`) and hook
/// bridges' HTTP counterpart to `CompanionEventCapability` — closes the S5
/// gap where the standalone CLI had no daemon-aware path and always wrote
/// `companion.json` directly, even with a daemon (and its capability)
/// already running as the single writer. Routes through the SAME
/// `CompanionHandle::record_event` the capability uses, persists, and
/// broadcasts `companion.updated` on `/events` — then answers with the
/// updated snapshot (plus the recorded-event message, for the CLI to print)
/// so a caller never needs a second round trip.
async fn companion_event_handler(
    State(state): State<LoadoutState>,
    headers: HeaderMap,
    Json(req): Json<CompanionEventRequest>,
) -> axum::response::Response {
    if let Err(resp) = auth(&state, &headers, "companion_event_unauthorized") {
        return *resp;
    }
    match state
        .companion
        .record_event(&req.event, &req.source, &state.events_tx)
    {
        Ok(message) => {
            let mut body = serde_json::to_value(state.companion.snapshot())
                .unwrap_or(serde_json::Value::Null);
            if let serde_json::Value::Object(map) = &mut body {
                map.insert("message".to_string(), serde_json::json!(message));
            }
            Json(body).into_response()
        }
        Err(e) => {
            tracing::warn!(event = "companion_event_failed", error = %e);
            (StatusCode::BAD_REQUEST, e.to_string()).into_response()
        }
    }
}

/// One field bag for every kind — serde tags on `kind` would 422 with an
/// opaque error; dispatching by hand keeps the A3 handler's explicit 400s.
#[derive(Deserialize)]
struct CreateProposalRequest {
    kind: String,
    // persona_edit
    slug: Option<String>,
    content: Option<String>,
    // model_config
    default_model: Option<String>,
    fallback_models: Option<Vec<String>>,
    // secret_set — `value` goes to the in-memory pen, NEVER into the payload
    // row (and this struct derives no Serialize/Debug, so it cannot leak).
    provider_id: Option<String>,
    env_key: Option<String>,
    value: Option<String>,
    // routing_config
    rules: Option<std::collections::HashMap<String, String>>,
}

async fn proposals_create_handler(
    State(state): State<LoadoutState>,
    headers: HeaderMap,
    Json(req): Json<CreateProposalRequest>,
) -> axum::response::Response {
    let owner = match auth(&state, &headers, "proposals_unauthorized") {
        Ok(o) => o,
        Err(resp) => return *resp,
    };
    let (payload, secret_value) = match req.kind.as_str() {
        "persona_edit" => {
            let (Some(slug), Some(content)) = (req.slug, req.content) else {
                return (StatusCode::BAD_REQUEST, "persona_edit needs slug and content")
                    .into_response();
            };
            if !proposals::is_safe_slug(&slug) {
                return (StatusCode::BAD_REQUEST, "invalid persona slug").into_response();
            }
            if content.len() > proposals::MAX_CONTENT_BYTES {
                return (StatusCode::PAYLOAD_TOO_LARGE, "content too large").into_response();
            }
            (ProposalPayload::PersonaEdit { slug, content }, None)
        }
        "model_config" => {
            let default_model = req.default_model.map(|m| m.trim().to_string());
            if req.fallback_models.is_none() && default_model.is_none() {
                return (
                    StatusCode::BAD_REQUEST,
                    "model_config needs default_model and/or fallback_models",
                )
                    .into_response();
            }
            if default_model.as_deref() == Some("") {
                return (StatusCode::BAD_REQUEST, "default_model must not be empty")
                    .into_response();
            }
            if let Some(fallbacks) = &req.fallback_models {
                if fallbacks.len() > proposals::MAX_FALLBACK_MODELS {
                    return (StatusCode::BAD_REQUEST, "too many fallback models").into_response();
                }
                if fallbacks.iter().any(|m| m.trim().is_empty()) {
                    return (StatusCode::BAD_REQUEST, "fallback model ids must not be empty")
                        .into_response();
                }
            }
            (
                ProposalPayload::ModelConfig {
                    default_model,
                    fallback_models: req.fallback_models,
                },
                None,
            )
        }
        "secret_set" => {
            let (Some(provider_id), Some(env_key), Some(value)) =
                (req.provider_id, req.env_key, req.value)
            else {
                return (
                    StatusCode::BAD_REQUEST,
                    "secret_set needs provider_id, env_key and value",
                )
                    .into_response();
            };
            if !proposals::is_valid_env_key(&env_key)
                || !model_catalog::is_known_provider_env_key(&env_key)
                || model_catalog::env_key_for_provider(&provider_id) != Some(env_key.as_str())
            {
                return (
                    StatusCode::BAD_REQUEST,
                    "env_key must be the known env key of a known API-key provider",
                )
                    .into_response();
            }
            if value.is_empty() {
                return (StatusCode::BAD_REQUEST, "secret value must not be empty")
                    .into_response();
            }
            if value.len() > proposals::MAX_SECRET_VALUE_BYTES {
                return (StatusCode::PAYLOAD_TOO_LARGE, "secret value too large").into_response();
            }
            (
                ProposalPayload::SecretSet {
                    provider_id,
                    env_key,
                },
                Some(bastion_types::SecretValue::new(value)),
            )
        }
        "routing_config" => {
            let Some(rules) = req.rules else {
                return (
                    StatusCode::BAD_REQUEST,
                    "routing_config needs rules (a class → model map; empty clears the override)",
                )
                    .into_response();
            };
            for (class, model) in &rules {
                if crate::routing::RouteClass::parse(class).is_none() {
                    return (
                        StatusCode::BAD_REQUEST,
                        "unknown routing class — classes are chat_turn, pursue_task, cabinet, \
                         reflection, compaction",
                    )
                        .into_response();
                }
                if model.trim().is_empty() {
                    return (StatusCode::BAD_REQUEST, "routing model ids must not be empty")
                        .into_response();
                }
                // Unknown model ids are legal (custom/niche providers route
                // by prefix, exactly like /model) — surfaced as a warning in
                // the log, never a rejection.
                if !model_catalog::static_catalog().iter().any(|e| e.id == *model) {
                    tracing::warn!(
                        event = "routing_rule_uncatalogued_model",
                        class = %class,
                        model = %model,
                        "model id is not in the static catalog — accepted (custom ids are legal)",
                    );
                }
            }
            (ProposalPayload::RoutingConfig { rules }, None)
        }
        _ => return (StatusCode::BAD_REQUEST, "unknown proposal kind").into_response(),
    };
    match state.proposal_store.create(&owner, "web", &payload).await {
        Ok(p) => {
            // secret_set: the value lives ONLY in this in-memory pen (keyed
            // by proposal id) until console approve writes it to the secrets
            // dir. The sqlite row above holds the reference, never the value.
            if let Some(value) = secret_value {
                state.pending_secrets.put(&p.id, value).await;
            }
            // Attention plumbing: the operator sees the request wherever they
            // watch — SSE (dashboard/TUI ledger) — and approves on console.
            let _ = state.events_tx.send(
                serde_json::json!({
                    "event": "config.change_requested",
                    "owner": p.owner_id,
                    "proposal": p.id,
                    "origin": p.origin,
                })
                .to_string(),
            );
            (StatusCode::CREATED, Json(serde_json::json!(p))).into_response()
        }
        Err(e) => {
            tracing::warn!(event = "proposal_create_failed", error = %e);
            (StatusCode::INTERNAL_SERVER_ERROR, "proposal store error").into_response()
        }
    }
}

/// Build the operator sub-router: `/loadout`, persona reads, and staged
/// proposals. Merged into the webhook app after `.with_state` — same slot
/// as `control_plane_routes`.
#[allow(clippy::too_many_arguments)] // composition-root bag, same as serve_with_mesh
pub fn router(
    snapshot: LoadoutSnapshot,
    owner_map: OwnerMap,
    jwt_secret: String,
    proposal_store: Arc<SqliteProposalStore>,
    config_store: ConfigStore,
    model_defaults: ModelDefaults,
    auth_cfg: AuthConfig,
    pending_secrets: PendingSecretValues,
    routing_toml: std::collections::HashMap<String, String>,
    events_tx: broadcast::Sender<String>,
    // A5 S5: shared with `CompanionEventCapability` — same instance, so
    // `POST /companion/care` and hook-triggered session events serialize
    // through one in-process writer.
    companion: CompanionHandle,
) -> Router {
    Router::new()
        .route("/loadout", get(loadout_handler))
        .route("/personas", get(personas_list_handler))
        .route("/personas/{slug}", get(persona_read_handler))
        .route(
            "/proposals",
            get(proposals_list_handler).post(proposals_create_handler),
        )
        .route("/config/overrides", get(config_overrides_handler))
        .route("/providers", get(providers_handler))
        .route("/models", get(models_handler))
        .route("/routing", get(routing_handler))
        .route("/companion", get(companion_handler))
        .route("/companion/care", axum::routing::post(companion_care_handler))
        .route(
            "/companion/event",
            axum::routing::post(companion_event_handler),
        )
        .with_state(LoadoutState {
            snapshot: Arc::new(snapshot),
            owner_map: Arc::new(owner_map),
            jwt_secret,
            proposal_store,
            config_store,
            model_defaults: Arc::new(model_defaults),
            auth: Arc::new(auth_cfg),
            pending_secrets,
            routing_toml: Arc::new(routing_toml),
            events_tx,
            companion,
        })
}

/// Capture the composition from the pieces `daemon_loop` already holds.
pub fn snapshot(
    persona_names: Vec<String>,
    tool_names: Vec<String>,
    runtime_ids: Vec<String>,
    channels: Vec<ChannelPiece>,
    mcp_servers: Vec<String>,
) -> LoadoutSnapshot {
    LoadoutSnapshot {
        personas: persona_names,
        tools: tool_names,
        runtimes: runtime_ids
            .into_iter()
            .map(|id| RuntimePiece { id })
            .collect(),
        channels,
        mcp_servers,
        extensions: Vec::new(),
        captured_at: now_nanos(),
    }
}

fn now_nanos() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn sample_router(
        owner_map: OwnerMap,
    ) -> (tempfile::NamedTempFile, Router, PendingSecretValues) {
        let snap = snapshot(
            vec!["ada".into()],
            vec!["create_task".into()],
            vec!["codex_app_server".into()],
            vec![ChannelPiece {
                id: "webhook",
                enabled: true,
            }],
            vec!["memupalace".into()],
        );
        let f = tempfile::NamedTempFile::new().unwrap();
        let store = Arc::new(SqliteProposalStore::new(
            f.path().to_str().unwrap().to_owned(),
        ));
        let config_store = ConfigStore::new(f.path().to_str().unwrap().to_owned());
        config_store.init_schema().await.unwrap();
        store.init_schema().await.unwrap();
        let (events_tx, _) = broadcast::channel(8);
        let pending = PendingSecretValues::default();
        (
            f,
            router(
                snap,
                owner_map,
                "test-secret".into(),
                store,
                config_store,
                ModelDefaults {
                    // "test-model" has no known prefix and no '/', so it
                    // routes to ollama — a custom id the merge must surface.
                    default_model: "test-model".into(),
                    fallback_models: vec!["gemini-2.5-flash".into()],
                },
                AuthConfig::default(),
                pending.clone(),
                // A4.5: a toml `[routing]` rule the store can override.
                std::collections::HashMap::from([(
                    "reflection".to_string(),
                    "llama3.2".to_string(),
                )]),
                events_tx,
                // A5 S5: `CompanionHandle::load` only ever READS
                // `companion.json` at construction (no `save()`), so this is
                // side-effect-free against a developer's real
                // `~/.config/bastion` — see the module doc for why a
                // POST /companion/care round trip isn't exercised here.
                CompanionHandle::load(false),
            ),
            pending,
        )
    }

    async fn get_json(
        app: Router,
        uri: &str,
        token: Option<&str>,
    ) -> (axum::http::StatusCode, serde_json::Value) {
        let mut builder = axum::http::Request::builder().uri(uri);
        if let Some(t) = token {
            builder = builder.header("x-bastion-token", t);
        }
        let req = builder.body(axum::body::Body::empty()).unwrap();
        let resp = tower::ServiceExt::oneshot(app, req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        let v = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, v)
    }

    async fn post_json(
        app: Router,
        uri: &str,
        token: &str,
        body: serde_json::Value,
    ) -> (axum::http::StatusCode, serde_json::Value) {
        let req = axum::http::Request::builder()
            .method("POST")
            .uri(uri)
            .header("x-bastion-token", token)
            .header("content-type", "application/json")
            .body(axum::body::Body::from(body.to_string()))
            .unwrap();
        let resp = tower::ServiceExt::oneshot(app, req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        let v = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, v)
    }

    #[tokio::test]
    async fn loadout_requires_owner_token() {
        let (_f, app, _pending) = sample_router(OwnerMap::default()).await;
        let (status, _) = get_json(app, "/loadout", None).await;
        assert_eq!(status, axum::http::StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn config_overrides_requires_owner_token_and_lists_for_a_valid_one() {
        let owner_map = OwnerMap::from_pairs(&[("tok-alice", "alice")]);
        let (_f, app, _pending) = sample_router(owner_map).await;

        let (status, _) = get_json(app.clone(), "/config/overrides", None).await;
        assert_eq!(status, axum::http::StatusCode::UNAUTHORIZED);

        let (status, v) = get_json(app, "/config/overrides", Some("tok-alice")).await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(v["items"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn loadout_answers_composition_for_a_valid_token() {
        let owner_map = OwnerMap::from_pairs(&[("tok-alice", "alice")]);
        let (_f, app, _pending) = sample_router(owner_map).await;
        let (status, v) = get_json(app, "/loadout", Some("tok-alice")).await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(v["personas"], serde_json::json!(["ada"]));
        assert_eq!(v["extensions"], serde_json::json!([]));
        assert!(v["captured_at"].as_i64().unwrap() > 0);
    }

    // ---- A4 S2: /providers, /models, new proposal kinds -----------------

    // `std::env` is process-global; every test here that touches a provider
    // env key serializes on this lock and restores the prior value (a dev
    // machine may genuinely have one set).
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[tokio::test]
    async fn providers_requires_owner_token_and_reports_boolean_status() {
        let owner_map = OwnerMap::from_pairs(&[("tok-alice", "alice")]);
        let (_f, app, _pending) = sample_router(owner_map).await;

        let (status, _) = get_json(app.clone(), "/providers", None).await;
        assert_eq!(status, axum::http::StatusCode::UNAUTHORIZED);

        let saved = {
            let _guard = ENV_LOCK.lock().unwrap();
            let saved = std::env::var("GEMINI_API_KEY").ok();
            std::env::set_var("GEMINI_API_KEY", "test-key-never-echoed");
            saved
        };
        let (status, v) = get_json(app, "/providers", Some("tok-alice")).await;
        {
            let _guard = ENV_LOCK.lock().unwrap();
            match saved {
                Some(prev) => std::env::set_var("GEMINI_API_KEY", prev),
                None => std::env::remove_var("GEMINI_API_KEY"),
            }
        }

        assert_eq!(status, axum::http::StatusCode::OK);
        let items = v["items"].as_array().unwrap();
        // 5 api_key providers + ollama (no [auth.*] profiles configured here).
        assert_eq!(items.len(), 6);
        assert!(!v.to_string().contains("test-key-never-echoed"));

        let gemini = items.iter().find(|i| i["id"] == "gemini").unwrap();
        assert_eq!(gemini["kind"], "api_key");
        assert_eq!(gemini["connected"], true);
        assert_eq!(gemini["source"], "env");
        assert!(gemini["models_count"].as_u64().unwrap() > 0);
        // S4 cleanup: name + env key come from the daemon's whitelist —
        // the web app renders these fields instead of a mirrored table.
        assert_eq!(gemini["display_name"], "Google Gemini");
        assert_eq!(gemini["env_key"], "GEMINI_API_KEY");

        let ollama = items.iter().find(|i| i["id"] == "ollama").unwrap();
        assert_eq!(ollama["kind"], "local");
        // default_model "test-model" routes to ollama → configured.
        assert_eq!(ollama["connected"], true);
        assert_eq!(ollama["source"], serde_json::Value::Null);
        assert_eq!(ollama["display_name"], "Ollama");
        assert_eq!(ollama["env_key"], serde_json::Value::Null);

        for item in items {
            assert!(item["connected"].is_boolean());
        }
    }

    #[tokio::test]
    async fn models_reports_effective_defaults_and_merges_custom_ids() {
        let owner_map = OwnerMap::from_pairs(&[("tok-alice", "alice")]);
        let (_f, app, _pending) = sample_router(owner_map).await;
        let (status, v) = get_json(app, "/models", Some("tok-alice")).await;
        assert_eq!(status, axum::http::StatusCode::OK);
        // No config-store overrides yet → bastion.toml values pass through.
        assert_eq!(v["default_model"], "test-model");
        assert_eq!(
            v["fallback_models"],
            serde_json::json!(["gemini-2.5-flash"])
        );

        let providers = v["providers"].as_array().unwrap();
        let kinds: Vec<&str> = providers
            .iter()
            .map(|p| p["provider_kind"].as_str().unwrap())
            .collect();
        assert_eq!(
            kinds,
            vec!["anthropic", "openai", "gemini", "groq", "openrouter", "ollama"]
        );
        // The custom toml default merged into its (ollama) group.
        let ollama = providers.iter().find(|p| p["provider_kind"] == "ollama").unwrap();
        let ids: Vec<&str> = ollama["models"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["id"].as_str().unwrap())
            .collect();
        assert!(ids.contains(&"test-model"));
        let entry = ollama["models"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["id"] == "test-model")
            .unwrap();
        assert_eq!(entry["provider_kind"], "ollama");
        assert_eq!(entry["display_name"], "test-model");
    }

    #[tokio::test]
    async fn model_config_proposal_stages_pending_and_validates() {
        let owner_map = OwnerMap::from_pairs(&[("tok-alice", "alice")]);
        let (_f, app, _pending) = sample_router(owner_map).await;

        let (status, _) = post_json(
            app.clone(),
            "/proposals",
            "tok-alice",
            serde_json::json!({ "kind": "model_config" }),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);

        let (status, p) = post_json(
            app,
            "/proposals",
            "tok-alice",
            serde_json::json!({
                "kind": "model_config",
                "default_model": "llama3.2",
                "fallback_models": ["mistral"],
            }),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::CREATED);
        assert_eq!(p["status"], "pending");
        assert_eq!(p["payload"]["kind"], "model_config");
        assert_eq!(p["payload"]["default_model"], "llama3.2");
    }

    // ---- A4.5: /routing + routing_config proposals ----------------------

    #[tokio::test]
    async fn routing_requires_owner_token_and_always_lists_all_five_classes() {
        let owner_map = OwnerMap::from_pairs(&[("tok-alice", "alice")]);
        let (_f, app, _pending) = sample_router(owner_map).await;

        let (status, _) = get_json(app.clone(), "/routing", None).await;
        assert_eq!(status, axum::http::StatusCode::UNAUTHORIZED);

        let (status, v) = get_json(app, "/routing", Some("tok-alice")).await;
        assert_eq!(status, axum::http::StatusCode::OK);
        let items = v["items"].as_array().unwrap();
        let classes: Vec<&str> = items
            .iter()
            .map(|i| i["class"].as_str().unwrap())
            .collect();
        assert_eq!(
            classes,
            vec!["chat_turn", "pursue_task", "cabinet", "reflection", "compaction"]
        );
        // sample_router's toml rule: reflection → llama3.2, source toml.
        let reflection = items.iter().find(|i| i["class"] == "reflection").unwrap();
        assert_eq!(reflection["model"], "llama3.2");
        assert_eq!(reflection["source"], "toml");
        assert_eq!(reflection["supported"], true);
        // No rule for chat_turn: nulls, but the row is still listed.
        let chat = items.iter().find(|i| i["class"] == "chat_turn").unwrap();
        assert_eq!(chat["model"], serde_json::Value::Null);
        assert_eq!(chat["source"], serde_json::Value::Null);
        assert_eq!(chat["supported"], true);
        // Honest v1: no reachable knob on the pinned core rev.
        for unsupported in ["pursue_task", "cabinet", "compaction"] {
            let row = items.iter().find(|i| i["class"] == unsupported).unwrap();
            assert_eq!(row["supported"], false, "{unsupported}");
        }
    }

    #[tokio::test]
    async fn routing_config_proposal_stages_pending_and_validates_classes() {
        let owner_map = OwnerMap::from_pairs(&[("tok-alice", "alice")]);
        let (_f, app, _pending) = sample_router(owner_map).await;

        let (status, _) = post_json(
            app.clone(),
            "/proposals",
            "tok-alice",
            serde_json::json!({ "kind": "routing_config" }),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);

        let (status, _) = post_json(
            app.clone(),
            "/proposals",
            "tok-alice",
            serde_json::json!({
                "kind": "routing_config",
                "rules": { "not_a_class": "llama3.2" },
            }),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);

        let (status, _) = post_json(
            app.clone(),
            "/proposals",
            "tok-alice",
            serde_json::json!({
                "kind": "routing_config",
                "rules": { "chat_turn": "  " },
            }),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);

        // Uncatalogued model id: legal (custom models route by prefix).
        let (status, p) = post_json(
            app,
            "/proposals",
            "tok-alice",
            serde_json::json!({
                "kind": "routing_config",
                "rules": { "chat_turn": "my-custom-local-model", "compaction": "llama3.2" },
            }),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::CREATED);
        assert_eq!(p["status"], "pending");
        assert_eq!(p["payload"]["kind"], "routing_config");
        assert_eq!(p["payload"]["rules"]["chat_turn"], "my-custom-local-model");
    }

    #[tokio::test]
    async fn secret_set_proposal_pens_the_value_and_never_stores_it() {
        let owner_map = OwnerMap::from_pairs(&[("tok-alice", "alice")]);
        let (_f, app, pending) = sample_router(owner_map).await;

        // Unknown env key / mismatched provider → 400.
        let (status, _) = post_json(
            app.clone(),
            "/proposals",
            "tok-alice",
            serde_json::json!({
                "kind": "secret_set",
                "provider_id": "gemini",
                "env_key": "SOME_OTHER_KEY",
                "value": "sk-x",
            }),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);

        let (status, p) = post_json(
            app,
            "/proposals",
            "tok-alice",
            serde_json::json!({
                "kind": "secret_set",
                "provider_id": "gemini",
                "env_key": "GEMINI_API_KEY",
                "value": "sk-pen-only",
            }),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::CREATED);
        // The response (the stored payload) carries the reference, not the value.
        assert_eq!(p["payload"]["kind"], "secret_set");
        assert_eq!(p["payload"]["env_key"], "GEMINI_API_KEY");
        assert!(!p.to_string().contains("sk-pen-only"));
        // The value went to the in-memory pen, keyed by the new proposal id.
        let id = p["id"].as_str().unwrap();
        let penned = pending.take(id).await.expect("value must be penned");
        assert_eq!(penned.expose_secret(), "sk-pen-only");
    }

    // ---- A5 S5: /companion + /companion/care -----------------------------

    #[tokio::test]
    async fn companion_requires_owner_token_and_reports_the_snapshot_shape() {
        let owner_map = OwnerMap::from_pairs(&[("tok-alice", "alice")]);
        let (_f, app, _pending) = sample_router(owner_map).await;

        let (status, _) = get_json(app.clone(), "/companion", None).await;
        assert_eq!(status, axum::http::StatusCode::UNAUTHORIZED);

        let (status, v) = get_json(app, "/companion", Some("tok-alice")).await;
        assert_eq!(status, axum::http::StatusCode::OK);
        // Shape-only: `CompanionHandle` always reads the REAL
        // `~/.config/bastion/companion.json` (see `sample_router`'s
        // comment), so the actual numbers vary by machine — this pins the
        // contract, not the values.
        assert!(v["game_enabled"].is_boolean());
        assert!(v["level"].as_u64().unwrap() >= 1);
        assert!(v["xp"].is_u64());
        assert!(v["successful_turns"].is_u64());
        for need in ["water", "food", "play", "rest"] {
            assert!(v["needs"][need].is_u64(), "needs.{need}");
        }
        assert!(v["cues"].is_array());
        assert!(v["frame"]["rows"].is_array());
        assert!(v["frame"]["width"].is_u64());
        assert!(v["pack_name"].is_string());
    }

    #[tokio::test]
    async fn companion_care_requires_owner_token_and_rejects_an_unknown_action() {
        let owner_map = OwnerMap::from_pairs(&[("tok-alice", "alice")]);
        let (_f, app, _pending) = sample_router(owner_map).await;

        let (status, _) = post_json(
            app.clone(),
            "/companion/care",
            "not-a-token",
            serde_json::json!({ "action": "water" }),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::UNAUTHORIZED);

        // An unknown action is rejected INSIDE `CompanionHandle::mutate`
        // before it ever calls `save()` (see `tui.rs`) — this never writes
        // to companion.json, so it's safe to run against the real path.
        let (status, _) = post_json(
            app,
            "/companion/care",
            "tok-alice",
            serde_json::json!({ "action": "nap" }),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
    }

    // ---- S6: POST /companion/event ---------------------------------------

    #[tokio::test]
    async fn companion_event_requires_owner_token() {
        let owner_map = OwnerMap::from_pairs(&[("tok-alice", "alice")]);
        let (_f, app, _pending) = sample_router(owner_map).await;

        let (status, _) = post_json(
            app,
            "/companion/event",
            "not-a-token",
            serde_json::json!({ "event": "activity", "source": "claude" }),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn companion_event_rejects_an_unknown_kind() {
        let owner_map = OwnerMap::from_pairs(&[("tok-alice", "alice")]);
        let (_f, app, _pending) = sample_router(owner_map).await;

        // Rejected INSIDE `CompanionHandle::record_event` before the
        // `mutate` closure ever reaches `save()` — never writes to the real
        // `companion.json`, same safety property `companion_care`'s test
        // above relies on.
        let (status, _) = post_json(
            app,
            "/companion/event",
            "tok-alice",
            serde_json::json!({ "event": "session-pause", "source": "claude" }),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn companion_event_rejects_an_invalid_source() {
        let owner_map = OwnerMap::from_pairs(&[("tok-alice", "alice")]);
        let (_f, app, _pending) = sample_router(owner_map).await;

        // `validate_companion_source` runs before `record_event` ever locks
        // the shared handle — same no-write-on-reject guarantee.
        let (status, _) = post_json(
            app,
            "/companion/event",
            "tok-alice",
            serde_json::json!({ "event": "activity", "source": "has space" }),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
    }
}
