//! XenoClaw — AI agent runtime binary entry point.
//!
//! Provides the `xenoclaw` CLI with three subcommands:
//! - `serve` (default): Start the agent runtime
//! - `setup`: Run the first-run configuration wizard
//! - `reset-key`: Generate a new admin API key

mod admin;
mod cli_health;
mod mcp_client;
mod mcp_server;
mod setup;
mod tools;
mod workspace;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tokio::sync::RwLock;
use tracing::{error, info};
use uuid::Uuid;

use tower_http::services::{ServeDir, ServeFile};

use agent_core::{AgentCore, AgentCoreConfig, AgentStatus, EventBus, PostTaskHookFn, ToolRegistry};
use api_server::state::{LogLevelSetter, MessagingStatus, ResourceMetrics};
use api_server::{build_router, AppState};
use chrono::Utc;
use clap::Args;
use common::config::{load_config, signal::spawn_reload_handler, ConfigError, SkillsConfig};
use common::{models::ApiKey, types::ApiKeyId};
use llm_router::{
    types::{ChatMessage, ChatRole, CompletionRequest},
    LlmRouter,
};
use plugin_system::PluginManager;
use process_supervisor::{
    supervisor::{AgentHealthCheckFn, AgentStartFn, StateRestoreFn},
    ProcessSupervisor, SupervisorConfig,
};
use security_layer::auth::ApiKeyAuthenticator;
use security_layer::rate_limit::RateLimitConfig;
use skills::{SkillCurator, SkillDoc, SkillLoader, SkillStore};
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
    /// Generate and display a new admin API key (does not modify config)
    ResetKey,
    /// Set the admin password directly in the config (bypasses the TUI wizard)
    Passwd {
        /// Username to set alongside the new password (optional).
        #[arg(long)]
        user: Option<String>,
        /// Password to set. If omitted, prompts twice on the TTY.
        #[arg(long)]
        password: Option<String>,
    },
    /// Rotate the admin API key in the config and print the new raw key.
    SetApiKey,
    /// Show the admin identity the running service would use.
    ShowAdmin,
    /// Locally verify whether a username/password would pass the API's bcrypt check.
    VerifyLogin {
        #[arg(long)]
        user: String,
        #[arg(long)]
        password: String,
    },
    /// Run as an MCP server over stdio (for Claude Code / Copilot integration)
    Mcp,
    /// Open an interactive chat session with a running XenoClaw agent.
    Tui {
        /// Inline mode — chat stays in your terminal scrollback (no alt-screen).
        #[arg(long)]
        inline: bool,

        /// API endpoint. Defaults to http://<api.host>:<api.port> from config.
        #[arg(long)]
        endpoint: Option<String>,

        /// API key. Defaults to $XENOCLAW_API_KEY, then prompts if missing.
        #[arg(long)]
        api_key: Option<String>,

        /// Session ID. Defaults to a new random UUID.
        #[arg(long)]
        session: Option<String>,
    },
    /// Manage the XenoClaw skill library.
    Skills(SkillsArgs),
}

/// Arguments for the `skills` subcommand.
#[derive(Args)]
pub struct SkillsArgs {
    #[command(subcommand)]
    pub command: SkillsCommand,
}

#[derive(Subcommand)]
pub enum SkillsCommand {
    /// List all skills with name, description, and use count.
    List,
    /// Show the full content of a skill.
    Show { name: String },
    /// Archive (soft-delete) a skill.
    Remove { name: String },
    /// Run the curator pass immediately (requires LLM to be configured).
    Curate,
    /// Show curator status and usage statistics.
    Status,
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
        Command::Passwd { user, password } => {
            admin::set_password(&config_path, user.as_deref(), password.as_deref())
        }
        Command::SetApiKey => admin::set_api_key(&config_path),
        Command::ShowAdmin => admin::show_admin(&config_path),
        Command::VerifyLogin { user, password } => {
            admin::verify_login(&config_path, &user, &password)
        }
        Command::Mcp => run_mcp_server(config_path).await,
        Command::Tui {
            inline,
            endpoint,
            api_key,
            session,
        } => run_tui(config_path, inline, endpoint, api_key, session).await,
        Command::Skills(args) => run_skills_command(args.command, config_path).await,
    }
}

