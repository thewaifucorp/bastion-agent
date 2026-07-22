//! Bastion MCP server — exposes capabilities as MCP tools/resources.
//!
//! Every inbound call dispatches through CapabilityRegistry::invoke, maintaining
//! the egress gate and approval queue (D-07). Static token auth with per-token
//! read-only/read-write permissions (D-05).
//!
//! Transports: Streamable HTTP (axum, Tasks 1-2) + stdio (Task 3, D-06).
//!
//! 09-REVIEW.md CR-01/CR-02/CR-03: authentication is fail-closed — a missing or
//! unrecognized `x-bastion-token` is REJECTED, never treated as an implicit grant
//! of local-owner access. `list_resources`/`read_resource` go through the same
//! token check as `call_tool` (they are reachable on the same network-exposed
//! port as `call_tool`), and `read_resource`'s memory/persona content is filtered
//! through `check_egress` per-item before it leaves the process, exactly like
//! every other cloud-facing surface in this codebase.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;

use crate::control_plane::scope::{require_scope, Scope, ScopeSet};
use axum::Router;
use bastion_cognition::goal::GoalEngine;
use bastion_memory::{PrivacyTier, SharedMemory};
use bastion_personas::persona::PersonaRegistry;
use bastion_runtime::capability::CapabilityRegistry;
use bastion_runtime::hooks::egress::check_egress;
use rmcp::handler::server::router::Router as McpRouter;
use rmcp::model::*;
use rmcp::service::{MaybeSendFuture, RequestContext, RoleServer};
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, tower::StreamableHttpService, StreamableHttpServerConfig,
};
use rmcp::{ErrorData as McpError, ServerHandler};
use serde_json::Value;

/// Per-token permissions (D-05): read_only vs read-write, bound to a specific owner.
///
/// `privacy_tier` (09-REVIEW.md CR-03) is the tier passed to `CapabilityRegistry::invoke`
/// for every call authenticated with this token — it is NEVER hardcoded to `CloudOk`.
/// Defaults to `LocalOnly` (the most restrictive tier, per the same fail-closed
/// convention used in `agent/loop_.rs`'s tool dispatch): an MCP token can only reach
/// local capabilities unless the operator explicitly opts it into `CloudOk` in config.
#[derive(Debug, Clone)]
pub struct TokenPermissions {
    pub read_only: bool,
    pub owner_id: String,
    pub privacy_tier: PrivacyTier,
    /// Control Plane scopes (`control_plane::scope::Scope`) this token is
    /// granted for the 5 dedicated Control Plane MCP tools
    /// (create_task/get_task/list_tasks/steer_task/cancel_task, see
    /// `control_plane::mcp_tools`). Checked in `call_tool` ONLY when
    /// dispatching to `control_plane_registry` — every other (shared-registry)
    /// MCP tool is unaffected, and `read_only` still gates ALL tool
    /// invocation exactly as before this field existed. The HTTP `/v1/*`
    /// routes (`control_plane::routes`) already enforce this same 4-scope
    /// model via `SqliteCredentialStore`-issued credentials; this closes the
    /// same gap for MCP callers, which previously had no scope concept at
    /// all beyond this coarse `read_only` boolean.
    pub control_plane_scopes: ScopeSet,
}

/// 09-REVIEW.md WR-08: constant-time byte comparison so token lookup doesn't leak
/// timing information about a configured secret via early-exit comparison.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

/// The Control Plane scope a given MCP tool name requires, or `None` if
/// `name` isn't one of the 5 dedicated Control Plane tools — every other
/// MCP tool (the shared registry) is entirely unaffected by Control Plane
/// scoping. NOTE: this table is the one place that must be kept in sync
/// with `control_plane::mcp_tools::build_registry`'s tool set — a new
/// Control Plane tool added there without a matching arm here would ship
/// UNGATED (falls through to `None`, skipping the scope check entirely).
/// `scope_mapping_covers_every_registered_control_plane_tool` (below) pins
/// today's 5-tool set so an unreviewed drift is at least caught by tests.
fn required_control_plane_scope(name: &str) -> Option<Scope> {
    match name {
        "get_task" | "list_tasks" => Some(Scope::TasksRead),
        "create_task" => Some(Scope::TasksCreate),
        "steer_task" | "cancel_task" => Some(Scope::TasksControl),
        _ => None,
    }
}

