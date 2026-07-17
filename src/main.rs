use clap::{Parser, Subcommand};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tracing_subscriber::fmt;

use bastion_cognition::goal::{GoalEngine, ScoringConfig};
use bastion_cognition::proactive::CronService;
use bastion_mcp::McpClient;
use bastion_memory::sqlite::SqliteMemory;
use bastion_personas::persona::PersonaRegistry;
use bastion_providers::registry::resolve_provider;
use bastion_runtime::agent::handle;
use bastion_runtime::agent::loop_::AgentLoop;
use bastion_runtime::session::SessionManager;

/// Inicializa o OTel TracerProvider.
///
/// stdout exporter é opt-in via `BASTION_OTEL_STDOUT=true` (off por padrão — não polui o REPL).
/// Se `OTEL_EXPORTER_OTLP_ENDPOINT` estiver setado, adiciona OTLP/gRPC exporter.
///
/// SECURITY: não emite conteúdo de conversa por padrão —
/// `gen_ai.input.messages` só é adicionado se `BASTION_OTEL_CONTENT_EVENTS=true`.
///
/// PITFALL 6: deve ser chamado ANTES de AgentLoop::new() para que spans criados
/// dentro do AgentLoop não sejam descartados em silêncio (no-op tracer).
fn init_otel_provider() -> anyhow::Result<opentelemetry_sdk::trace::SdkTracerProvider> {
    use opentelemetry_sdk::trace::SdkTracerProvider;

    let mut provider_builder = SdkTracerProvider::builder();

    // stdout exporter opt-in — off por padrão p/ não afogar o REPL do daemon.
    // ponytail: era sempre-ligado; agora atrás de BASTION_OTEL_STDOUT=true.
    if std::env::var("BASTION_OTEL_STDOUT").as_deref() == Ok("true") {
        provider_builder =
            provider_builder.with_batch_exporter(opentelemetry_stdout::SpanExporter::default());
    }

    // OTLP exporter opcional — só se endpoint configurado
    let provider = if let Ok(endpoint) = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT") {
        use opentelemetry_otlp::{SpanExporter, WithExportConfig};
        let otlp_exporter = SpanExporter::builder()
            .with_tonic()
            .with_endpoint(&endpoint)
            .build()?;
        provider_builder.with_batch_exporter(otlp_exporter).build()
    } else {
        provider_builder.build()
    };

    Ok(provider)
}

#[derive(Parser)]
#[command(
    name = "bastion",
    about = "Bastion Life OS",
    long_about = "Open the Bastion terminal UI. With no subcommand, Bastion discovers or starts the local runtime automatically.",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Open the official terminal UI and connect to a Bastion daemon
    Chat {
        /// Base URL of the daemon webhook server
        #[arg(long, env = "BASTION_URL", default_value = "http://127.0.0.1:8080")]
        url: String,
        /// Existing owner token; prefer the BASTION_TOKEN environment variable
        #[arg(long, env = "BASTION_TOKEN")]
        token: Option<String>,
        /// Canonical owner associated with --token
        #[arg(long, env = "BASTION_OWNER_ID", default_value = "_local")]
        owner: String,
        /// Do not start a missing local runtime automatically
        #[arg(long)]
        no_auto_start: bool,
    },
    /// Execute a single-turn agent call and exit
    Agent {
        #[arg(short = 'm', long, help = "Message to send to the agent")]
        message: String,
    },
    /// Start long-running REPL daemon (reads stdin, responds, loops)
    Daemon,
    /// Companion state and agent-session event bridge
    Companion {
        #[command(subcommand)]
        action: CompanionAction,
    },
    /// Start MCP server over stdio (local subprocess transport).
    /// Used by local agents that control lifecycle (Claude Code, opencode, etc.).
    #[cfg(feature = "mcp-server")]
    McpStdio,
    /// Export agent identity, memories, goals, personas, and config to .af file
    Export {
        /// Export mode: full or template
        #[arg(long, default_value = "full")]
        mode: String,
        /// Include identity secrets in full exports
        #[arg(long)]
        with_identity: bool,
        /// Output path for the .af file
        #[arg(short = 'o', long)]
        output: String,
    },
    /// Import agent identity, memories, goals from .af file
    Import {
        /// Input path to the .af file (omit for stdin)
        input: Option<String>,
        /// Apply standalone personas and non-secret config; skills remain reviewable candidates
        #[arg(long)]
        apply_product_state: bool,
    },
}

#[derive(Subcommand)]
enum CompanionAction {
    /// Print level, needs, and game-mode status
    Status,
    /// Record an external coding-agent lifecycle event
    Event {
        /// session-start | activity | session-stop
        kind: String,
        /// Event source, for example claude, codex, or opencode
        #[arg(long, default_value = "external")]
        source: String,
    },
    /// Care for the companion: feed | water | play | sleep
    Care { action: String },
}

