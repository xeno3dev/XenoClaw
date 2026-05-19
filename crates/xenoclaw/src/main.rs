//! XenoClaw — AI agent runtime binary entry point.
//!
//! Provides the `xenoclaw` CLI with three subcommands:
//! - `serve` (default): Start the agent runtime
//! - `setup`: Run the first-run configuration wizard
//! - `reset-key`: Generate a new admin API key

mod mcp_client;
mod mcp_server;
mod setup;
mod tools;
mod workspace;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tokio::sync::RwLock;
use tracing::{error, info};

use agent_core::{AgentCore, AgentCoreConfig, AgentStatus, EventBus, ToolRegistry};
use api_server::{build_router, AppState};
use common::config::{load_config, signal::spawn_reload_handler, ConfigError};
use llm_router::LlmRouter;
use plugin_system::PluginManager;
use process_supervisor::{
    supervisor::{AgentHealthCheckFn, AgentStartFn, StateRestoreFn},
    ProcessSupervisor, SupervisorConfig,
};
use security_layer::auth::ApiKeyAuthenticator;
use security_layer::rate_limit::RateLimitConfig;
use task_scheduler::{Scheduler, SchedulerConfig};

#[derive(Parser)]
#[command(name = "xenoclaw", about = "XenoClaw AI agent runtime")]
struct Cli {
    /// Path to config.toml (default: ~/.xenoclaw/config.toml)
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Run the first-run setup wizard
    #[arg(short = 's', long = "setup")]
    setup: bool,

    /// Generate and display a new admin API key
    #[arg(long = "reset-key")]
    reset_key: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the agent runtime (default)
    Serve,
    /// Run the first-run setup wizard
    Setup,
    /// Generate and display a new admin API key
    ResetKey,
    /// Run as an MCP server over stdio (for Claude Code / Copilot integration)
    Mcp,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let config_path = cli.config.unwrap_or_else(default_config_path);

    // Flags take priority over subcommands
    if cli.setup {
        setup::run_wizard(&config_path).await?;
        return Ok(());
    }
    if cli.reset_key {
        return reset_key();
    }

    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => serve(config_path).await,
        Command::Setup => {
            setup::run_wizard(&config_path).await?;
            Ok(())
        }
        Command::ResetKey => reset_key(),
        Command::Mcp => run_mcp_server(config_path).await,
    }
}

/// Returns ~/.xenoclaw/config.toml
fn default_config_path() -> PathBuf {
    xenoclaw_home().join("config.toml")
}

/// Returns ~/.xenoclaw
fn xenoclaw_home() -> PathBuf {
    dirs_or_home().join(".xenoclaw")
}

/// Returns the user's home directory, falling back to /tmp if unavailable.
fn dirs_or_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