/// Pure scope gate for a Control Plane tool call: `Ok(())` for every
/// non-control-plane tool name (unaffected) or a control-plane tool the
/// caller's `scopes` cover; `Err` (a ready-to-return `CallToolResult`,
/// mirroring the `read_only` check's existing soft-error shape — this is a
/// 403-equivalent, never the hard `McpError` `authenticate_token` uses for
/// actual authentication failures) otherwise.
///
/// This function alone does NOT know whether `name` actually resolves to
/// `control_plane_registry` at dispatch time — it re-derives "is this a
/// Control Plane tool" from `required_control_plane_scope`'s static table.
/// `call_tool` is responsible for only calling this when its own
/// `dispatches_to_control_plane` (computed from `control_plane_registry.
/// list_names()`, the actual dispatch decision) is true, so the two checks
/// stay symmetric even if a future shared-registry capability name were to
/// collide with one of these 5 tool names.
fn check_control_plane_scope(name: &str, scopes: &ScopeSet) -> Result<(), CallToolResult> {
    let Some(required) = required_control_plane_scope(name) else {
        return Ok(());
    };
    require_scope(scopes, required).map_err(|missing| {
        CallToolResult::error(vec![Content::text(format!("forbidden: {missing}"))])
    })
}

/// 09-REVIEW.md CR-01/CR-02: shared fail-closed token check used by `list_tools`,
/// `call_tool`, `list_resources`, and `read_resource`. A missing token, an empty
/// token map, or a token that doesn't match any configured entry is rejected —
/// never defaulted to a permissive local-owner grant.
fn authenticate_token(
    tokens: &HashMap<String, TokenPermissions>,
    meta: Option<&Meta>,
) -> Result<TokenPermissions, McpError> {
    let presented = meta
        .and_then(|m| m.get("x-bastion-token"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    tokens
        .iter()
        // Milestone-close code review (2026-07-13): an empty-string presented
        // token must never authenticate, even against a misconfigured empty
        // configured entry — `constant_time_eq("", "")` would otherwise be
        // `true`, silently granting that entry's permissions to any caller
        // who supplied no `x-bastion-token` at all.
        .find(|(configured, _)| {
            !configured.is_empty() && constant_time_eq(configured.as_bytes(), presented.as_bytes())
        })
        .map(|(_, perms)| perms.clone())
        .ok_or_else(|| {
            tracing::warn!(
                event = "mcp_unauthorized",
                "missing or unknown x-bastion-token"
            );
            McpError::invalid_request("unauthorized: missing or invalid x-bastion-token", None)
        })
}

/// Bastion MCP server — dispatches to CapabilityRegistry, Memory, PersonaRegistry, GoalEngine.
pub struct BastionMcpServer {
    registry: Arc<CapabilityRegistry>,
    /// US External Control Plane and SDK, Phase 5: a SEPARATE registry
    /// holding only the 5 Control Plane tools
    /// (`create_task`/`get_task`/`list_tasks`/`steer_task`/`cancel_task`,
    /// see `control_plane::mcp_tools`) — deliberately NOT merged into
    /// `registry` above, which Bastion's own internal tool-calling loop
    /// (`agent/loop_.rs`) also dispatches through. See
    /// `control_plane::mcp_tools`'s module doc for why: these tools must be
    /// reachable by an external MCP caller but NOT by a running Pursue
    /// task's own LLM reasoning. `list_tools`/`call_tool` below check both
    /// registries; every other method (`list_resources`/`read_resource`) is
    /// unaffected.
    control_plane_registry: Arc<CapabilityRegistry>,
    memory: SharedMemory,
    personas: Arc<PersonaRegistry>,
    goals: GoalEngine,
    token_permissions: HashMap<String, TokenPermissions>,
    local_owner: String,
}

impl BastionMcpServer {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        registry: Arc<CapabilityRegistry>,
        control_plane_registry: Arc<CapabilityRegistry>,
        memory: SharedMemory,
        personas: Arc<PersonaRegistry>,
        goals: GoalEngine,
        token_permissions: HashMap<String, TokenPermissions>,
        local_owner: String,
    ) -> Self {
        Self {
            registry,
            control_plane_registry,
            memory,
            personas,
            goals,
            token_permissions,
            local_owner,
        }
    }
}

impl Clone for BastionMcpServer {
    fn clone(&self) -> Self {
        Self {
            registry: self.registry.clone(),
            control_plane_registry: self.control_plane_registry.clone(),
            memory: self.memory.clone(),
            personas: self.personas.clone(),
            goals: self.goals.clone(),
            token_permissions: self.token_permissions.clone(),
            local_owner: self.local_owner.clone(),
        }
    }
}

impl ServerHandler for BastionMcpServer {
    fn get_info(&self) -> ServerInfo {
        let caps = ServerCapabilities::builder()
            .enable_tools()
            .enable_resources()
            .build();
        ServerInfo::new(caps)
            .with_server_info(Implementation::new("bastion", env!("CARGO_PKG_VERSION")))
    }

    fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, McpError>> + MaybeSendFuture + '_ {
        let meta = request.and_then(|r| r.meta);
        let token_permissions = self.token_permissions.clone();
        let registry = self.registry.clone();
        let control_plane_registry = self.control_plane_registry.clone();

        async move {
            authenticate_token(&token_permissions, meta.as_ref())?;

            let mut tools: Vec<Tool> = registry
                .list_tool_defs()
                .into_iter()
                .chain(control_plane_registry.list_tool_defs())
                .map(|def| {
                    let name = def["name"].as_str().unwrap_or("unknown").to_string();
                    let description = def["description"].as_str().unwrap_or("").to_string();
                    let schema_obj = match def.get("input_schema") {
                        Some(Value::Object(obj)) => obj.clone(),
                        _ => serde_json::Map::new(),
                    };
                    Tool::new(name, description, Arc::new(schema_obj))
                })
                .collect();
            // COST-01/D-14b: `list_tool_defs()` sorts WITHIN each registry,
            // but chaining two already-sorted lists doesn't sort the combined
            // one — re-sort here so the merged listing is still byte-stable
            // turn-over-turn regardless of which registry a tool lives in.
            tools.sort_by(|a, b| a.name.cmp(&b.name));
            Ok(ListToolsResult::with_all_items(tools))
        }
    }

    fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<CallToolResult, McpError>> + MaybeSendFuture + '_ {
        let meta = request.meta.clone();
        let name = request.name.clone();
        let args = request.arguments.unwrap_or_default();

        let registry = self.registry.clone();
        let control_plane_registry = self.control_plane_registry.clone();
        let token_permissions = self.token_permissions.clone();

        async move {
            let perms = authenticate_token(&token_permissions, meta.as_ref())?;

            if perms.read_only {
                return Ok(CallToolResult::error(vec![Content::text(
                    "read-only token cannot invoke tools",
                )]));
            }

            // CR-03: privacy_tier comes from the AUTHENTICATED token's configured
            // tier — never a blanket CloudOk applied to every MCP caller, which
            // would silently disable check_egress's fail-closed guarantee for
            // every capability routed through this server.
            let ctx = bastion_runtime::capability::InvokeCtx {
                owner: perms.owner_id,
                privacy_tier: Some(perms.privacy_tier),
            };

            // Phase 5: the 5 Control Plane tools live in a SEPARATE registry
            // (see the `control_plane_registry` field doc comment) — dispatch
            // to it when the name is one of its own, otherwise fall through
            // to the shared registry exactly as before.
            let dispatches_to_control_plane =
                control_plane_registry.list_names().contains(&name.as_ref());
            let target = if dispatches_to_control_plane {
                &control_plane_registry
            } else {
                &registry
            };

            // Gap closed: a Control Plane tool call must additionally carry
            // the specific scope that tool requires (matching the HTTP
            // `/v1/*` routes' `require_scope` enforcement) — checked BEFORE
            // `target.invoke`, so an under-scoped token never reaches
            // `core_ops` at all, not even for a failed/no-op attempt.
            //
            // Gated on `dispatches_to_control_plane` — the SAME boolean used
            // to pick `target` above — rather than letting
            // `check_control_plane_scope` re-derive "is this a Control Plane
            // tool" from the raw `name` in isolation. This keeps the scope
            // gate symmetric with dispatch: a name is only ever scope-checked
            // when it is ACTUALLY routed to `control_plane_registry`. Without
            // this, a future shared-registry capability that happened to
            // share a name with one of the 5 CP tools (e.g. a hypothetical
            // "cancel_task") would still get scope-gated by
            // `required_control_plane_scope`'s static table even though
            // `target` resolved to the shared registry for it.
            if dispatches_to_control_plane {
                if let Err(result) = check_control_plane_scope(&name, &perms.control_plane_scopes)
                {
                    return Ok(result);
                }
            }

            match target.invoke(&name, Value::Object(args), &ctx).await {
                // Plan 11-07 (SEC-04): `.data` is the same JSON-stringified content
                // MCP clients have always received — spotlighting's LLM-facing
                // untrusted-result framing (agent/loop_.rs's dispatch_tool_loop) is
                // scoped to Bastion's OWN tool-loop, not to what Bastion exposes AS
                // an MCP server to other agents. This external response shape is
                // unchanged.
                Ok(tagged) => Ok(CallToolResult::success(vec![Content::text(
                    tagged.data.to_string(),
                )])),
                Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
            }
        }
    }

    fn list_resources(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListResourcesResult, McpError>> + MaybeSendFuture + '_ {
        let meta = request.and_then(|r| r.meta);
        let token_permissions = self.token_permissions.clone();

        async move {
            authenticate_token(&token_permissions, meta.as_ref())?;

            let resources = vec![
                Annotated::new(
                    RawResource::new("bastion://memories", "Agent Memories")
                        .with_description("Retrieve stored beliefs and memories")
                        .with_mime_type("application/json"),
                    None,
                ),
                Annotated::new(
                    RawResource::new("bastion://personas", "Personas")
                        .with_description("List available agent personas")
                        .with_mime_type("application/json"),
                    None,
                ),
                Annotated::new(
                    RawResource::new("bastion://goals", "Goals")
                        .with_description("List tracked goals and progress")
                        .with_mime_type("application/json"),
                    None,
                ),
            ];
            Ok(ListResourcesResult::with_all_items(resources))
        }
    }

    fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ReadResourceResult, McpError>> + MaybeSendFuture + '_ {
        let uri = request.uri;
        let meta = request.meta;

        let memory = self.memory.clone();
        let personas = self.personas.clone();
        let goals = self.goals.clone();
        let local_owner = self.local_owner.clone();
        let token_permissions = self.token_permissions.clone();

        async move {
            authenticate_token(&token_permissions, meta.as_ref())?;

            let contents = match uri.as_str() {
                "bastion://memories" => {
                    let mem = memory.read().await;
                    let beliefs = mem
                        .retrieve_tagged(&local_owner, None)
                        .await
                        .unwrap_or_default();
                    // CR-02: MCP is an external destination — drop any belief that
                    // wouldn't pass check_egress to a non-local provider (LocalOnly
                    // and untagged/None both fail closed) instead of dumping the
                    // owner's full belief store, including LocalOnly-tagged ones.
                    let cloud_ok: Vec<_> = beliefs
                        .into_iter()
                        .filter(|b| check_egress(b.tier, "external").is_ok())
                        .collect();
                    let json =
                        serde_json::to_string_pretty(&cloud_ok).unwrap_or_else(|_| "[]".into());
                    vec![ResourceContents::text(json, &uri).with_mime_type("application/json")]
                }
                "bastion://personas" => {
                    let all_personas: Vec<&bastion_personas::persona::Persona> = personas
                        .names()
                        .into_iter()
                        .filter_map(|name| personas.get(name))
                        // CR-02: same egress rule applied to persona system prompts.
                        .filter(|p| check_egress(Some(p.tier), "external").is_ok())
                        .collect();
                    let json =
                        serde_json::to_string_pretty(&all_personas).unwrap_or_else(|_| "[]".into());
                    vec![ResourceContents::text(json, &uri).with_mime_type("application/json")]
                }
                "bastion://goals" => {
                    let all_goals = goals.list_goals(&local_owner).await.unwrap_or_default();
                    let json =
                        serde_json::to_string_pretty(&all_goals).unwrap_or_else(|_| "[]".into());
                    vec![ResourceContents::text(json, &uri).with_mime_type("application/json")]
                }
                _ => {
                    return Err(McpError::invalid_params(
                        format!("unknown resource: {}", uri),
                        None,
                    ));
                }
            };

            Ok(ReadResourceResult::new(contents))
        }
    }
}