fn default_chat_command() -> Command {
    let url = std::env::var("BASTION_URL").unwrap_or_else(|_| {
        let port = std::env::var("BASTION_HTTP_PORT").unwrap_or_else(|_| "8080".to_string());
        format!("http://127.0.0.1:{port}")
    });
    Command::Chat {
        url,
        token: std::env::var("BASTION_TOKEN").ok(),
        owner: std::env::var("BASTION_OWNER_ID").unwrap_or_else(|_| "_local".to_string()),
        no_auto_start: false,
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load .env (if present) before any std::env::var read. Real shell env wins.
    dotenvy::dotenv().ok();

    let cli = Cli::parse();
    let command = cli.command.unwrap_or_else(default_chat_command);
    if let Command::Chat {
        url,
        token,
        owner,
        no_auto_start,
    } = &command
    {
        return bastion::tui::run(url, token.as_deref(), owner, !no_auto_start).await;
    }
    if let Command::Companion { action } = &command {
        let output = match action {
            CompanionAction::Status => bastion::tui::companion_status(),
            CompanionAction::Event { kind, source } => bastion::tui::companion_event(kind, source)?,
            CompanionAction::Care { action } => bastion::tui::companion_care(action)?,
        };
        println!("{output}");
        return Ok(());
    }

    // Load bastion.toml config (non-secret config only; secrets stay in .env)
    let config_path = std::env::var("BASTION_CONFIG").unwrap_or_else(|_| "bastion.toml".to_owned());
    let cfg = bastion::config::load_config(&config_path)?;

    // Init structured JSON logging
    std::fs::create_dir_all(
        std::path::Path::new(&cfg.logging.log_path)
            .parent()
            .unwrap_or_else(|| std::path::Path::new(".bastion")),
    )?;
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&cfg.logging.log_path)?;

    fmt()
        .json()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(log_file)
        .init();

    // Init SessionManager
    let db_path = cfg.session.db_path.clone();
    let session = SessionManager::new(&db_path);
    session.init_schema().await?;

    // D-02: auto-resume most recent session, or create new one
    let session_id = match session.load_most_recent_id().await? {
        Some(id) => {
            tracing::info!(event = "session_resumed", session_id = %id);
            id
        }
        None => {
            let id = session.create_session().await?;
            tracing::info!(event = "session_created", session_id = %id);
            id
        }
    };

    // Init MCP client from bastion.toml [mcp.servers] (D-09). connect_from_config handles
    // failed servers gracefully: logs tracing::warn per failed server and continues.
    // (Previously this used the legacy .bastion/mcp-servers.json path, which isn't mounted
    // in the FROM-scratch container — so memupalace/skill-writer tools were silently absent.)
    let mut mcp_client = McpClient::connect_from_config(&cfg.mcp.servers).await?;

    // SEC-03: Composio OAuth is opt-in — only constructed when COMPOSIO_API_KEY is
    // actually set. ComposioOAuth::new() itself panics on a missing/empty key (a
    // deliberate fail-loud contract for direct callers), so this guard is what keeps
    // the daemon from panicking at startup for deployments that simply don't use
    // Composio at all.
    let composio_oauth: Option<Arc<bastion_mcp::ComposioOAuth>> =
        if std::env::var("COMPOSIO_API_KEY").is_ok_and(|v| !v.trim().is_empty()) {
            let oauth = Arc::new(bastion_mcp::ComposioOAuth::new(&db_path));
            mcp_client = mcp_client.with_composio_oauth(oauth.clone());
            tracing::info!(event = "composio_oauth_enabled");
            Some(oauth)
        } else {
            None
        };
    // M2 (P3 `ToolSource`): shared by-Arc so it can back both the loop's
    // `ToolSource` port AND the Reflector's directly-registered `McpToolAdapter`
    // (below, in `daemon_loop`) — the SAME connected client, never a second
    // connection. Wrapped only after `with_composio_oauth` above, which still
    // needs owned `McpClient`.
    let mcp_client = Arc::new(mcp_client);
    let mcp_for_product = mcp_client.clone();

    // The reviewable TOML default can be overridden by a local `/model` choice
    // saved beside the persistent session database. This makes an interactive
    // provider switch survive a daemon restart without rewriting bastion.toml.
    let default_model = bastion::config::load_model_selection(&cfg)
        .unwrap_or_else(|| cfg.agent.default_model.clone());
    let provider: bastion_providers::SharedProvider =
        Arc::new(RwLock::new(resolve_provider(&default_model)?));

    let daily_budget = cfg.agent.daily_budget_usd;

    // Init persona registry (load from "./personas/" directory; empty if missing — PERS-07)
    let registry = PersonaRegistry::load_dir(".").await?;
    // M2 (P1 `Responder`): `registry` moves into `PersonaResponder` below — the
    // kernel no longer holds `AgentLoop.registry`. Several product-level
    // consumers (BastionMcpServer's resources, .af export/import) still need a
    // `PersonaRegistry` handle; keep a clone from BEFORE the move, same pattern
    // as `goals_for_product`/`mcp_for_product` above.
    let registry_for_product = registry.clone();
    let responder: Arc<dyn bastion_runtime::agent::ports::Responder> = Arc::new(
        bastion_personas::persona::responder::PersonaResponder::new(registry),
    );

    // Init shared memory
    let memory: bastion_memory::SharedMemory = Arc::new(RwLock::new(Box::new(SqliteMemory::new(
        &db_path,
    ))
        as Box<dyn bastion_memory::Memory>));

    // Init goal engine
    let goals = GoalEngine::new(&db_path, ScoringConfig::default());
    // M2 (P4 `GoalPort`): `agent.goals` becomes `Option<Arc<dyn GoalPort>>` below
    // — the loop only ever needed `list_goals`. Two other product-level
    // consumers still need the concrete `GoalEngine` (out of scope for this
    // cut, not loop internals): `BastionMcpServer` (its own `goals: GoalEngine`
    // field, src/mcp/server.rs) and `CronService` (needs `drift_nudge` too,
    // src/proactive/mod.rs) — both wired inside `daemon_loop`. Keep a plain
    // clone from BEFORE `goals` moves into the loop and thread it through,
    // rather than reaching into `agent.goals` (no longer the right type).
    let goals_for_product = goals.clone();

    // SEAM #4: inicializar OTel TracerProvider ANTES de AgentLoop::new()
    // (Pitfall 6: se chamado depois, spans no AgentLoop usariam no-op tracer)
    // OTel 0.32: SdkTracerProvider shuts down on drop — keep _otel_provider alive until end of main().
    let _otel_provider = init_otel_provider()
        .unwrap_or_else(|e| {
            tracing::warn!(event = "otel_init_failed", error = %e, "OTel init falhou — usando no-op tracer");
            opentelemetry_sdk::trace::SdkTracerProvider::builder().build()
        });
    opentelemetry::global::set_tracer_provider(_otel_provider.clone());

    let agent_identity: Option<Arc<bastion_mesh::identity::age_identity::AgeIdentity>> =
        if let Ok(identity_key) = std::env::var("MESH_IDENTITY_KEY") {
            match bastion_mesh::identity::age_identity::AgeIdentity::from_bech32(&identity_key) {
                Ok(id) => {
                    tracing::info!(event = "agent_identity_enabled");
                    Some(Arc::new(id))
                }
                Err(e) => {
                    // WR-03: route through the sanitizer instead of logging `e` directly —
                    // keeps the public-facing message generic even if a future error
                    // variant here ever carries secret material.
                    let sanitized = bastion_mesh::identity::age_identity::sanitised_identity_error(
                        &e.to_string(),
                    );
                    tracing::warn!(event = "agent_identity_init_failed", error = %sanitized);
                    None
                }
            }
        } else {
            tracing::info!(event = "agent_identity_disabled");
            None
        };

    // M2 step 3b (D2): the composition root builds the `ToolSource` port and
    // the SEAM #2 context providers, and populates the capability registry
    // from the connected MCP tools — the kernel constructor no longer does any
    // of this itself.
    // P3 `ToolSource` port: wraps the SAME Arc<McpClient> shared with the
    // McpToolAdapters registered into capability_registry below.
    let tool_source: std::sync::Arc<dyn bastion_runtime::agent::ports::ToolSource> =
        std::sync::Arc::new(bastion_mcp::McpToolSource::new(mcp_client.clone()));
    let mut agent = AgentLoop::new(
        provider.clone(),
        session,
        tool_source,
        session_id,
        daily_budget,
        responder,
        memory.clone(),
        Some(std::sync::Arc::new(goals)),
        cfg.agent.fallback_models.clone(),
        std::sync::Arc::new(bastion_runtime::capability::SqliteApprovalGate::new(
            &db_path,
        )),
        std::sync::Arc::new(bastion_cognition::eval::failure_sink::EvalFailureSink),
        bastion::agent::default_context_providers(&memory),
        // A3 `ProviderResolver`: registry-backed fallback-ladder resolution.
        std::sync::Arc::new(bastion_providers::registry::RegistryProviderResolver),
        // A1 `PreCompactionFlush`: MEM-09 dream flush, closing over the memory.
        Some(std::sync::Arc::new(
            bastion_cognition::agent::dream::DreamFlush::new(memory.clone()),
        )),
        // A2 `ToolResultObserver`: skill-writer hot-reload signal (D-06/Gap 1).
        Some(std::sync::Arc::new(
            bastion::agent::skills::SkillReloadObserver,
        )),
    );
    // BIG-1 (Gap 2): one McpToolAdapter per connected MCP tool, into the SAME
    // registry instance the loop owns (moved verbatim out of `AgentLoop::new`).
    bastion_mcp::registry_setup::register_mcp_tools(&mut agent.capability_registry, &mcp_client);
    agent.capability_registry.register(Arc::new(
        bastion::companion_capability::CompanionEventCapability::new(),
    ))?;

    // Ciclo 2.4 (`docs/revamp/C2-backend-profile-design.md` §2): build the
    // RuntimeRegistry from whatever AgentRuntime adapters are actually
    // healthy on this host RIGHT NOW — conditional registration, an
    // unhealthy adapter never enters the map (an owner who then selects it
    // gets the fail-closed typed error from `RuntimeRegistry::resolve` at
    // turn start, never a silent fallback to Model). Cheap to build even
    // when `[backend]` is entirely absent from bastion.toml: `health()` here
    // is a handful of `--version` subprocess spawns, not a live session.
    let runtime_registry = bastion::agent_runtime_registry::build_runtime_registry().await;

    let mut backend_profile = bastion::config::backend_profile_from_config(&cfg.backend);
    if let bastion_runtime::agent::backend::ConversationBackend::Runtime(id) =
        &backend_profile.conversation
    {
        match runtime_registry.get(id) {
            // coverage_note is a pass-through of the adapter's own honest
            // declaration (RuntimeDescriptor::policy_coverage) — never
            // invented from the config string.
            Some(rt) => backend_profile.coverage_note = Some(rt.descriptor().policy_coverage),
            None => tracing::warn!(
                event = "backend_conversation_runtime_not_registered",
                runtime_id = %id,
                "configured conversation backend is not in the RuntimeRegistry (missing binary/auth/health) — \
                 the turn will fail closed with a typed error at turn start, never silently fall back to Model",
            ),
        }
    }
    // M4-07 (docs/revamp/BACKLOG.md): verify every configured `[auth.<profile>]`
    // entry against the live host (by reference only — no token ever read/
    // logged, see auth_profile_registry.rs) and wire the result as the
    // AuthResolver a runtime-backed turn checks before start/resume. Cheap
    // even with zero `[auth.*]` sections: the loop over an empty map does
    // nothing, and AgentLoop's own NullAuthResolver default (unchanged if
    // this call is ever removed) already preserves pre-M4-07 behavior.
    let auth_resolver = bastion::auth_profile_registry::AuthProfileRegistry::build(&cfg.auth).await;

    agent = agent
        .with_backend_profile(backend_profile)
        .with_runtime_registry(runtime_registry)
        .with_auth_resolver(std::sync::Arc::new(auth_resolver))
        // Loop 3-A (6a, docs/revamp/C3-runtime-followups-design.md §6a):
        // owner-scoped, persisted cross-turn permission queue — the same
        // db_path SqliteApprovalGate above already opens. Without this call
        // AgentLoop keeps NullPermissionGate (fail-closed immediate deny,
        // pre-6a behavior); wiring the real gate here is what lets a
        // delegated task's paused PermissionRequest survive to be resolved
        // by a LATER turn instead of denying instantly.
        .with_permission_gate(std::sync::Arc::new(
            bastion_runtime::capability::SqlitePermissionGate::new(&db_path),
        ));

    match command {
        Command::Chat { .. } => unreachable!("chat is handled before daemon initialization"),
        Command::Companion { .. } => {
            unreachable!("companion is handled before daemon initialization")
        }
        Command::Agent { message } => {
            let response = agent.run_turn(&message).await?;
            println!("{}", response);
        }
        Command::Daemon => {
            let secret_resolver: Arc<dyn bastion_types::SecretResolver> =
                Arc::new(bastion::secret::default_secret_resolver());
            daemon_loop(
                &mut agent,
                &cfg,
                agent_identity,
                composio_oauth,
                goals_for_product,
                mcp_for_product,
                registry_for_product,
                secret_resolver,
            )
            .await?;
        }
        #[cfg(feature = "mcp-server")]
        Command::McpStdio => {
            use rmcp::ServiceExt;

            let token_perms = build_token_perms(&cfg);
            let local_owner = std::env::var("BASTION_OWNER_ID")
                .unwrap_or_else(|_| bastion_runtime::agent::loop_::DEFAULT_OWNER.to_string());
            let personas = Arc::new(registry_for_product.clone());
            let mcp_server = bastion::mcp::server::BastionMcpServer::new(
                Arc::new(agent.capability_registry.clone()),
                memory.clone(),
                personas,
                goals_for_product.clone(),
                token_perms,
                local_owner,
            );
            let (stdin, stdout) = rmcp::transport::stdio();
            tracing::info!(event = "mcp_stdio_started", "MCP stdio server starting");
            let running = mcp_server
                .serve((stdin, stdout))
                .await
                .map_err(|e| anyhow::anyhow!("MCP stdio server error: {}", e))?;
            running
                .waiting()
                .await
                .map_err(|e| anyhow::anyhow!("MCP stdio server terminated: {}", e))?;
        }
        Command::Export {
            mode,
            with_identity,
            output,
        } => {
            let owner_id = std::env::var("BASTION_OWNER_ID")
                .unwrap_or_else(|_| bastion_runtime::agent::loop_::DEFAULT_OWNER.to_string());

            let af = match mode.as_str() {
                "full" => {
                    let identity = if with_identity {
                        agent_identity.as_deref()
                    } else {
                        None
                    };
                    bastion_mesh::interop::export::export_full(
                        &memory,
                        &registry_for_product,
                        &goals_for_product,
                        &cfg.agent,
                        identity,
                        &owner_id,
                    )
                    .await?
                }
                "template" => {
                    if with_identity {
                        anyhow::bail!("--with-identity is only valid with --mode full");
                    }
                    bastion_mesh::interop::export::export_template(
                        &registry_for_product,
                        &cfg.agent,
                    )
                    .await?
                }
                other => {
                    anyhow::bail!("Invalid export mode '{}'. Use 'full' or 'template'.", other)
                }
            };

            let json = serde_json::to_string_pretty(&af)?;
            tokio::fs::write(&output, &json).await?;
            // WR-04: --with-identity embeds the age + Ed25519 SECRET keys in plaintext —
            // this file is the trust root for the entire mesh identity. Restrict to
            // owner-read-write before anything else touches it; the process umask alone
            // (commonly 0644) would leave it group/world-readable on a shared host.
            #[cfg(unix)]
            if with_identity {
                use std::os::unix::fs::PermissionsExt;
                tokio::fs::set_permissions(&output, std::fs::Permissions::from_mode(0o600)).await?;
            }
            tracing::info!(event = "export_complete", mode = %mode, output = %output);
            println!("Exported agent to {output}");
        }
        Command::Import {
            input,
            apply_product_state,
        } => {
            let owner_id = std::env::var("BASTION_OWNER_ID")
                .unwrap_or_else(|_| bastion_runtime::agent::loop_::DEFAULT_OWNER.to_string());

            let json = match input {
                Some(path) => tokio::fs::read_to_string(&path).await?,
                None => {
                    // Read from stdin
                    let mut buf = String::new();
                    use std::io::Read;
                    std::io::stdin().read_to_string(&mut buf)?;
                    buf
                }
            };
            let af: bastion_mesh::interop::AgentFile = serde_json::from_str(&json)?;
            let managed = std::env::var("BASTION_DEPLOYMENT_MODE")
                .map(|mode| mode.eq_ignore_ascii_case("managed"))
                .unwrap_or(false);
            let product_import = bastion::product_import::PreparedProductImport::prepare(
                &af,
                apply_product_state,
                managed,
                std::path::Path::new(&config_path),
            )?;
            let restored = bastion_mesh::interop::import::import(
                af,
                &memory,
                &registry_for_product,
                &goals_for_product,
                &owner_id,
            )
            .await?;
            product_import.commit()?;
            if let Some(id) = restored {
                let age_secret = id.age_secret_bech32();
                println!("Import complete. Identity restored.");
                println!("Set MESH_IDENTITY_KEY={age_secret} for mesh use.");
            } else {
                println!("Import complete (no identity in file).");
            }
            if managed && apply_product_state {
                println!("Managed deployment: persona, skill, and config blocks were not applied locally.");
            } else if apply_product_state {
                println!("Personas and non-secret config applied; skills staged as reviewable candidates.");
            }
        }
    }

    // SEAM #4: flush e shutdown do OTel para não perder spans buffered.
    // OTel 0.32: SdkTracerProvider::shutdown() flushes all batch processors.
    // _otel_provider is still alive (owns the processors) — explicit shutdown before drop.
    let _ = _otel_provider.shutdown();

    Ok(())
}