/// Start the agent runtime.
async fn serve(config_path: PathBuf) -> Result<()> {
    // Load configuration
    let config = match load_config(&config_path) {
        Ok(cfg) => cfg,
        Err(ConfigError::FileNotFound(_)) => {
            eprintln!(
                "Configuration file not found: {}\n\n\
                 Run `xenoclaw setup` to create one interactively,\n\
                 or copy config.example.toml to config.toml and fill in your values.",
                config_path.display()
            );
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Configuration error:\n{e}");
            std::process::exit(1);
        }
    };

    // Initialize tracing
    let log_level = config.monitoring.log_level.to_string();
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&log_level));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .init();

    info!("XenoClaw agent runtime starting");
    info!(config_path = %config_path.display(), "Configuration loaded");

    // Workspace directory lives under ~/.xenoclaw/workspace
    let workspace_dir = xenoclaw_home().join("workspace");

    // Build the system prompt
    let system_prompt = workspace::build_system_prompt(&workspace_dir)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "Failed to build system prompt, using empty");
            String::new()
        });

    // Construct the component graph
    let llm_router = LlmRouter::from_config(&config.llm);

    // Task scheduler
    let scheduler_config = SchedulerConfig::default();
    let scheduler = Arc::new(Scheduler::new(scheduler_config));

    // Initialize the database for memory tools
    let data_dir = xenoclaw_home().join("data");
    let db_pool = match std::fs::create_dir_all(&data_dir) {
        Ok(()) => {
            let db_path = data_dir.join("xenoclaw.db");
            match memory_store::init_database(&db_path).await {
                Ok(pool) => {
                    info!(path = %db_path.display(), "Database initialized");
                    Some(pool)
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Database initialization failed — memory tools will be disabled");
                    None
                }
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, path = %data_dir.display(), "Failed to create data directory — memory tools will be disabled");
            None
        }
    };

    // Register built-in tools
    let mut tool_registry = tools::register_builtin_tools(tools::BuiltinToolsConfig {
        coding: config.coding.clone(),
        filesystem_rules: config.security.filesystem_rules.clone(),
        db_pool,
        scheduler: Arc::clone(&scheduler),
    });

    // Connect to external MCP servers and register their proxy tools
    let mut mcp_manager = mcp_client::connect_mcp_servers(
        &config.mcp.servers,
        &mut tool_registry,
    )
    .await;

    let mut agent_config = AgentCoreConfig::default();
    agent_config.system_prompt = Some(system_prompt);

    let agent_core = Arc::new(AgentCore::new(llm_router, tool_registry, agent_config));

    // API state
    let api_keys = Vec::new(); // Loaded from config/DB in production
    let rate_limit_config = RateLimitConfig {
        default_limit: config.api.rate_limit_per_minute,
        ..RateLimitConfig::default()
    };
    let state = AppState::with_admin_credentials(
        api_keys,
        rate_limit_config,
        config.security.admin_username.clone(),
        config.security.admin_password_hash.clone(),
    );
    let router = build_router(state);

    // Plugin system
    let plugin_registry = Arc::new(RwLock::new(ToolRegistry::new()));
    let event_bus = EventBus::new(256);
    let plugin_limits = config.security.resource_limits.clone();
    let mut plugin_manager =
        PluginManager::new(config.plugins.clone(), plugin_registry, event_bus, plugin_limits);

    // Process supervisor
    let supervisor = Arc::new(ProcessSupervisor::new(SupervisorConfig::default()));

    // Wire supervisor callbacks
    let agent_for_start = Arc::clone(&agent_core);
    let start_fn: AgentStartFn = Arc::new(move || {
        let agent = Arc::clone(&agent_for_start);
        Box::pin(async move {
            agent.start().await;
            Ok(())
        })
    });
    supervisor.set_agent_start_fn(start_fn).await;

    let agent_for_health = Arc::clone(&agent_core);
    let health_fn: AgentHealthCheckFn = Arc::new(move || {
        let agent = Arc::clone(&agent_for_health);
        Box::pin(async move {
            let status = agent.status().await;
            matches!(status, AgentStatus::Idle | AgentStatus::Working { .. })
        })
    });
    supervisor.set_health_check_fn(health_fn).await;

    let restore_fn: StateRestoreFn = Arc::new(|| {
        Box::pin(async {
            tracing::debug!("TODO: implement state restoration");
            Ok(())
        })
    });
    supervisor.set_state_restore_fn(restore_fn).await;

    // Spawn subsystems
    let bind_addr = format!("{}:{}", config.api.host, config.api.port);
    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .context(format!("Failed to bind to {bind_addr}"))?;
    info!(address = %bind_addr, "API server listening");

    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, router).await {
            error!(error = %e, "API server error");
        }
    });

    tokio::spawn(async move {
        let results = plugin_manager.initialize().await;
        for r in &results {
            if r.success {
                info!(plugin = %r.name, "Plugin loaded");
            } else {
                tracing::warn!(plugin = %r.name, error = ?r.error, "Plugin failed to load");
            }
        }
    });

    // Start supervisor (which starts the agent)
    supervisor.start().await.context("Failed to start process supervisor")?;

    // Spawn SIGHUP reload handler
    let shared_config = Arc::new(RwLock::new(config));
    spawn_reload_handler(Arc::clone(&shared_config), config_path);

    info!("XenoClaw agent runtime ready");

    // Wait for shutdown signal
    shutdown_signal().await;

    info!("Shutdown signal received, stopping...");

    // Graceful shutdown
    mcp_manager.shutdown().await;

    if let Err(e) = agent_core.shutdown().await {
        tracing::warn!(error = %e, "Agent shutdown error");
    }
    if let Err(e) = supervisor.stop().await {
        tracing::warn!(error = %e, "Supervisor stop error");
    }

    info!("XenoClaw agent runtime stopped");
    Ok(())
}