/// Build an axum Router for the MCP Streamable HTTP server.
///
/// Creates a `BastionMcpServer` from components, wraps it in the rmcp
/// `Router` → `StreamableHttpService` chain, and nests it under `mount_path`
/// on an otherwise-empty axum `Router` ready to be merged into the main app.
#[allow(clippy::too_many_arguments)]
pub fn build_mcp_axum_router(
    registry: Arc<CapabilityRegistry>,
    control_plane_registry: Arc<CapabilityRegistry>,
    memory: SharedMemory,
    personas: Arc<PersonaRegistry>,
    goals: GoalEngine,
    tokens: HashMap<String, TokenPermissions>,
    local_owner: String,
    mount_path: &str,
) -> Router {
    let server = BastionMcpServer::new(
        registry,
        control_plane_registry,
        memory,
        personas,
        goals,
        tokens,
        local_owner,
    );
    let session_manager = Arc::new(LocalSessionManager::default());

    let streamable: StreamableHttpService<McpRouter<BastionMcpServer>, LocalSessionManager> =
        StreamableHttpService::new(
            move || Ok(McpRouter::new(server.clone())),
            session_manager,
            StreamableHttpServerConfig::default(),
        );

    Router::new().nest_service(mount_path, streamable)
}

/// 09-REVIEW.md WR-05: the new MCP auth surface had zero test coverage — these
/// exercise `authenticate_token` (the fail-closed CR-01/CR-02 gate shared by
/// `call_tool`/`list_resources`/`read_resource`) and `TokenPermissions`' default
/// tier (CR-03), the exact scenarios that would have caught CR-01/CR-02/CR-03.
#[cfg(test)]
mod tests {
    use super::*;