/// REPL daemon loop: stdin line by line, slash commands, graceful shutdown (D-01).
/// Five select arms: stdin, pending_rx (proactive), inbound_rx (channel), SIGTERM, Ctrl-C.
/// All arms serialize through ONE `&mut agent` — single-turn invariant holds (CR-07).
// Wires 8 independent startup dependencies from `main()`; a params struct would be a
// single-call-site bag with no reusable shape (same rationale as `serve_with_mesh` below).
#[allow(clippy::too_many_arguments)]
async fn daemon_loop(
    agent: &mut AgentLoop,
    cfg: &bastion::config::BastionConfig,
    agent_identity: Option<Arc<bastion_mesh::identity::age_identity::AgeIdentity>>,
    // SEC-03: opt-in Composio OAuth client (Some only when COMPOSIO_API_KEY is
    // configured) — wired into both the agent (/connect-app-composio) and the
    // webhook server's /auth/composio/callback route below.
    composio_oauth: Option<Arc<bastion_mcp::ComposioOAuth>>,
    // M2 (P4 `GoalPort`): concrete `GoalEngine` for the two product-level
    // consumers `daemon_loop` wires up (`BastionMcpServer`'s MCP-over-HTTP
    // resources, `CronService`'s heartbeat) that need more than the loop's
    // `list_goals`-only port surface. Same underlying engine `agent.goals`
    // wraps — cloned in `main()` before it moved into the loop.
    goals_for_product: GoalEngine,
    // M2 (P3 `ToolSource`): concrete `Arc<McpClient>` for the Reflector's
    // directly-registered `McpToolAdapter` below — the SAME connected client
    // `agent.tool_source` wraps, shared by-Arc from `main()`.
    mcp_for_product: Arc<McpClient>,
    // M2 (P1 `Responder`): concrete `PersonaRegistry` for `BastionMcpServer`'s
    // resources and `CommandResources` (`/as`/`/cabinet` validation) — the SAME
    // registry `PersonaResponder` wraps, cloned in `main()` before it moved
    // into the responder.
    registry_for_product: PersonaRegistry,
    // Loop 3-D (`docs/revamp/C3-cloud-ready-design.md`, security point 1):
    // the injectable resolver every daemon-level `SecretRef` (currently
    // `APP_JWT_SECRET`, `BASTION_INFER_TOKEN`) is resolved through at boot —
    // env var today, optionally a mounted-secrets directory
    // (`BASTION_SECRETS_DIR`); a hosted operator's own secret manager is a
    // drop-in replacement built in `main()`, never a daemon_loop change.
    secret_resolver: Arc<dyn bastion_types::SecretResolver>,
) -> anyhow::Result<()> {
    use bastion::agent::command::{CommandResources, CommandResult};
    use tokio::io::{AsyncBufReadExt, BufReader};
    use tokio::signal::unix::{signal, SignalKind};

    // PROACT-05: take pending_rx out of the agent so we own it in the select! loop.
    // Because select! processes ONE branch per iteration and run_turn fully awaits,
    // a pending message is only picked up BETWEEN turns — this IS the structural guarantee.
    let mut pending_rx = agent
        .pending_rx
        .take()
        .expect("pending_rx must be available at daemon start");

    // Loop 3-D (`docs/revamp/C3-cloud-ready-design.md`): session/memory/
    // provider are guaranteed initialized by the time `daemon_loop` is ever
    // called — `main()` already propagated any of their own init failures
    // before dispatching to `Command::Daemon` — so they're marked ready
    // right here, at the top. `channels` is marked ready only once every
    // configured channel below has finished its spawn attempt, right before
    // the `select!` loop starts (see near the bottom of this function).
    let readiness = bastion::channel::operational::ReadinessState::new();
    readiness.mark_session_ready();
    readiness.mark_memory_ready();
    readiness.mark_provider_ready();
    let lifecycle_auth = bastion::channel::operational::DaemonAccessAuth::new(
        secret_resolver
            .resolve("BASTION_DAEMON_TOKEN")
            .ok()
            .map(|v| v.expose_secret().to_string()),
    );
    let lifecycle = bastion::channel::operational::LifecycleControl::new(lifecycle_auth);

    // M2 (P5 despejo): `otc_store`/`composio_oauth` are no longer `AgentLoop`
    // fields — this replaces `agent.set_otc_store`/`agent.set_composio_oauth`.
    // Declared here (before the webhook-gated block below, which is the only
    // place that ever populates otc_store/composio_oauth — same condition as
    // before) so BOTH select! arms below (`stdin` and `inbound_rx`) that call
    // `agent.handle_command` see the same resources the removed setters used
    // to inject onto `agent`.
    // M2 (P1 `Responder`): `registry` is set unconditionally right away
    // (unlike otc_store/composio_oauth) — `/as`/`/cabinet` validation needs it
    // regardless of whether the webhook channel is running, matching the
    // original `self.registry` field's always-present behavior.
    let mut command_resources = CommandResources {
        registry: registry_for_product.clone(),
        model_selection: Some(bastion::agent::command::ModelSelection {
            path: bastion::config::model_selection_path(cfg),
            default_model: cfg.agent.default_model.clone(),
        }),
        ..Default::default()
    };

    // CR-07: create AgentHandle + inbound receiver BEFORE the select! loop.
    // Channels (Telegram, webhook) hold clones of `handle` and send messages into `inbound_rx`.
    // The select! arm below serializes all channel turns through the SAME agent as stdin/proactive.
    let (agent_handle, mut inbound_rx) = handle::channel();

    // CHAN-02/D-05: OwnerMaps for ALL 7 channels are now projected from the single
    // `[[identity]]` table (bastion::config::owner_map_for_*) instead of the old
    // scattered per-channel env vars (BASTION_WEBHOOK_OWNERS/BASTION_TELEGRAM_OWNERS).
    // This is the plan 10-09 deliverable that makes CHAN-02's "unified owner-based
    // routing" claim literally true — one mechanism, not N.
    let mut webhook_owner_map = bastion::config::owner_map_for_webhook(&cfg.identity);
    if let Ok(token) = std::env::var("BASTION_BOOTSTRAP_TOKEN") {
        if !token.trim().is_empty() {
            let owner = std::env::var("BASTION_OWNER_ID")
                .unwrap_or_else(|_| bastion_runtime::agent::loop_::DEFAULT_OWNER.to_string());
            webhook_owner_map.0.insert(token, owner);
        }
    }
    let telegram_owner_map = bastion::config::owner_map_for_telegram(&cfg.identity);
    #[cfg(feature = "channels-extra")]
    let whatsapp_owner_map = bastion::config::owner_map_for_whatsapp(&cfg.identity);
    #[cfg(feature = "channels-extra")]
    let discord_owner_map = bastion::config::owner_map_for_discord(&cfg.identity);
    #[cfg(feature = "channels-extra")]
    let slack_owner_map = bastion::config::owner_map_for_slack(&cfg.identity);
    #[cfg(feature = "channels-extra")]
    let email_owner_map = bastion::config::owner_map_for_email(&cfg.identity);

    // A channel starts only when it is enabled in bastion.toml AND its required
    // secret/address is present in the environment.
    if cfg.channels.webhook.enabled {
        if let Ok(addr) = std::env::var("BASTION_WEBHOOK_ADDR") {
            let h = agent_handle.clone();
            let owner_map = webhook_owner_map;
            // Phase 6: mesh connectivity — load peers from config, create broadcast channel for SSE.
            let (events_tx, _) = tokio::sync::broadcast::channel::<String>(128);
            let peer_map_initial = bastion::config::load_mesh_peers(cfg);
            let mesh_peer_map = Arc::new(RwLock::new(peer_map_initial));
            // WR-01/CR-04: APP_JWT_SECRET must be set — no insecure fallback. This is the
            // actual `bastion daemon` startup path (serve_with_mesh performs no validation
            // of its own); only `WebhookChannel::run`, which daemon_loop never calls, had
            // the fail-closed check. Fail here instead of silently signing/verifying JWTs
            // with a well-known default that anyone reading this public repo can use to
            // impersonate any owner.
            //
            // Loop 3-D: resolved BY REFERENCE through the injected
            // `SecretResolver` (env var today; a mounted-file/hosted secret
            // manager transparently for an operator that sets
            // `BASTION_SECRETS_DIR` or injects their own resolver in `main()`)
            // rather than reading `std::env::var` directly — same contract as
            // `BASTION_INFER_TOKEN` below.
            let jwt_secret = secret_resolver
                .resolve("APP_JWT_SECRET")
                .map_err(|_| {
                    tracing::error!(
                        event = "webhook_no_jwt_secret",
                        "APP_JWT_SECRET is not set — refusing to start"
                    );
                    anyhow::anyhow!(
                        "APP_JWT_SECRET must be set; refusing to start with a hardcoded default"
                    )
                })?
                .expose_secret()
                .to_string();

            // Phase 6 Wave 2: P2PTransport + MeshSliceProvider when MESH_IDENTITY_KEY is set.
            let (mesh_transport, mesh_slice_store) = if let Ok(identity_key) =
                std::env::var("MESH_IDENTITY_KEY")
            {
                let local_owner = std::env::var("BASTION_OWNER_ID")
                    .unwrap_or_else(|_| bastion_runtime::agent::loop_::DEFAULT_OWNER.to_string());
                let transport = bastion_mesh::mesh::p2p::P2PTransport::new(
                    local_owner.clone(),
                    identity_key,
                    mesh_peer_map.clone(),
                    events_tx.clone(),
                );
                let shared: bastion_mesh::mesh::SharedMeshTransport = Arc::new(transport);

                // MeshSliceProvider::new returns (provider, store); build the `from_store`
                // provider here (M2 P5 despejo — `add_mesh_slice_provider` is gone from the
                // loop; it only receives an already-built `TurnContextProvider` boxed now).
                let (_, store) = bastion_mesh::mesh::context_provider::MeshSliceProvider::new(
                    local_owner.clone(),
                );
                // WR-06: mirrors the removed `add_mesh_slice_provider`'s OWN owner
                // resolution exactly (BASTION_OWNER_ID, then MESH_OWNER_ID, then
                // DEFAULT_OWNER) — deliberately NOT the outer `local_owner` above (no
                // MESH_OWNER_ID fallback there); preserved verbatim, not reconciled.
                let local_owner_for_mesh_provider = std::env::var("BASTION_OWNER_ID")
                    .or_else(|_| std::env::var("MESH_OWNER_ID"))
                    .unwrap_or_else(|_| bastion_runtime::agent::loop_::DEFAULT_OWNER.to_string());
                let mesh_provider =
                    bastion_mesh::mesh::context_provider::MeshSliceProvider::from_store(
                        local_owner_for_mesh_provider,
                        store.clone(),
                    );
                agent.add_context_provider(Box::new(mesh_provider));
                tracing::info!(
                    event = "mesh_slice_provider_registered",
                    "MeshSliceProvider registered in context_providers (SEAM #2)"
                );

                // Periodic mesh sync (mesh.sync_interval minutes, default 15; 0 = disable)
                let sync_interval = cfg.mesh.sync_interval;
                let _mesh_sync_handle = bastion_mesh::scheduler::cron::spawn_mesh_sync_job(
                    shared.clone(),
                    mesh_peer_map.clone(),
                    agent.memory.clone(),
                    local_owner,
                    sync_interval,
                );
                tracing::info!(
                    event = "mesh_transport_enabled",
                    sync_interval_minutes = sync_interval
                );

                (Some(shared), Some(store))
            } else {
                tracing::info!(
                    event = "mesh_transport_disabled",
                    "MESH_IDENTITY_KEY not set — mesh disabled"
                );
                (None, None)
            };

            // agent_identity was already loaded above (line ~170) — reuse outer scope.
            let agent_name =
                std::env::var("BASTION_AGENT_NAME").unwrap_or_else(|_| "bastion".to_string());

            // Build MCP Streamable HTTP server if enabled.
            // M3-05: compiled only under the `mcp-server` feature; without it,
            // `mcp_routes` is always `None` (and an enabled config is warned about).
            #[cfg(feature = "mcp-server")]
            let mcp_routes = if cfg.mcp_server.enabled {
                // Clone AgentLoop components that BastionMcpServer needs.
                let cap_registry = Arc::new(agent.capability_registry.clone());
                let mem = agent.memory.clone();
                let personas = Arc::new(registry_for_product.clone());
                let goals = goals_for_product.clone();
                let local_owner = std::env::var("BASTION_OWNER_ID")
                    .unwrap_or_else(|_| bastion_runtime::agent::loop_::DEFAULT_OWNER.to_string());
                let token_perms = build_token_perms(cfg);
                // WR-06: after CR-01's fail-closed auth fix, an empty token map means the
                // server is enabled but permanently unreachable (no token can ever match) —
                // the safe direction, but still a likely operator mistake worth surfacing
                // instead of a silent "MCP server doesn't work" support report.
                if token_perms.is_empty() {
                    tracing::warn!(
                    event = "mcp_server_no_tokens_configured",
                    "mcp_server.enabled=true but [mcp_server.tokens] is empty — no client can authenticate"
                );
                }
                let router = bastion::mcp::server::build_mcp_axum_router(
                    cap_registry,
                    mem,
                    personas,
                    goals,
                    token_perms,
                    local_owner,
                    &cfg.mcp_server.mount_path,
                );
                tracing::info!(
                    event = "mcp_server_enabled",
                    mount_path = %cfg.mcp_server.mount_path,
                );
                Some(router)
            } else {
                tracing::info!(event = "mcp_server_disabled");
                None
            };
            #[cfg(not(feature = "mcp-server"))]
            let mcp_routes: Option<axum::Router> = {
                if cfg.mcp_server.enabled {
                    tracing::warn!(
                    event = "mcp_server_not_compiled",
                    "mcp_server.enabled=true but this binary was built without the `mcp-server` feature"
                );
                }
                None
            };

            // CR-02: create an OtcStore and pass it to serve_with_mesh so skill commands
            // can insert BAST-XXXX codes for /auth/exchange and /mesh/pair.
            // The same Arc is injected into the agent so the /connect-app REPL command
            // writes codes the webhook server reads (06-08 OTC-writer wiring).
            let otc_store = bastion::channel::webhook::new_otc_store();
            command_resources.otc_store = Some(otc_store.clone());

            // SEC-03: mirrors the OTC store wiring above — inject the same ComposioOAuth
            // Arc into both the command dispatch (for /connect-app-composio) and
            // serve_with_mesh (for the /auth/composio/callback route), only when configured.
            if let Some(oauth) = &composio_oauth {
                command_resources.composio_oauth = Some(oauth.clone());
            }

            // WhatsApp (CHAN-01): reuses this same webhook router (10-RESEARCH.md
            // Pattern 1) — no second axum server. `WHATSAPP_PHONE_NUMBER_ID` presence
            // gates whether we attempt to build a sender at all.
            // M3-05: runtime wiring gated under `channels-extra` (the module itself
            // always compiles — its types thread through the webhook router).
            #[cfg(feature = "channels-extra")]
            let whatsapp_config = if cfg.channels.whatsapp.as_ref().is_some_and(|c| c.enabled)
                && std::env::var("WHATSAPP_PHONE_NUMBER_ID").is_ok()
            {
                match bastion::channel::whatsapp::WhatsAppSender::from_env() {
                    Ok(sender) => Some(bastion::channel::whatsapp::WhatsAppConfig {
                        owner_map: whatsapp_owner_map,
                        sender: std::sync::Arc::new(sender),
                    }),
                    Err(e) => {
                        tracing::warn!(event = "whatsapp_start_failed", error = %e);
                        None
                    }
                }
            } else {
                None
            };
            #[cfg(not(feature = "channels-extra"))]
            let whatsapp_config: Option<bastion::channel::whatsapp::WhatsAppConfig> = {
                if std::env::var("WHATSAPP_PHONE_NUMBER_ID").is_ok() {
                    tracing::warn!(
                    event = "whatsapp_not_compiled",
                    "WHATSAPP_PHONE_NUMBER_ID is set but this binary was built without the `channels-extra` feature"
                );
                }
                None
            };

            // Cloned BEFORE the `async move` block below — `readiness`/`lifecycle`
            // are used again later in `daemon_loop` (readiness.mark_channels_ready()
            // right before the select! loop; the shutdown/reload arms inside it),
            // so the ORIGINAL bindings must survive this spawn, not be moved into it.
            let readiness_for_webhook = readiness.clone();
            let lifecycle_for_webhook = lifecycle.clone();
            tokio::spawn(async move {
                if let Err(e) = bastion::channel::webhook::serve_with_mesh(
                    h,
                    &addr,
                    owner_map,
                    events_tx,
                    mesh_peer_map,
                    jwt_secret,
                    mesh_transport,
                    mesh_slice_store,
                    otc_store,
                    agent_identity,
                    agent_name,
                    mcp_routes,
                    whatsapp_config,
                    composio_oauth.clone(),
                    readiness_for_webhook,
                    lifecycle_for_webhook,
                )
                .await
                {
                    tracing::error!(event = "webhook_error", error = %e, "webhook channel terminated");
                }
            });
            tracing::info!(event = "webhook_started", addr = %std::env::var("BASTION_WEBHOOK_ADDR").unwrap_or_default());
        } else {
            tracing::warn!(
                event = "webhook_enabled_without_addr",
                "channels.webhook.enabled=true but BASTION_WEBHOOK_ADDR is not set"
            );
        }
    } else {
        #[cfg(feature = "channels-extra")]
        if std::env::var("WHATSAPP_PHONE_NUMBER_ID").is_ok() {
            tracing::warn!(
                event = "whatsapp_requires_webhook_addr",
                "WHATSAPP_PHONE_NUMBER_ID is set but BASTION_WEBHOOK_ADDR is not — WhatsApp mounts on the webhook router and cannot start without it"
            );
        }
    }

    if cfg.channels.telegram.enabled && std::env::var("TELEGRAM_BOT_TOKEN").is_ok() {
        match bastion::channel::telegram::TelegramChannel::from_env() {
            Ok(tg) => {
                let tg = tg.with_owner_map(telegram_owner_map);
                let h = agent_handle.clone();
                tokio::spawn(async move {
                    use bastion::channel::Channel;
                    if let Err(e) = Box::new(tg).run(h).await {
                        tracing::error!(event = "telegram_error", error = %e, "telegram channel terminated");
                    }
                });
                tracing::info!(event = "telegram_started");
            }
            Err(e) => {
                tracing::warn!(event = "telegram_start_failed", error = %e);
            }
        }
    }

    // Spawn Discord channel if DISCORD_BOT_TOKEN is set (CHAN-03).
    // M3-05: compiled only under `channels-extra` (serenity dep).
    #[cfg(feature = "channels-extra")]
    if cfg.channels.discord.as_ref().is_some_and(|c| c.enabled)
        && std::env::var("DISCORD_BOT_TOKEN").is_ok()
    {
        match bastion::channel::discord::DiscordChannel::from_env() {
            Ok(ch) => {
                let ch = ch.with_owner_map(discord_owner_map);
                let h = agent_handle.clone();
                tokio::spawn(async move {
                    use bastion::channel::Channel;
                    if let Err(e) = Box::new(ch).run(h).await {
                        tracing::error!(event = "discord_error", error = %e, "discord channel terminated");
                    }
                });
                tracing::info!(event = "discord_started");
            }
            Err(e) => {
                tracing::warn!(event = "discord_start_failed", error = %e);
            }
        }
    }

    // Spawn Slack channel if SLACK_BOT_TOKEN and SLACK_APP_TOKEN are set (CHAN-03).
    // M3-05: compiled only under `channels-extra` (slack-morphism dep).
    #[cfg(feature = "channels-extra")]
    if cfg.channels.slack.as_ref().is_some_and(|c| c.enabled)
        && std::env::var("SLACK_BOT_TOKEN").is_ok()
        && std::env::var("SLACK_APP_TOKEN").is_ok()
    {
        match bastion::channel::slack::SlackChannel::from_env() {
            Ok(ch) => {
                let ch = ch.with_owner_map(slack_owner_map);
                let h = agent_handle.clone();
                tokio::spawn(async move {
                    use bastion::channel::Channel;
                    if let Err(e) = Box::new(ch).run(h).await {
                        tracing::error!(event = "slack_error", error = %e, "slack channel terminated");
                    }
                });
                tracing::info!(event = "slack_started");
            }
            Err(e) => {
                tracing::warn!(event = "slack_start_failed", error = %e);
            }
        }
    }

    // Spawn Email channel if EMAIL_ADDRESS is set (CHAN-03).
    // M3-05: compiled only under `channels-extra` (lettre/async-imap deps).
    #[cfg(feature = "channels-extra")]
    if cfg.channels.email.as_ref().is_some_and(|c| c.enabled)
        && std::env::var("EMAIL_ADDRESS").is_ok()
    {
        match bastion::channel::email::EmailChannel::from_env() {
            Ok(ch) => {
                let ch = ch.with_owner_map(email_owner_map);
                let h = agent_handle.clone();
                tokio::spawn(async move {
                    use bastion::channel::Channel;
                    if let Err(e) = Box::new(ch).run(h).await {
                        tracing::error!(event = "email_error", error = %e, "email channel terminated");
                    }
                });
                tracing::info!(event = "email_started");
            }
            Err(e) => {
                tracing::warn!(event = "email_start_failed", error = %e);
            }
        }
    }

    // Spawn Voice channel if [channels.voice].enabled (VOICE-01). No secret env var to
    // gate on — voice authenticates via local mic/speaker hardware presence, not a
    // remote credential. `voice_transcribe`/`voice_speak` are already present in the
    // SAME registry AgentLoop::new() populated (auto-classified is_local_override=true
    // by Plan 10-08's [mcp.servers.voice].is_local=true wiring) — no manual
    // registration call is needed here.
    // M3-05: compiled only under `voice` (cpal/hound/rustpotter deps).
    #[cfg(feature = "voice")]
    if cfg.channels.voice.enabled {
        let voice_registry = Arc::new(agent.capability_registry.clone());
        let vc = bastion::channel::voice::VoiceChannel::new(
            voice_registry,
            cfg.channels.voice.voice.clone(),
            cfg.channels.voice.wake_word_enabled,
        );
        let h = agent_handle.clone();
        tokio::spawn(async move {
            use bastion::channel::Channel;
            if let Err(e) = Box::new(vc).run(h).await {
                tracing::error!(event = "voice_error", error = %e, "voice channel terminated");
            }
        });
        tracing::info!(event = "voice_started");
    }
    #[cfg(not(feature = "voice"))]
    if cfg.channels.voice.enabled {
        tracing::warn!(
            event = "voice_not_compiled",
            "channels.voice.enabled=true but this binary was built without the `voice` feature"
        );
    }

    // Spawn /api/infer gateway for Python MCP containers (D-08 / D-09).
    // Port: BASTION_INFER_ADDR env var, default "127.0.0.1:3000" (loopback).
    // Python containers call this endpoint; they never hold raw API keys.
    //
    // SEC (unauthenticated token-minting): this endpoint proxies inference using
    // Bastion's provider credentials, so it MUST NOT be reachable unauthenticated.
    // Defense in depth:
    //   1. Default bind is loopback; widening requires explicit BASTION_INFER_ADDR.
    //   2. BASTION_INFER_TOKEN enforces `Authorization: Bearer <token>` per request.
    //   3. Fail closed: refuse to bind a non-loopback interface without a token.
    // In Docker, the token is injected and the port stays on a private, unpublished
    // network (see plan 03-06).
    {
        // Loop 3-D: resolved BY REFERENCE through the same injected
        // `SecretResolver` as `APP_JWT_SECRET` above — `resolve` already
        // fails closed on an absent/empty value (`EnvSecretResolver`), so
        // `Err` here means exactly what the old `.ok().filter(non_empty)`
        // chain meant: "no usable token configured".
        let infer_token = secret_resolver
            .resolve("BASTION_INFER_TOKEN")
            .ok()
            .map(|v| v.expose_secret().to_string());
        let infer_addr =
            std::env::var("BASTION_INFER_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".to_owned());
        let host = infer_addr
            .rsplit_once(':')
            .map(|(h, _)| h)
            .unwrap_or(&infer_addr);
        let is_loopback =
            host == "127.0.0.1" || host == "::1" || host == "[::1]" || host == "localhost";

        if infer_token.is_none() && !is_loopback {
            tracing::error!(
                event = "infer_gateway_refused",
                addr = %infer_addr,
                "refusing to expose /api/infer on a non-loopback interface without BASTION_INFER_TOKEN (SEC: unauthenticated token-minting)"
            );
        } else {
            if infer_token.is_none() {
                tracing::warn!(
                    event = "infer_gateway_no_auth",
                    addr = %infer_addr,
                    "/api/infer running without BASTION_INFER_TOKEN — loopback-only dev mode"
                );
            }
            let infer_router = bastion::api::infer::router(agent.provider.clone(), infer_token);
            tokio::spawn(async move {
                match tokio::net::TcpListener::bind(&infer_addr).await {
                    Ok(listener) => {
                        tracing::info!(event = "infer_gateway_started", addr = %infer_addr);
                        if let Err(e) = axum::serve(listener, infer_router).await {
                            tracing::error!(event = "infer_gateway_error", error = %e);
                        }
                    }
                    Err(e) => {
                        tracing::error!(event = "infer_gateway_bind_failed", addr = %infer_addr, error = %e);
                    }
                }
            });
        }
    }

    // Spawn CronService heartbeat into the pending queue (PROACT-01 / PROACT-02).
    // It feeds goal-drift nudges into pending_tx. On tick the daemon will pick them up
    // between turns via the pending_rx arm.
    {
        let cron = CronService::new(agent.pending_tx.clone(), goals_for_product.clone());
        let owner = bastion_runtime::agent::loop_::DEFAULT_OWNER.to_string();
        // Only spawn heartbeat if there are goals to nudge (fire-and-forget task)
        tokio::spawn(async move {
            cron.run_heartbeat(std::time::Duration::from_secs(86_400), &owner)
                .await;
        });
    }

    // LEARN-02/LEARN-05: spawn the offline Reflector. Budget/interval/model/dedup-cadence
    // come from bastion.toml [reflector] (defaults if absent). Never reachable from a
    // user-facing turn (ADR D-4) — this is a separate tokio::spawn, same idiom as
    // CronService::run_heartbeat and spawn_mesh_sync_job above.
    {
        // Minimal registry scoped to exactly what the Reflector's dedup leg needs
        // (memupalace's memory_embed tool) — avoids refactoring AgentLoop.capability_registry's
        // field type just to share it across a separately-spawned task.
        let mut reflector_registry = bastion_runtime::capability::CapabilityRegistry::new();
        if let Err(e) = reflector_registry.register(Arc::new(
            bastion_mcp::adapters::McpToolAdapter {
                tool_name: "memory_embed".to_string(),
                server_label: "memupalace".to_string(),
                description: "Return the embedding vector for a text (dedup similarity)"
                    .to_string(),
                schema: serde_json::json!({"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}),
                mcp: mcp_for_product.clone(),
                // memupalace's memory_embed is NOT local (Plan 10-08) — preserves
                // today's exact behavior unchanged.
                is_local_override: false,
                // memory_embed is a read-only embedding lookup, not destructive —
                // and this minimal reflector registry has no real ApprovalGate
                // wired anyway (bare `CapabilityRegistry::new()` defaults to the
                // fail-closed `NullApprovalGate`), so needs_approval:true here
                // would fail-closed-deny it outright (Plan 11-04).
                needs_approval_override: false,
                trusted_override: false,
            },
        )) {
            tracing::warn!(event = "reflector_registry_register_failed", error = %e);
        }

        // LEARN-05 gap fix: an explicit [reflector].model must actually select the
        // Reflector's provider, not just be threaded through inertly. Unset/empty falls
        // back to the exact same default-agent provider instance (safe pre-fix behavior).
        let reflector_provider = bastion_providers::registry::resolve_reflector_provider(
            cfg.reflector.model.as_deref(),
            &cfg.agent.default_model,
            agent.provider.clone(),
        )?;

        let generator: Arc<dyn bastion_cognition::learn::CandidateGenerator> =
            Arc::new(bastion_cognition::learn::LlmCandidateGenerator::new(
                reflector_provider,
                cfg.reflector.model.clone(),
                cfg.reflector.allow_cloud,
            ));

        let reflector = bastion_cognition::learn::Reflector::new(
            agent.memory.clone(),
            generator,
            Arc::new(reflector_registry),
            cfg.reflector.clone(),
            cfg.session.db_path.clone(),
            cfg.logging.log_path.clone(),
        );
        let owner = bastion_runtime::agent::loop_::DEFAULT_OWNER.to_string();
        let interval_hours = cfg.reflector.interval_hours;
        tokio::spawn(async move {
            reflector.run(&owner).await;
        });
        tracing::info!(event = "reflector_scheduled", interval_hours);
    }

    // CONC-1: session mutex per owner — serializes turns from the same owner so a
    // double-tap (two Telegram messages in quick succession) never starts a concurrent
    // turn for that owner. Different owners are NOT blocked by each other.
    // Arc<Mutex<()>> is cheap: lock body is just the run_turn_for call.
    // HashMap grows per unique owner but never shrinks — acceptable for personal use
    // with a small fixed set of owners (T-05-02-03 accepted risk).
    let mut session_locks: HashMap<String, Arc<Mutex<()>>> = HashMap::new();

    // M2 step 3b (D3): the P6 `CommandHandler` implementation, built AFTER the
    // webhook-gated block above (the only place that ever populates
    // `command_resources.otc_store`/`.composio_oauth`) so it closes over the
    // fully-populated resources — the same values the per-call
    // `&command_resources` argument used to carry into `agent.handle_command`.
    let command_handler =
        bastion::agent::command::CockpitCommandHandler::new(command_resources.clone());

    let mut stdin = BufReader::new(tokio::io::stdin()).lines();
    // In a detached container (`docker compose up -d`) stdin is closed and returns EOF
    // immediately. The daemon must keep running to serve channels (Telegram), so we track
    // whether stdin is still live and disable that select arm on EOF instead of exiting.
    let mut stdin_open = true;
    let mut sigterm = signal(SignalKind::terminate())?;

    // Loop 3-D: every configured channel above has finished its spawn
    // attempt (success or logged failure) — `/readyz` can now report ready.
    readiness.mark_channels_ready();

    println!("Bastion daemon started. Type a message or /help for commands.");

    loop {
        tokio::select! {
            line = stdin.next_line(), if stdin_open => {
                match line? {
                    None => {
                        tracing::info!(event = "stdin_eof");
                        // Non-interactive / detached: stop polling the (now-dead) stdin arm but
                        // keep serving channels, proactive nudges, and signals. The daemon exits
                        // only on SIGTERM/Ctrl-C — NOT on stdin EOF (D-01 long-running invariant).
                        stdin_open = false;
                    }
                    Some(s) if s.trim().is_empty() => continue,
                    Some(s) if s.trim().starts_with('/') => {
                        match agent
                            .handle_command(
                                s.trim(),
                                bastion_runtime::agent::loop_::DEFAULT_OWNER,
                                &command_handler,
                            )
                            .await?
                        {
                            CommandResult::Stop => break,
                            CommandResult::Handled(msg) => println!("{msg}"),
                            CommandResult::Unknown(cmd) => {
                                println!("Unknown command: {}. Type /help.", cmd);
                            }
                        }
                    }
                    Some(s) => {
                        match agent.run_turn(&s).await {
                            Ok(response) => {
                                println!("{}", response);
                            }
                            Err(e) => {
                                eprintln!("Error: {}", e);
                                tracing::error!(event = "turn_error", error = %e);
                            }
                        }
                    }
                }
            }
            // PROACT-05: proactive messages delivered ONLY between turns.
            // 6d (docs/revamp/C3-runtime-followups-design.md): the queued item
            // carries its own owner now — route the turn to THAT owner instead
            // of always assuming DEFAULT_OWNER (a delegated task's or a
            // goal-drift nudge's owner may be any owner, not just the local
            // one). `None` (no producer left in this codebase sends that, but
            // the type keeps the fallback honest) still degrades to the exact
            // pre-6d behavior.
            Some(item) = pending_rx.recv() => {
                let owner = item
                    .owner
                    .as_deref()
                    .unwrap_or(bastion_runtime::agent::loop_::DEFAULT_OWNER);
                tracing::info!(event = "proactive_turn", msg_len = item.text.len(), owner = %owner);
                match agent.run_turn_for(&item.text, owner).await {
                    Ok(r) => println!("{r}"),
                    Err(e) => tracing::error!(event = "proactive_turn_error", owner = %owner, error = %e),
                }
            }
            // CR-07: channel inbound arm — serializes Telegram/webhook turns through the SAME
            // agent as stdin/proactive. The trusted owner was resolved by the channel layer.
            // Typed Result propagated back through the oneshot (WR-10).
            //
            // WEB-CMD-01: slash commands reuse the same router the stdin console uses
            // (agent.handle_command), but ALLOWLISTED, not blocklisted — commands are
            // console-only by default. `provider` and `forced_persona` are single fields
            // shared by the whole daemon (not per-owner), so /as lets one remote
            // owner affect every other owner's turns; /logs exposes daemon-wide (not
            // owner-scoped) WARN/ERROR entries; /connect-app mints a JWT whose `sub` is the
            // caller-chosen device name verbatim — remotely reachable, that's an
            // authentication bypass (mint a code naming ANY owner, then impersonate them).
            // `/connect` is instructional only. `/models` and `/model` deliberately let an
            // authenticated cockpit choose the daemon-wide provider: that is the purpose of
            // the local TUI's picker, and the selection is persisted beside its local session
            // database. Keep every other stateful command console-only unless its authority
            // and scope are reviewed explicitly.
            Some(req) = inbound_rx.recv() => {
                const REMOTE_ALLOWED_COMMANDS: &[&str] =
                    &["/help", "/contest", "/connect", "/models", "/model"];
                // CONC-1: acquire per-owner lock before processing turn.
                // Two turns from the same owner cannot run concurrently (double-tap protection).
                // Different owners are independent — their locks do not contend.
                let lock = session_locks
                    .entry(req.owner.clone())
                    .or_insert_with(|| Arc::new(Mutex::new(())))
                    .clone();
                let _guard = lock.lock().await;
                let trimmed = req.text.trim();
                let command_token = trimmed.split_whitespace().next().filter(|s| s.starts_with('/'));
                // A token can look like a command but not be one (e.g. a Claude-Code-style
                // `/usage` typed out of habit, or a plain typo) — only KNOWN_COMMANDS get the
                // "console-only" verdict; anything else falls through to handle_command's own
                // Unknown-command message, exactly matching what the console would say.
                let is_known_command = command_token
                    .is_some_and(|c| bastion::agent::command::KNOWN_COMMANDS.contains(&c));
                let res = if let Some(cmd) =
                    command_token.filter(|c| is_known_command && !REMOTE_ALLOWED_COMMANDS.contains(c))
                {
                    Ok(format!("{cmd} is console-only — not allowed remotely."))
                } else if command_token.is_some() {
                    match agent
                        .handle_command(trimmed, &req.owner, &command_handler)
                        .await
                    {
                        Ok(CommandResult::Handled(msg)) => Ok(msg),
                        Ok(CommandResult::Unknown(cmd)) => {
                            Ok(format!("Unknown command: {cmd}. Type /help."))
                        }
                        Ok(CommandResult::Stop) => {
                            Ok("/stop is console-only — not allowed remotely.".to_string())
                        }
                        Err(e) => Err(e),
                    }
                } else {
                    // SEC-05: threads the channel-resolved trust classification
                    // (email always untrusted; public-channel Discord/Slack
                    // untrusted; DMs and every other pre-existing channel
                    // trusted) into the quarantine-aware turn entry point.
                    agent
                        .run_turn_for_with_trust(&req.text, &req.owner, req.untrusted)
                        .await
                };
                if let Err(ref e) = res {
                    tracing::warn!(
                        event = "channel_turn_error",
                        owner = %req.owner,
                        error = %e,
                        "channel turn failed"
                    );
                }
                let _ = req.reply.send(res);
            }
            _ = sigterm.recv() => {
                tracing::info!(event = "sigterm_received");
                println!("Shutting down (SIGTERM).");
                break;
            }
            _ = tokio::signal::ctrl_c() => {
                tracing::info!(event = "ctrl_c_received");
                println!("\nShutting down (Ctrl-C).");
                break;
            }
            // Loop 3-D (`docs/revamp/C3-cloud-ready-design.md`): the SAME
            // graceful-shutdown path as SIGTERM/Ctrl-C, triggered instead by
            // an authenticated `POST /lifecycle/stop`.
            _ = lifecycle.shutdown.notified() => {
                tracing::info!(event = "http_lifecycle_stop_received");
                println!("Shutting down (HTTP /lifecycle/stop).");
                break;
            }
            // `POST /lifecycle/reload` reloads the persona registry from
            // disk into `command_resources` (the copy `/as`/`/cabinet`
            // slash-command validation reads). Deliberately honest about
            // scope: this does NOT hot-swap the turn-dispatch
            // `PersonaResponder`'s own registry — that would need a
            // Responder-level hot-reload port, out of scope for this loop
            // (`PersonaResponder` holds its `PersonaRegistry` by value, not
            // behind a swappable handle; changing that shape is a kernel
            // contract change this cycle does not make).
            _ = lifecycle.reload.notified() => {
                match bastion_personas::persona::PersonaRegistry::load_dir(".").await {
                    Ok(fresh) => {
                        command_resources.registry = fresh;
                        tracing::info!(
                            event = "http_lifecycle_reload_applied",
                            scope = "command_resources.registry",
                            "persona registry reloaded from disk for /as and /cabinet validation \
                             — the turn-dispatch responder's own registry is NOT hot-swapped by this call",
                        );
                    }
                    Err(e) => {
                        tracing::error!(event = "http_lifecycle_reload_failed", error = %e);
                    }
                }
            }
        }
    }
    Ok(())
}

/// Build token permissions map from config (shared between daemon and mcp-stdio paths).
#[cfg(feature = "mcp-server")]
fn build_token_perms(
    cfg: &bastion::config::BastionConfig,
) -> HashMap<String, bastion::mcp::server::TokenPermissions> {
    cfg.mcp_server
        .tokens
        .iter()
        .map(|(token, t)| {
            (
                token.clone(),
                bastion::mcp::server::TokenPermissions {
                    read_only: t.read_only,
                    owner_id: t.owner_id.clone(),
                    privacy_tier: if t.cloud_ok {
                        bastion_memory::PrivacyTier::CloudOk
                    } else {
                        bastion_memory::PrivacyTier::LocalOnly
                    },
                },
            )
        })
        .collect()
}

#[cfg(test)]
mod cli_tests {
    use super::*;

    #[test]
    fn bare_bastion_selects_default_tui_flow() {
        let cli = Cli::try_parse_from(["bastion"]).unwrap();
        assert!(cli.command.is_none());
        assert!(matches!(default_chat_command(), Command::Chat { .. }));
    }

    #[test]
    fn explicit_subcommands_remain_available() {
        let cli = Cli::try_parse_from(["bastion", "daemon"]).unwrap();
        assert!(matches!(cli.command, Some(Command::Daemon)));

        let cli = Cli::try_parse_from([
            "bastion",
            "chat",
            "--url",
            "https://example.test",
            "--no-auto-start",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Chat {
                no_auto_start: true,
                ..
            })
        ));

        let cli = Cli::try_parse_from([
            "bastion",
            "companion",
            "event",
            "activity",
            "--source",
            "codex",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Companion {
                action: CompanionAction::Event { ref kind, ref source }
            }) if kind == "activity" && source == "codex"
        ));
    }
}