/// Returns the config path to use when `--config` is not given.
///
/// Preference order:
///   1. $XENOCLAW_CONFIG_PATH (set by the systemd service unit).
///   2. /etc/xenoclaw/config.toml when it exists — system install path that
///      install.sh writes to. Without this, `xenoclaw -s` would default to
///      ~/.xenoclaw/config.toml and silently diverge from the file the
///      systemd service actually reads, leaving login broken.
///   3. ~/.xenoclaw/config.toml — local dev / unprivileged use.
fn default_config_path() -> PathBuf {
    if let Some(p) = std::env::var_os("XENOCLAW_CONFIG_PATH") {
        return PathBuf::from(p);
    }
    let system = PathBuf::from("/etc/xenoclaw/config.toml");
    if system.exists() {
        return system;
    }
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

/// Returns the data directory: $XENOCLAW_DATA_DIR when set (systemd service),
/// otherwise ~/.xenoclaw/data for local dev.
fn data_dir() -> PathBuf {
    std::env::var_os("XENOCLAW_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| xenoclaw_home().join("data"))
}

/// Resolve a relative plugins directory to an absolute path.
///
/// Preference order:
///   1. Already absolute — use as-is.
///   2. $XENOCLAW_DATA_DIR is set (systemd service) → data_dir/plugins.
///   3. Fall back to ~/.xenoclaw/plugins.
///
/// This runs at startup after config load, so it fixes both serde defaults
/// and any explicit `./plugins` written into config.toml.
fn resolve_plugins_dir(dir: &Path) -> PathBuf {
    if dir.is_absolute() {
        return dir.to_path_buf();
    }
    std::env::var_os("XENOCLAW_DATA_DIR")
        .map(|d| PathBuf::from(d).join("plugins"))
        .unwrap_or_else(|| data_dir().join("plugins"))
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

    // Initialize tracing with a reload-capable EnvFilter so PUT /api/v1/config
    // can change the log level at runtime without a restart.
    let log_level = config.monitoring.log_level.to_string();
    let initial_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&log_level));
    let (filter_layer, reload_handle) = tracing_subscriber::reload::Layer::new(initial_filter);
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    let fmt_layer = tracing_subscriber::fmt::layer().with_target(true);
    tracing_subscriber::registry()
        .with(filter_layer)
        .with(fmt_layer)
        .init();
    // Type-erased setter — AppState stores this closure, the config endpoint calls it.
    let log_level_setter = LogLevelSetter::new(move |s: &str| -> Result<(), String> {
        let new_filter = tracing_subscriber::EnvFilter::try_new(s)
            .map_err(|e| format!("invalid filter '{s}': {e}"))?;
        reload_handle
            .reload(new_filter)
            .map_err(|e| format!("reload failed: {e}"))
    });

    info!("XenoClaw agent runtime starting");
    info!(config_path = %config_path.display(), "Configuration loaded");

    // Workspace directory: use configured path or fall back to ~/.xenoclaw/workspace
    let workspace_dir = config
        .coding
        .workspace_dirs
        .first()
        .cloned()
        .unwrap_or_else(|| xenoclaw_home().join("workspace"));

    // Skills infrastructure — initialise store and loader.
    let skills_dir = resolve_skills_dir(&config.skills);
    let skill_store = Arc::new(SkillStore::new(skills_dir.clone()));
    if let Err(e) = skill_store.ensure_dir().await {
        tracing::warn!(error = %e, "Failed to create skills directory");
    }
    let skill_loader = Arc::new(SkillLoader::new(Arc::clone(&skill_store)));

    // Level 0 index for system prompt injection.
    let skill_index = skill_loader.level0_index().await;

    // Build the system prompt
    let system_prompt = workspace::build_system_prompt(
        &workspace_dir,
        if skill_index.is_empty() {
            None
        } else {
            Some(&skill_index)
        },
    )
    .await
    .unwrap_or_else(|e| {
        tracing::warn!(error = %e, "Failed to build system prompt, using empty");
        String::new()
    });

    // Construct the component graph
    let llm_router = LlmRouter::from_config(&config.llm);
    // Capture vision capability before the router is moved into AgentCore —
    // the view_image tool needs it for its fail-safe.
    let vision_supported = llm_router.supports_vision();

    // Task scheduler
    let scheduler_config = SchedulerConfig::default();
    let scheduler = Arc::new(Scheduler::new(scheduler_config));

    // Initialize the database for memory tools
    let data_dir = data_dir();
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

    // Register built-in tools — clone the pool so we can also pass the
    // original into AppState (used for plugin_states persistence).
    let mut tool_registry = tools::register_builtin_tools(tools::BuiltinToolsConfig {
        coding: config.coding.clone(),
        filesystem_rules: config.security.filesystem_rules.clone(),
        db_pool: db_pool.clone(),
        scheduler: Arc::clone(&scheduler),
        workspace_dir: workspace_dir.clone(),
        vision_supported,
    });

    // Register the use_skill tool.
    tool_registry.register(Arc::new(tools::UseSkillTool::new(Arc::clone(
        &skill_loader,
    ))));

    // Connect to external MCP servers and register their proxy tools
    let mut mcp_manager =
        mcp_client::connect_mcp_servers(&config.mcp.servers, &mut tool_registry).await;

    let agent_config = AgentCoreConfig {
        system_prompt: Some(system_prompt),
        ..AgentCoreConfig::default()
    };

    // Build the post-task reflection hook (fire-and-forget after complex tasks).
    let logs_dir = xenoclaw_home().join("logs");
    let mut agent_core = AgentCore::new(llm_router, tool_registry, agent_config);

    if config.skills.auto_create {
        let llm_arc = Arc::clone(agent_core.llm_router());
        let live_prompt_slot = agent_core.live_system_prompt();
        let hook = make_skill_hook(
            Arc::clone(&skill_store),
            llm_arc,
            config.skills.auto_create_threshold,
            logs_dir.clone(),
            live_prompt_slot,
            workspace_dir.clone(),
        );
        agent_core = agent_core.with_post_task_hook(hook);
        info!(
            threshold = config.skills.auto_create_threshold,
            "Skill auto-creation hook registered"
        );
    }

    let agent_core = Arc::new(agent_core);

    // API state — bootstrap the admin API key from config if one was generated.
    let api_keys = if !config.security.admin_key_hash.is_empty() {
        vec![ApiKey {
            id: ApiKeyId::new(),
            key_hash: config.security.admin_key_hash.clone(),
            name: "admin".to_string(),
            rate_limit: config.api.rate_limit_per_minute,
            created_at: Utc::now(),
            last_used: None,
        }]
    } else {
        Vec::new()
    };

    // Loud warning when no login method is configured — every web/API login
    // attempt will return 401 ("Invalid …") until creds are set. This is the
    // failure mode you'd hit if install.sh dropped the example config but the
    // wizard wrote creds to a different file (e.g. ~/.xenoclaw/config.toml).
    let pw_set = !config.security.admin_password_hash.is_empty();
    let key_set = !config.security.admin_key_hash.is_empty();
    if !pw_set && !key_set {
        tracing::warn!(
            config_path = %config_path.display(),
            "No admin credentials in config: admin_password_hash and admin_key_hash are both empty. \
             Web UI and API logins will be rejected. Run `xenoclaw -s --config {}` to configure.",
            config_path.display()
        );
    } else {
        info!(
            password_login = pw_set,
            api_key_login = key_set,
            admin_username = %config.security.admin_username,
            "Admin credentials loaded"
        );
    }
    let rate_limit_config = RateLimitConfig {
        default_limit: config.api.rate_limit_per_minute,
        ..RateLimitConfig::default()
    };
    // Build the plugin manager BEFORE AppState so we can pass it in. We hold
    // it in an Arc<RwLock<>> because `initialize()`/`shutdown()` need &mut self
    // but the per-request handlers only need &self (via .read()).
    let plugin_registry = Arc::new(RwLock::new(ToolRegistry::new()));
    let event_bus = EventBus::new(256);
    let plugin_limits = config.security.resource_limits.clone();
    let mut plugins_config = config.plugins.clone();
    plugins_config.directory = resolve_plugins_dir(&plugins_config.directory);
    let plugin_manager =
        PluginManager::new(plugins_config, plugin_registry, event_bus, plugin_limits);
    let plugin_manager = Arc::new(RwLock::new(plugin_manager));

    // Resource metrics cell — shared between the sysinfo sampler task and the
    // status route handler.
    let metrics_cell: Arc<RwLock<ResourceMetrics>> =
        Arc::new(RwLock::new(ResourceMetrics::default()));

    let mut state = AppState::with_admin_credentials(
        api_keys,
        rate_limit_config,
        config.security.admin_username.clone(),
        config.security.admin_password_hash.clone(),
    )
    .with_messaging_status(MessagingStatus {
        telegram_configured: config.messaging.telegram.is_some(),
        discord_configured: config.messaging.discord.is_some(),
        whatsapp_configured: config.messaging.whatsapp.is_some(),
    })
    .with_agent_core(Arc::clone(&agent_core))
    .with_workspace_dir(workspace_dir.clone())
    .with_plugin_manager(Arc::clone(&plugin_manager))
    .with_metrics(Arc::clone(&metrics_cell))
    .with_log_level_setter(log_level_setter, log_level.clone())
    .with_skill_store(Arc::clone(&skill_store));

    if let Some(pool) = db_pool.clone() {
        state = state.with_db_pool(pool.clone());
        // Load any previously-toggled plugin states from SQLite so restarts
        // don't surprise the user with re-enabled plugins they disabled.
        let persisted = api_server::state::load_plugin_states(&pool).await;
        if !persisted.is_empty() {
            info!(
                count = persisted.len(),
                "Loaded persisted plugin enable/disable states"
            );
            *state.plugin_states.write().await = persisted;
        }
    }

    // Spawn the resource-metrics sampler. Updates every 2s; the status handler
    // reads from the same Arc<RwLock<ResourceMetrics>> without paying refresh cost.
    spawn_metrics_sampler(Arc::clone(&metrics_cell));

    let router = build_router(state.clone());

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

    // Serve the frontend from the API port so relative /api/v1/* URLs resolve on the same origin.
    let router = if config.web.enabled && config.web.dir.exists() {
        let web_dir = config.web.dir.clone();
        let index = web_dir.join("index.html");
        info!(dir = %web_dir.display(), "Web UI served from API port");
        router.fallback_service(ServeDir::new(&web_dir).fallback(ServeFile::new(index)))
    } else {
        if config.web.enabled {
            tracing::warn!(
                dir = %config.web.dir.display(),
                "Web UI dir not found — run 'npm run build' in web/ or set XENOCLAW_WEB_DIR"
            );
        }
        router
    };

    // Spawn subsystems
    let bind_addr = format!("{}:{}", config.api.host, config.api.port);
    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .context(format!("Failed to bind to {bind_addr}"))?;
    info!(address = %bind_addr, "API + Web UI server listening");

    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, router).await {
            error!(error = %e, "API server error");
        }
    });

    // Spawn the skill curator background task if enabled.
    if config.skills.curator_enabled {
        let curator = Arc::new(SkillCurator::new(
            Arc::clone(&skill_store),
            Arc::clone(agent_core.llm_router()),
            config.skills.curator_schedule.clone(),
            logs_dir.clone(),
        ));
        tokio::spawn(async move { curator.start_background().await });
    }

    let pm_for_init = Arc::clone(&plugin_manager);
    let initial_plugin_states = state.plugin_states.clone();
    let db_pool_for_init = state.db_pool.clone();
    tokio::spawn(async move {
        let results = {
            let mut mgr = pm_for_init.write().await;
            mgr.initialize().await
        };
        for r in &results {
            if r.success {
                info!(plugin = %r.name, "Plugin loaded");
            } else {
                tracing::warn!(plugin = %r.name, error = ?r.error, "Plugin failed to load");
            }
        }
        // Unload anything the user previously toggled off — restart should
        // honour the last enable/disable state.
        let toggles = initial_plugin_states.read().await.clone();
        let mgr = pm_for_init.read().await;
        for (name, enabled) in toggles {
            if !enabled && mgr.is_loaded(&name).await {
                if let Err(e) = mgr.unload_plugin(&name).await {
                    tracing::warn!(plugin = %name, error = %e, "Failed to re-apply disabled state on startup");
                } else {
                    // Save again to refresh updated_at
                    api_server::state::save_plugin_state(db_pool_for_init.as_ref(), &name, false)
                        .await;
                }
            }
        }
    });

    // Start supervisor (which starts the agent)
    supervisor
        .start()
        .await
        .context("Failed to start process supervisor")?;

    // Spawn SIGHUP reload handler
    let shared_config = Arc::new(RwLock::new(config));
    spawn_reload_handler(Arc::clone(&shared_config), config_path);

    info!("XenoClaw agent runtime ready");

    // Wait for shutdown signal
    shutdown_signal().await;

    info!("Shutdown signal received, stopping...");

    // Graceful shutdown
    mcp_manager.shutdown().await;
    {
        let mut mgr = plugin_manager.write().await;
        mgr.shutdown().await;
    }

    if let Err(e) = agent_core.shutdown().await {
        tracing::warn!(error = %e, "Agent shutdown error");
    }
    if let Err(e) = supervisor.stop().await {
        tracing::warn!(error = %e, "Supervisor stop error");
    }

    info!("XenoClaw agent runtime stopped");
    Ok(())
}