    fn tokens_with(token: &str, perms: TokenPermissions) -> HashMap<String, TokenPermissions> {
        HashMap::from([(token.to_string(), perms)])
    }

    fn meta_with_token(token: &str) -> Meta {
        let mut map = serde_json::Map::new();
        map.insert("x-bastion-token".to_string(), Value::String(token.into()));
        Meta(map)
    }

    fn rw_perms(owner: &str) -> TokenPermissions {
        rw_perms_with_scopes(
            owner,
            ScopeSet::new([
                Scope::TasksRead,
                Scope::TasksCreate,
                Scope::TasksControl,
                Scope::WebhooksManage,
            ]),
        )
    }

    fn rw_perms_with_scopes(owner: &str, control_plane_scopes: ScopeSet) -> TokenPermissions {
        TokenPermissions {
            read_only: false,
            owner_id: owner.to_string(),
            privacy_tier: PrivacyTier::LocalOnly,
            control_plane_scopes,
        }
    }

    #[test]
    fn missing_token_is_rejected() {
        let tokens = tokens_with("real-token", rw_perms("alice"));
        let result = authenticate_token(&tokens, None);
        assert!(result.is_err(), "absent x-bastion-token must be denied");
    }

    #[test]
    fn unknown_token_is_rejected() {
        let tokens = tokens_with("real-token", rw_perms("alice"));
        let meta = meta_with_token("wrong-token");
        let result = authenticate_token(&tokens, Some(&meta));
        assert!(result.is_err(), "unrecognized token must be denied");
    }