/// Run the MCP server over stdio.
///
/// This mode starts a lightweight MCP protocol handler without the full runtime
/// (no API server, no supervisor, no TUI). It's designed for integration with
/// Claude Code, Copilot, and other MCP-compatible clients.
async fn run_mcp_server(config_path: PathBuf) -> Result<()> {
    // Load configuration
    let config = match load_config(&config_path) {
        Ok(cfg) => cfg,
        Err(ConfigError::FileNotFound(_)) => {
            eprintln!(
                "Configuration file not found: {}\n\n\
                 Run `xenoclaw setup` to create one interactively,\n\
                 or copy config.example.toml to config.toml and fill in your values.",
                config_path.display()
            );
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Configuration error:\n{e}");
            std::process::exit(1);
        }
    };

    // Initialize tracing to stderr (stdout is the MCP transport)
    let log_level = config.monitoring.log_level.to_string();
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&log_level));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .with_writer(std::io::stderr)
        .init();

    info!("XenoClaw MCP server starting");

    // Initialize the database for memory tools
    let data_dir = xenoclaw_home().join("data");
    let db_pool = match std::fs::create_dir_all(&data_dir) {
        Ok(()) => {
            let db_path = data_dir.join("xenoclaw.db");
            match memory_store::init_database(&db_path).await {
                Ok(pool) => {
                    info!(path = %db_path.display(), "Database initialized");
                    Some(pool)
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Database initialization failed — memory tools will be disabled");
                    None
                }
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, path = %data_dir.display(), "Failed to create data directory — memory tools will be disabled");
            None
        }
    };

    // Task scheduler (needed for task tools)
    let scheduler_config = SchedulerConfig::default();
    let scheduler = Arc::new(Scheduler::new(scheduler_config));

    // Register built-in tools
    let tool_registry = tools::register_builtin_tools(tools::BuiltinToolsConfig {
        coding: config.coding.clone(),
        filesystem_rules: config.security.filesystem_rules.clone(),
        db_pool,
        scheduler,
    });

    let registry = Arc::new(RwLock::new(tool_registry));

    // Create and run the MCP server
    let mcp_server = mcp_server::McpServer::new(registry);
    mcp_server.run_stdio().await.context("MCP server error")?;

    info!("XenoClaw MCP server stopped");
    Ok(())
}

/// Wait for SIGINT or SIGTERM.
async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();

    #[cfg(unix)]
    {
        let mut sigterm =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("failed to register SIGTERM handler");
        tokio::select! {
            _ = ctrl_c => {},
            _ = sigterm.recv() => {},
        }
    }

    #[cfg(not(unix))]
    {
        ctrl_c.await.ok();
    }
}

/// Generate and display a new admin API key.
fn reset_key() -> Result<()> {
    let auth = ApiKeyAuthenticator::new();
    let raw_key = auth.generate_key(64);
    let key_hash = auth.hash_key(&raw_key);

    println!("New admin API key (save this — it will NOT be shown again):\n");
    println!("  {raw_key}\n");
    println!("SHA-256 hash (for config.toml [security] section):\n");
    println!("  {key_hash}\n");
    println!("Update your config.toml with this hash to enable the new key.");

    Ok(())
}