/// Spawn a background task that refreshes CPU + memory metrics every 2s by
/// reading /proc directly. Linux-only — the agent doesn't run on macOS/Windows.
fn spawn_metrics_sampler(cell: Arc<RwLock<ResourceMetrics>>) {
    tokio::spawn(async move {
        // Prime the CPU counters — first call returns None because we need
        // two samples to compute a delta.
        let mut last = read_cpu_totals();
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        loop {
            let now = read_cpu_totals();
            let cpu_percent = match (last, now) {
                (Some((idle_a, total_a)), Some((idle_b, total_b))) if total_b > total_a => {
                    let idle_delta = idle_b.saturating_sub(idle_a) as f64;
                    let total_delta = (total_b - total_a) as f64;
                    ((total_delta - idle_delta) / total_delta * 100.0) as f32
                }
                _ => 0.0,
            };
            last = now;

            let memory_percent = read_memory_percent().unwrap_or(0.0);

            *cell.write().await = ResourceMetrics {
                cpu_percent,
                memory_percent,
            };
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    });
}

/// Read aggregate CPU jiffies from /proc/stat — returns (idle, total) or None
/// if the file is unreadable / malformed. The "cpu " summary line lists:
///   user nice system idle iowait irq softirq steal guest guest_nice
/// We sum all fields for total; idle = idle + iowait.
fn read_cpu_totals() -> Option<(u64, u64)> {
    let contents = std::fs::read_to_string("/proc/stat").ok()?;
    let line = contents.lines().find(|l| l.starts_with("cpu "))?;
    let mut parts = line.split_whitespace();
    parts.next()?; // "cpu" tag
    let nums: Vec<u64> = parts.filter_map(|p| p.parse().ok()).collect();
    if nums.len() < 4 {
        return None;
    }
    let idle = nums[3] + nums.get(4).copied().unwrap_or(0); // idle + iowait
    let total: u64 = nums.iter().sum();
    Some((idle, total))
}

/// Read memory usage from /proc/meminfo as a percentage of MemTotal that is
/// not available to userspace. Returns None on parse failure.
fn read_memory_percent() -> Option<f32> {
    let contents = std::fs::read_to_string("/proc/meminfo").ok()?;
    let mut mem_total: Option<u64> = None;
    let mut mem_available: Option<u64> = None;
    for line in contents.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            mem_total = rest.split_whitespace().next().and_then(|n| n.parse().ok());
        } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
            mem_available = rest.split_whitespace().next().and_then(|n| n.parse().ok());
        }
        if mem_total.is_some() && mem_available.is_some() {
            break;
        }
    }
    let total = mem_total?;
    let available = mem_available?;
    if total == 0 {
        return Some(0.0);
    }
    let used = total.saturating_sub(available);
    Some((used as f64 / total as f64 * 100.0) as f32)
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
    let data_dir = data_dir();
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
        workspace_dir: config
            .coding
            .workspace_dirs
            .first()
            .cloned()
            .unwrap_or_else(|| xenoclaw_home().join("workspace")),
        // MCP clients render images themselves; the agent-side vision tool is
        // not used over the MCP transport.
        vision_supported: false,
    });

    let registry = Arc::new(RwLock::new(tool_registry));

    // Create and run the MCP server
    let mcp_server = mcp_server::McpServer::new(registry);
    mcp_server.run_stdio().await.context("MCP server error")?;

    info!("XenoClaw MCP server stopped");
    Ok(())
}