    #[test]
    fn empty_token_map_rejects_every_caller() {
        // WR-06: enabled-with-no-tokens is unreachable, not fail-open.
        let tokens: HashMap<String, TokenPermissions> = HashMap::new();
        let meta = meta_with_token("anything");
        assert!(authenticate_token(&tokens, Some(&meta)).is_err());
        assert!(authenticate_token(&tokens, None).is_err());
    }

    /// Regression (milestone-close code review, 2026-07-13): a misconfigured
    /// empty-string configured token must never authenticate a caller who
    /// presented no `x-bastion-token` at all — `constant_time_eq("", "")`
    /// would otherwise be `true`.
    #[test]
    fn empty_configured_token_never_authenticates_missing_header() {
        let tokens = tokens_with("", rw_perms("alice"));
        assert!(
            authenticate_token(&tokens, None).is_err(),
            "an empty configured token must never grant access to a caller with no token"
        );
        let meta = meta_with_token("");
        assert!(
            authenticate_token(&tokens, Some(&meta)).is_err(),
            "an empty configured token must never grant access to an explicit empty token either"
        );
    }

    #[test]
    fn valid_token_resolves_to_its_configured_permissions() {
        let tokens = tokens_with("real-token", rw_perms("alice"));
        let meta = meta_with_token("real-token");
        let perms =
            authenticate_token(&tokens, Some(&meta)).expect("valid token must authenticate");
        assert_eq!(perms.owner_id, "alice");
        assert!(!perms.read_only);
    }

    #[test]
    fn token_permissions_default_to_local_only_not_cloud_ok() {
        // CR-03: nothing constructs a TokenPermissions with CloudOk unless the
        // operator explicitly opted the token into it (McpServerTokenConfig.cloud_ok).
        let perms = rw_perms("alice");
        assert_eq!(perms.privacy_tier, PrivacyTier::LocalOnly);
    }

    #[test]
    fn constant_time_eq_matches_and_rejects_correctly() {
        assert!(constant_time_eq(b"same-token", b"same-token"));
        assert!(!constant_time_eq(b"same-token", b"different"));
        assert!(!constant_time_eq(b"short", b"much-longer-value"));
    }

    /// Pins today's 5-tool Control Plane MCP surface against
    /// `required_control_plane_scope`'s match arms — see that function's own
    /// doc comment on why an added-but-unmapped tool would silently ship
    /// ungated.
    #[test]
    fn scope_mapping_covers_every_registered_control_plane_tool() {
        for name in [
            "create_task",
            "get_task",
            "list_tasks",
            "steer_task",
            "cancel_task",
        ] {
            assert!(
                required_control_plane_scope(name).is_some(),
                "'{name}' must map to a required scope"
            );
        }
    }

    #[test]
    fn non_control_plane_tool_names_are_unaffected_by_scope_gating() {
        assert_eq!(required_control_plane_scope("browser"), None);
        assert_eq!(required_control_plane_scope("git"), None);
        assert!(check_control_plane_scope("browser", &ScopeSet::default()).is_ok());
    }

    #[test]
    fn read_and_list_require_tasks_read_only() {
        let scopes = ScopeSet::new([Scope::TasksRead]);
        assert!(check_control_plane_scope("get_task", &scopes).is_ok());
        assert!(check_control_plane_scope("list_tasks", &scopes).is_ok());
        assert!(check_control_plane_scope("create_task", &scopes).is_err());
        assert!(check_control_plane_scope("steer_task", &scopes).is_err());
        assert!(check_control_plane_scope("cancel_task", &scopes).is_err());
    }

    #[test]
    fn create_requires_tasks_create_specifically() {
        let scopes = ScopeSet::new([Scope::TasksCreate]);
        assert!(check_control_plane_scope("create_task", &scopes).is_ok());
        assert!(check_control_plane_scope("get_task", &scopes).is_err());
        assert!(check_control_plane_scope("cancel_task", &scopes).is_err());
    }

    #[test]
    fn steer_and_cancel_require_tasks_control() {
        let scopes = ScopeSet::new([Scope::TasksControl]);
        assert!(check_control_plane_scope("steer_task", &scopes).is_ok());
        assert!(check_control_plane_scope("cancel_task", &scopes).is_ok());
        assert!(check_control_plane_scope("get_task", &scopes).is_err());
        assert!(check_control_plane_scope("create_task", &scopes).is_err());
    }

    #[test]
    fn empty_scope_set_denies_every_control_plane_tool() {
        let scopes = ScopeSet::default();
        for name in [
            "create_task",
            "get_task",
            "list_tasks",
            "steer_task",
            "cancel_task",
        ] {
            assert!(
                check_control_plane_scope(name, &scopes).is_err(),
                "'{name}' must be denied with no granted scopes"
            );
        }
    }

    #[test]
    fn rw_perms_default_helper_grants_every_scope_matching_legacy_full_access_behavior() {
        // Backward compatibility: a token that predates this field (or an
        // operator who hasn't opted into narrower scoping) must keep
        // reaching every Control Plane tool exactly as it could before
        // control_plane_scopes existed.
        let perms = rw_perms("alice");
        for name in [
            "create_task",
            "get_task",
            "list_tasks",
            "steer_task",
            "cancel_task",
        ] {
            assert!(check_control_plane_scope(name, &perms.control_plane_scopes).is_ok());
        }
    }
}