/// Run the TUI client.
async fn run_tui(
    config_path: PathBuf,
    inline: bool,
    endpoint: Option<String>,
    api_key: Option<String>,
    session: Option<String>,
) -> Result<()> {
    // Load configuration (same as serve)
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

    // Build API base URL
    let api_base_url =
        endpoint.unwrap_or_else(|| format!("http://{}:{}", config.api.host, config.api.port));

    // Resolve API key: --api-key > env var > prompt
    let api_key = match api_key {
        Some(k) => k,
        None => match std::env::var("XENOCLAW_API_KEY") {
            Ok(k) => k,
            Err(_) => {
                eprint!("Enter API key: ");
                std::io::stdout().flush()?;
                rpassword::read_password()?
            }
        },
    };

    // Resolve session ID
    let session_id = match session {
        Some(s) => Uuid::parse_str(&s).context("Invalid session UUID")?,
        None => Uuid::new_v4(),
    };

    // Build WebSocket URL
    let ws_url = {
        let base = api_base_url.trim_start_matches("http://");
        format!("ws://{}/api/v1/ws/chat?token={}", base, api_key)
    };

    // Health check before starting the UI
    {
        let client = tui::client::http::ApiClient::new(api_base_url.clone(), api_key.clone());
        if let Err(e) = client.health().await {
            eprintln!(
                "Agent not reachable at {api_base_url} — is 'xenoclaw serve' running?\nError: {e}"
            );
            std::process::exit(1);
        }
        println!("Agent reachable ✓");
    }

    // Build TUI config
    let tui_config = tui::TuiConfig {
        api_base_url,
        ws_url,
        api_key,
        session_id,
        inline,
        status_refresh_interval: std::time::Duration::from_secs(2),
        reconnect_interval: std::time::Duration::from_secs(5),
        max_reconnect_attempts: 6,
        max_history_size: 200,
    };

    // Run the TUI
    tui::run(tui_config)
        .await
        .map_err(|e| anyhow::anyhow!("TUI error: {e}"))
}

/// Wait for SIGINT or SIGTERM.
async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();

    #[cfg(unix)]
    {
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
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

// ---------------------------------------------------------------------------
// Skills CLI
// ---------------------------------------------------------------------------

/// Resolve the skills directory from config (or default to ~/.xenoclaw/skills/).
fn resolve_skills_dir(skills_config: &SkillsConfig) -> PathBuf {
    if let Some(ref dir) = skills_config.skills_dir {
        return expand_tilde(dir);
    }
    xenoclaw_home().join("skills")
}

/// Expand a leading `~/` in a path using $HOME.
fn expand_tilde(path: &Path) -> PathBuf {
    if let Some(s) = path.to_str() {
        if let Some(rest) = s.strip_prefix("~/") {
            let home = std::env::var_os("HOME").unwrap_or_else(|| "/tmp".into());
            return PathBuf::from(home).join(rest);
        }
    }
    path.to_path_buf()
}

/// Handle `xenoclaw skills <subcommand>`.
async fn run_skills_command(cmd: SkillsCommand, config_path: PathBuf) -> Result<()> {
    // Load config for skills_dir and LLM settings.
    let config = match load_config(&config_path) {
        Ok(c) => c,
        Err(ConfigError::FileNotFound(_)) => {
            // Skills commands work without a full config — use defaults.
            common::config::PlatformConfig::default()
        }
        Err(e) => {
            eprintln!("Configuration error: {e}");
            std::process::exit(1);
        }
    };

    let skills_dir = resolve_skills_dir(&config.skills);
    let store = Arc::new(SkillStore::new(skills_dir.clone()));
    store
        .ensure_dir()
        .await
        .context("Failed to create skills directory")?;

    match cmd {
        SkillsCommand::List => {
            let skills = store.list().await.context("Failed to list skills")?;
            if skills.is_empty() {
                println!(
                    "No skills installed. Skills are created automatically after complex tasks."
                );
            } else {
                println!("{:<32} {:>8}   {}", "Name", "Uses", "Description");
                println!("{}", "-".repeat(72));
                for s in &skills {
                    println!(
                        "{:<32} {:>8}   {}",
                        s.front_matter.name, s.front_matter.use_count, s.front_matter.description,
                    );
                }
                println!("\n{} skill(s) total.", skills.len());
            }
        }

        SkillsCommand::Show { name } => match store.get(&name).await? {
            Some(doc) => println!("{}", doc.render()),
            None => {
                eprintln!("Skill '{}' not found.", name);
                std::process::exit(1);
            }
        },

        SkillsCommand::Remove { name } => match store.archive(&name).await? {
            true => println!("Skill '{}' archived (moved to .archive/).", name),
            false => {
                eprintln!("Skill '{}' not found.", name);
                std::process::exit(1);
            }
        },

        SkillsCommand::Status => {
            let skills = store.list().await.context("Failed to list skills")?;
            let logs_dir = xenoclaw_home().join("logs");

            println!("Skill Library Status");
            println!("====================");
            println!("Skills directory : {}", skills_dir.display());
            println!("Total skills     : {}", skills.len());

            // Curator last run
            let curator_dir = logs_dir.join("curator");
            let last_run = {
                let mut dates: Vec<String> = Vec::new();
                if let Ok(mut entries) = tokio::fs::read_dir(&curator_dir).await {
                    while let Ok(Some(entry)) = entries.next_entry().await {
                        let name = entry.file_name().to_string_lossy().to_string();
                        if name.ends_with(".md") {
                            dates.push(name.trim_end_matches(".md").to_string());
                        }
                    }
                }
                dates.sort();
                dates.last().cloned()
            };
            match last_run {
                Some(date) => println!("Last curator run : {date}"),
                None => println!("Last curator run : No curator runs yet"),
            }

            if !skills.is_empty() {
                let mut by_use = skills.clone();
                by_use.sort_by(|a, b| b.front_matter.use_count.cmp(&a.front_matter.use_count));

                println!("\nTop 5 by use count:");
                for s in by_use.iter().take(5) {
                    println!("  {:>6}  {}", s.front_matter.use_count, s.front_matter.name);
                }

                println!("\nBottom 5 by use count:");
                for s in by_use.iter().rev().take(5) {
                    println!("  {:>6}  {}", s.front_matter.use_count, s.front_matter.name);
                }
            }
        }

        SkillsCommand::Curate => {
            // Need a real LLM config.
            if config.llm.providers.is_empty() {
                eprintln!(
                    "No LLM providers configured. Add at least one [[llm.providers]] entry in config.toml."
                );
                std::process::exit(1);
            }

            // Initialise tracing to stderr so progress is visible.
            let filter = tracing_subscriber::EnvFilter::new("info");
            let _ = tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_writer(std::io::stderr)
                .try_init();

            let llm_router = LlmRouter::from_config(&config.llm);
            let llm_arc = Arc::new(tokio::sync::RwLock::new(llm_router));
            let logs_dir = xenoclaw_home().join("logs");

            let curator = SkillCurator::new(
                Arc::clone(&store),
                llm_arc,
                config.skills.curator_schedule.clone(),
                logs_dir,
            );

            println!("Running curator pass...");
            match curator.run_once().await {
                Ok(report) => {
                    println!("{report}");
                }
                Err(e) => {
                    eprintln!("Curator run failed: {e}");
                    std::process::exit(1);
                }
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Skill reflection hook
// ---------------------------------------------------------------------------

/// Build the `PostTaskHookFn` that runs skill reflection after complex tasks.
fn make_skill_hook(
    store: Arc<SkillStore>,
    llm_router: Arc<tokio::sync::RwLock<LlmRouter>>,
    threshold: u32,
    logs_dir: PathBuf,
    live_system_prompt: Arc<RwLock<Option<String>>>,
    workspace_dir: PathBuf,
) -> PostTaskHookFn {
    Arc::new(move |tool_count: usize, transcript: Vec<ChatMessage>| {
        let store = Arc::clone(&store);
        let llm_router = Arc::clone(&llm_router);
        let logs_dir = logs_dir.clone();
        let live_system_prompt = Arc::clone(&live_system_prompt);
        let workspace_dir = workspace_dir.clone();
        Box::pin(async move {
            if tool_count >= threshold as usize {
                run_skill_reflection(
                    tool_count,
                    transcript,
                    store,
                    llm_router,
                    logs_dir,
                    live_system_prompt,
                    workspace_dir,
                )
                .await;
            }
        })
    })
}

/// Call the LLM to decide if a skill should be created/updated, then act on the response.
async fn run_skill_reflection(
    tool_count: usize,
    transcript: Vec<ChatMessage>,
    store: Arc<SkillStore>,
    llm_router: Arc<tokio::sync::RwLock<LlmRouter>>,
    _logs_dir: PathBuf,
    live_system_prompt: Arc<RwLock<Option<String>>>,
    workspace_dir: PathBuf,
) {
    let loader = SkillLoader::new(Arc::clone(&store));
    let skill_index = loader.level0_index().await;

    let transcript_text = transcript
        .iter()
        .map(|m| {
            let role = format!("{:?}", m.role).to_lowercase();
            format!("[{role}]: {}", m.content)
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    let prompt = format!(
        "You just completed a complex task ({tool_count} tool calls). \
         Review what you did and decide:\n\
         1. Is there a reusable procedure here worth saving as a skill?\n\
         2. If yes: write a SKILL.md for it. Include the slug name (lowercase-hyphenated), \
            one-line description, step-by-step procedure, any pitfalls encountered, and \
            verified working commands.\n\
         3. If a skill for this already exists (listed below), decide if it should be \
            updated. Output the full updated SKILL.md.\n\
         4. If no skill is warranted, output: SKIP\n\n\
         Existing skills:\n{skill_index}\n\n\
         Task transcript:\n{transcript_text}\n\n\
         Respond ONLY with the SKILL.md content (starting with ---) or SKIP."
    );

    let request = CompletionRequest {
        messages: vec![ChatMessage {
            role: ChatRole::User,
            content: prompt,
        }],
        tools: Vec::new(),
        max_tokens: Some(4096),
        temperature: Some(0.3),
        stream: false,
    };

    let response = {
        let router = llm_router.read().await;
        match router.complete(&request).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "Skill reflection LLM call failed");
                return;
            }
        }
    };

    let content = response.content.trim();

    if content == "SKIP" || content.is_empty() {
        tracing::info!(
            event = "skill_skipped",
            tool_count,
            "Skill reflection: no skill warranted"
        );
        return;
    }

    match SkillDoc::parse(content) {
        Ok(mut doc) => {
            // Validate that the skill name is a safe lowercase slug.
            if doc.front_matter.name.is_empty()
                || !doc
                    .front_matter
                    .name
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
            {
                tracing::warn!(
                    skill = %doc.front_matter.name,
                    "Rejected reflected skill with invalid slug"
                );
                return;
            }
            let exists = store
                .get(&doc.front_matter.name)
                .await
                .ok()
                .flatten()
                .is_some();
            if exists {
                doc.front_matter.updated_at = Utc::now();
                doc.front_matter.use_count += 1;
            }
            let name = doc.front_matter.name.clone();
            match store.save(&doc).await {
                Ok(_) if exists => {
                    tracing::info!(
                        skill = %name,
                        event = "skill_updated",
                        "Skill updated by reflection"
                    );
                    refresh_system_prompt(&store, &live_system_prompt, &workspace_dir).await;
                }
                Ok(_) => {
                    tracing::info!(
                        skill = %name,
                        event = "skill_created",
                        "Skill created by reflection"
                    );
                    refresh_system_prompt(&store, &live_system_prompt, &workspace_dir).await;
                }
                Err(e) => tracing::warn!(error = %e, skill = %name, "Failed to save skill"),
            }
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                event = "skill_skipped",
                "Could not parse skill reflection response"
            );
        }
    }
}

/// Rebuild the skill index and regenerate the system prompt, then push it into
/// the live slot so that the next agent message sees the updated skill list.
async fn refresh_system_prompt(
    store: &Arc<SkillStore>,
    live_system_prompt: &Arc<RwLock<Option<String>>>,
    workspace_dir: &std::path::Path,
) {
    let loader = SkillLoader::new(Arc::clone(store));
    let skill_index = loader.level0_index().await;
    match workspace::build_system_prompt(
        workspace_dir,
        if skill_index.is_empty() {
            None
        } else {
            Some(&skill_index)
        },
    )
    .await
    {
        Ok(new_prompt) => {
            *live_system_prompt.write().await = Some(new_prompt);
            tracing::debug!("System prompt refreshed after skill reflection");
        }
        Err(e) => {
            tracing::warn!(error = %e, "Failed to rebuild system prompt after skill reflection");
        }
    }
}
