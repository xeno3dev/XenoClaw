//! Configuration model definitions for the VPS AI Agent Platform.
//!
//! All structs support serde deserialization from TOML format.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

/// Top-level platform configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformConfig {
    /// General platform settings.
    #[serde(default)]
    pub general: GeneralConfig,

    /// LLM provider configuration.
    #[serde(default)]
    pub llm: LlmConfig,

    /// Security settings.
    #[serde(default)]
    pub security: SecurityConfig,

    /// Coding module settings (optional — None means coding mode is disabled).
    #[serde(default)]
    pub coding: Option<CodingConfig>,

    /// Task scheduler settings.
    #[serde(default)]
    pub scheduler: SchedulerConfig,

    /// Web interface settings.
    #[serde(default)]
    pub web: WebConfig,

    /// API server settings.
    #[serde(default)]
    pub api: ApiConfig,

    /// Messaging integration settings.
    #[serde(default)]
    pub messaging: MessagingConfig,

    /// Monitoring and observability settings.
    #[serde(default)]
    pub monitoring: MonitoringConfig,

    /// Plugin system settings.
    #[serde(default)]
    pub plugins: PluginConfig,

    /// MCP (Model Context Protocol) settings.
    #[serde(default)]
    pub mcp: McpConfig,

    /// Skill library settings.
    #[serde(default)]
    pub skills: SkillsConfig,
}

impl Default for PlatformConfig {
    fn default() -> Self {
        Self {
            general: GeneralConfig::default(),
            llm: LlmConfig::default(),
            security: SecurityConfig::default(),
            coding: None,
            scheduler: SchedulerConfig::default(),
            web: WebConfig::default(),
            api: ApiConfig::default(),
            messaging: MessagingConfig::default(),
            monitoring: MonitoringConfig::default(),
            plugins: PluginConfig::default(),
            mcp: McpConfig::default(),
            skills: SkillsConfig::default(),
        }
    }
}

/// General platform settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralConfig {
    /// Name of the agent instance.
    #[serde(default = "default_agent_name")]
    pub agent_name: String,

    /// Directory for persistent data (SQLite, etc.).
    #[serde(default = "default_data_dir")]
    pub data_dir: PathBuf,

    /// Directory for log files.
    #[serde(default = "default_log_dir")]
    pub log_dir: PathBuf,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            agent_name: default_agent_name(),
            data_dir: default_data_dir(),
            log_dir: default_log_dir(),
        }
    }
}

fn default_agent_name() -> String {
    "xenoclaw".to_string()
}

fn default_data_dir() -> PathBuf {
    PathBuf::from("./data")
}

fn default_log_dir() -> PathBuf {
    PathBuf::from("./logs")
}

/// LLM provider configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    /// Configured LLM providers (1–10 required).
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            providers: Vec::new(),
        }
    }
}

/// Configuration for a single LLM provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Human-readable name for this provider.
    pub name: String,

    /// Type of provider (determines API protocol).
    pub provider_type: ProviderType,

    /// API key for authentication (not required for Ollama).
    #[serde(default)]
    pub api_key: Option<String>,

    /// Base URL for the provider's API.
    pub base_url: String,

    /// Model identifier to use.
    pub model: String,

    /// Priority for failover ordering (lower = higher priority).
    pub priority: u8,

    /// Request timeout in seconds (5–120, default 30).
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u32,

    /// Maximum tokens for completions (optional).
    #[serde(default)]
    pub max_tokens: Option<u32>,
}

fn default_timeout_seconds() -> u32 {
    30
}

/// Supported LLM provider types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderType {
    /// OpenAI-compatible API (works with OpenAI, Together, vLLM, etc.)
    OpenAiCompatible,
    /// Anthropic Claude API
    Anthropic,
    /// Local Ollama inference
    Ollama,
    /// Claude Code CLI (uses `claude` command — requires Claude Pro/Max subscription)
    ///
    /// **Deprecated**: CLI providers cannot participate in the tool execution loop.
    /// Use an API provider (Anthropic, OpenAI) for the LLM backend instead, and
    /// connect Claude Code to XenoClaw via MCP for tool access.
    ClaudeCode,
    /// GitHub Copilot CLI (uses `gh copilot` — requires Copilot subscription)
    ///
    /// **Deprecated**: CLI providers cannot participate in the tool execution loop.
    /// Use an API provider for the LLM backend and connect Copilot via MCP instead.
    CopilotCli,
    /// Google Gemini CLI (uses `gemini` command — requires Google account sign-in)
    ///
    /// **Deprecated**: CLI providers cannot participate in the tool execution loop.
    /// Use an API provider for the LLM backend and connect Gemini CLI via MCP instead.
    GeminiCli,
    /// OpenAI Codex CLI (uses `codex` command — requires OpenAI account / API key)
    ///
    /// **Deprecated**: CLI providers cannot participate in the tool execution loop.
    /// Use an API provider for the LLM backend and connect Codex CLI via MCP instead.
    CodexCli,
}

impl fmt::Display for ProviderType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProviderType::OpenAiCompatible => write!(f, "open_ai_compatible"),
            ProviderType::Anthropic => write!(f, "anthropic"),
            ProviderType::Ollama => write!(f, "ollama"),
            ProviderType::ClaudeCode => write!(f, "claude_code"),
            ProviderType::CopilotCli => write!(f, "copilot_cli"),
            ProviderType::GeminiCli => write!(f, "gemini_cli"),
            ProviderType::CodexCli => write!(f, "codex_cli"),
        }
    }
}

impl FromStr for ProviderType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "openai_compatible" | "open_ai_compatible" | "openaicompatible" | "openai" => {
                Ok(ProviderType::OpenAiCompatible)
            }
            "anthropic" | "claude" => Ok(ProviderType::Anthropic),
            "ollama" | "local" => Ok(ProviderType::Ollama),
            "claude_code" | "claudecode" => Ok(ProviderType::ClaudeCode),
            "copilot_cli" | "copilotcli" | "copilot" | "github_copilot" => {
                Ok(ProviderType::CopilotCli)
            }
            "gemini_cli" | "geminicli" | "gemini_cli_provider" => Ok(ProviderType::GeminiCli),
            "codex_cli" | "codexcli" | "openai_codex" => Ok(ProviderType::CodexCli),
            _ => Err(format!("unknown provider type: {s}")),
        }
    }
}

/// Security configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    /// Filesystem access rules.
    #[serde(default)]
    pub filesystem_rules: Vec<FilesystemRule>,

    /// Network access allowlist.
    #[serde(default)]
    pub network_allowlist: Vec<NetworkRule>,

    /// Resource limits for spawned processes.
    #[serde(default)]
    pub resource_limits: ResourceLimits,

    /// Session inactivity timeout in minutes (default: 30).
    #[serde(default = "default_session_timeout_minutes")]
    pub session_timeout_minutes: u32,

    /// Max consecutive failed auth attempts before lockout (default: 5).
    #[serde(default = "default_max_failed_attempts")]
    pub max_failed_attempts: u8,

    /// Lockout duration in minutes after max failed attempts (default: 15).
    #[serde(default = "default_lockout_minutes")]
    pub lockout_minutes: u16,

    /// Admin username for web UI login (default: "admin").
    #[serde(default = "default_admin_username")]
    pub admin_username: String,

    /// Bcrypt hash of the admin password. Empty = password login disabled.
    #[serde(default)]
    pub admin_password_hash: String,

    /// SHA-256 hash of the admin API key. Empty = API key login disabled.
    #[serde(default)]
    pub admin_key_hash: String,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            filesystem_rules: Vec::new(),
            network_allowlist: Vec::new(),
            resource_limits: ResourceLimits::default(),
            session_timeout_minutes: default_session_timeout_minutes(),
            max_failed_attempts: default_max_failed_attempts(),
            lockout_minutes: default_lockout_minutes(),
            admin_username: default_admin_username(),
            admin_password_hash: String::new(),
            admin_key_hash: String::new(),
        }
    }
}

fn default_admin_username() -> String {
    "admin".to_string()
}

fn default_session_timeout_minutes() -> u32 {
    30
}

fn default_max_failed_attempts() -> u8 {
    5
}

fn default_lockout_minutes() -> u16 {
    15
}

/// A filesystem access rule defining permissions for a directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilesystemRule {
    /// Path to the directory.
    pub path: PathBuf,
    /// Whether read access is allowed.
    #[serde(default)]
    pub read: bool,
    /// Whether write access is allowed.
    #[serde(default)]
    pub write: bool,
    /// Whether execute access is allowed.
    #[serde(default)]
    pub execute: bool,
}

/// A network access rule (host + optional port range).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkRule {
    /// Hostname or IP address.
    pub host: String,
    /// Port number (if None, all ports are allowed for this host).
    #[serde(default)]
    pub port: Option<u16>,
}

/// Resource limits for agent-spawned processes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLimits {
    /// Maximum memory in megabytes.
    #[serde(default = "default_max_memory_mb")]
    pub max_memory_mb: u32,

    /// Maximum CPU usage as percentage of a single core.
    #[serde(default = "default_max_cpu_percent")]
    pub max_cpu_percent: u8,

    /// Maximum number of concurrent processes.
    #[serde(default = "default_max_processes")]
    pub max_processes: u8,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            max_memory_mb: default_max_memory_mb(),
            max_cpu_percent: default_max_cpu_percent(),
            max_processes: default_max_processes(),
        }
    }
}

fn default_max_memory_mb() -> u32 {
    512
}

fn default_max_cpu_percent() -> u8 {
    80
}

fn default_max_processes() -> u8 {
    10
}

/// Coding module configuration (presence enables coding mode).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodingConfig {
    /// Directories the agent can operate on for file operations.
    #[serde(default)]
    pub workspace_dirs: Vec<PathBuf>,

    /// Directories containing git repositories.
    #[serde(default)]
    pub repository_dirs: Vec<PathBuf>,

    /// Commands explicitly allowed for shell execution.
    #[serde(default)]
    pub command_allowlist: Vec<String>,

    /// Commands explicitly blocked from shell execution.
    #[serde(default)]
    pub command_blocklist: Vec<String>,

    /// Maximum file size in MB for file operations (default: 10).
    #[serde(default = "default_max_file_size_mb")]
    pub max_file_size_mb: u32,

    /// Maximum concurrent shell processes (default: 5).
    #[serde(default = "default_max_concurrent_shells")]
    pub max_concurrent_shells: u8,

    /// Shell command timeout in seconds (default: 300).
    #[serde(default = "default_shell_timeout_seconds")]
    pub shell_timeout_seconds: u32,

    /// Number of file operations to keep in undo history (default: 50).
    #[serde(default = "default_undo_history_size")]
    pub undo_history_size: usize,

    /// Language server configurations.
    #[serde(default)]
    pub language_servers: Vec<LspConfig>,
}

fn default_max_file_size_mb() -> u32 {
    10
}

fn default_max_concurrent_shells() -> u8 {
    5
}

fn default_shell_timeout_seconds() -> u32 {
    300
}

fn default_undo_history_size() -> usize {
    50
}

/// Configuration for a language server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspConfig {
    /// Language identifier (e.g., "rust", "typescript").
    pub language: String,

    /// Command to start the language server.
    pub command: String,

    /// Arguments for the language server command.
    #[serde(default)]
    pub args: Vec<String>,
}

/// Task scheduler configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SchedulerConfig {
    /// Maximum number of tasks that can run concurrently.
    #[serde(default = "default_max_concurrent_tasks")]
    pub max_concurrent_tasks: u8,

    /// Default timeout for tasks in seconds.
    #[serde(default = "default_default_timeout_seconds")]
    pub default_timeout_seconds: u32,

    /// Maximum dependency chain depth.
    #[serde(default = "default_max_dependency_depth")]
    pub max_dependency_depth: u8,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            max_concurrent_tasks: default_max_concurrent_tasks(),
            default_timeout_seconds: default_default_timeout_seconds(),
            max_dependency_depth: default_max_dependency_depth(),
        }
    }
}

fn default_max_concurrent_tasks() -> u8 {
    10
}

fn default_default_timeout_seconds() -> u32 {
    300
}

fn default_max_dependency_depth() -> u8 {
    10
}

/// Web interface configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebConfig {
    /// Whether the web interface is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Host to bind the web server to.
    #[serde(default = "default_host")]
    pub host: String,

    /// Port for the web server.
    #[serde(default = "default_web_port")]
    pub port: u16,

    /// Directory containing the built web UI static files.
    /// Defaults to $XENOCLAW_WEB_DIR env var, then ./web/dist.
    #[serde(default = "default_web_dir")]
    pub dir: PathBuf,

    /// Path to TLS certificate file (optional).
    #[serde(default)]
    pub tls_cert: Option<PathBuf>,

    /// Path to TLS key file (optional).
    #[serde(default)]
    pub tls_key: Option<PathBuf>,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            host: default_host(),
            port: default_web_port(),
            dir: default_web_dir(),
            tls_cert: None,
            tls_key: None,
        }
    }
}

pub fn default_web_dir() -> PathBuf {
    std::env::var_os("XENOCLAW_WEB_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("./web/dist"))
}

fn default_true() -> bool {
    true
}

fn default_host() -> String {
    "0.0.0.0".to_string()
}

fn default_web_port() -> u16 {
    8080
}

/// API server configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiConfig {
    /// Host to bind the API server to.
    #[serde(default = "default_host")]
    pub host: String,

    /// Port for the API server.
    #[serde(default = "default_api_port")]
    pub port: u16,

    /// Default rate limit per API key (requests per minute).
    #[serde(default = "default_rate_limit")]
    pub rate_limit_per_minute: u32,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_api_port(),
            rate_limit_per_minute: default_rate_limit(),
        }
    }
}

fn default_api_port() -> u16 {
    9090
}

fn default_rate_limit() -> u32 {
    100
}

/// Messaging integration configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessagingConfig {
    /// Telegram bot configuration.
    #[serde(default)]
    pub telegram: Option<TelegramConfig>,

    /// Discord bot configuration.
    #[serde(default)]
    pub discord: Option<DiscordConfig>,

    /// WhatsApp bot configuration.
    #[serde(default)]
    pub whatsapp: Option<WhatsAppConfig>,
}

impl Default for MessagingConfig {
    fn default() -> Self {
        Self {
            telegram: None,
            discord: None,
            whatsapp: None,
        }
    }
}

/// Telegram bot configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramConfig {
    /// Bot token from BotFather.
    pub bot_token: String,
}

/// Discord bot configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscordConfig {
    /// Bot token from Discord Developer Portal.
    pub bot_token: String,
}

/// WhatsApp bot configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhatsAppConfig {
    /// Phone number for the WhatsApp bot.
    pub phone_number: String,
}

/// Monitoring and observability configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitoringConfig {
    /// Log level.
    #[serde(default = "default_log_level")]
    pub log_level: LogLevel,

    /// Log retention in days (1–365, default: 30).
    #[serde(default = "default_log_retention_days")]
    pub log_retention_days: u16,

    /// Maximum log file size in MB before rotation.
    #[serde(default = "default_max_log_file_size_mb")]
    pub max_log_file_size_mb: u32,

    /// Whether Prometheus metrics are enabled.
    #[serde(default = "default_true")]
    pub metrics_enabled: bool,

    /// Port for the Prometheus metrics endpoint.
    #[serde(default = "default_metrics_port")]
    pub metrics_port: u16,

    /// Alert rules.
    #[serde(default)]
    pub alert_rules: Vec<AlertRule>,
}

impl Default for MonitoringConfig {
    fn default() -> Self {
        Self {
            log_level: default_log_level(),
            log_retention_days: default_log_retention_days(),
            max_log_file_size_mb: default_max_log_file_size_mb(),
            metrics_enabled: true,
            metrics_port: default_metrics_port(),
            alert_rules: Vec::new(),
        }
    }
}

fn default_log_level() -> LogLevel {
    LogLevel::Info
}

fn default_log_retention_days() -> u16 {
    30
}

fn default_max_log_file_size_mb() -> u32 {
    100
}

fn default_metrics_port() -> u16 {
    9100
}

/// Log level for the monitoring system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
    Fatal,
}

impl fmt::Display for LogLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LogLevel::Debug => write!(f, "debug"),
            LogLevel::Info => write!(f, "info"),
            LogLevel::Warn => write!(f, "warn"),
            LogLevel::Error => write!(f, "error"),
            LogLevel::Fatal => write!(f, "fatal"),
        }
    }
}

impl FromStr for LogLevel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "debug" => Ok(LogLevel::Debug),
            "info" => Ok(LogLevel::Info),
            "warn" | "warning" => Ok(LogLevel::Warn),
            "error" => Ok(LogLevel::Error),
            "fatal" => Ok(LogLevel::Fatal),
            _ => Err(format!("unknown log level: {s}")),
        }
    }
}

/// An alert rule for monitoring.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertRule {
    /// Name of the alert rule.
    pub name: String,

    /// Metric to monitor.
    pub metric: String,

    /// Threshold value that triggers the alert.
    pub threshold: f64,

    /// Comparison operator.
    pub operator: AlertOperator,
}

/// Comparison operator for alert rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertOperator {
    GreaterThan,
    LessThan,
    GreaterThanOrEqual,
    LessThanOrEqual,
}

/// Plugin system configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginConfig {
    /// Whether the plugin system is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Directory to scan for plugins.
    #[serde(default = "default_plugins_dir")]
    pub directory: PathBuf,

    /// Hot-reload detection interval in seconds.
    #[serde(default = "default_reload_interval")]
    pub reload_interval_seconds: u32,
}

impl Default for PluginConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            directory: default_plugins_dir(),
            reload_interval_seconds: default_reload_interval(),
        }
    }
}

fn default_plugins_dir() -> PathBuf {
    // Prefer $XENOCLAW_DATA_DIR/plugins (set by the systemd unit to /var/lib/xenoclaw).
    // Fall back to ./plugins for local dev runs where the env var is absent.
    std::env::var_os("XENOCLAW_DATA_DIR")
        .map(|d| PathBuf::from(d).join("plugins"))
        .unwrap_or_else(|| PathBuf::from("./plugins"))
}

fn default_reload_interval() -> u32 {
    10
}

/// MCP (Model Context Protocol) configuration.
///
/// XenoClaw can act as both an MCP server (exposing tools to external clients
/// like Claude Code and Copilot) and an MCP client (connecting to external
/// MCP servers to gain additional tools).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpConfig {
    /// Whether the MCP server is enabled (exposes XenoClaw tools to external clients).
    #[serde(default = "default_mcp_server_enabled")]
    pub server_enabled: bool,

    /// Transport for the MCP server: "stdio" or "http".
    #[serde(default = "default_mcp_transport")]
    pub server_transport: String,

    /// Port for HTTP+SSE MCP server transport (only used if transport = "http").
    #[serde(default = "default_mcp_port")]
    pub server_port: u16,

    /// External MCP servers to connect to as a client.
    #[serde(default)]
    pub servers: Vec<McpServerConfig>,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            server_enabled: true,
            server_transport: default_mcp_transport(),
            server_port: default_mcp_port(),
            servers: Vec::new(),
        }
    }
}

fn default_mcp_server_enabled() -> bool {
    true
}

fn default_mcp_transport() -> String {
    "stdio".to_string()
}

fn default_mcp_port() -> u16 {
    3100
}

/// Configuration for an external MCP server that XenoClaw connects to as a client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Human-readable name for this MCP server.
    pub name: String,

    /// Command to start the MCP server (e.g., "uvx", "npx", "node").
    pub command: String,

    /// Arguments for the command.
    #[serde(default)]
    pub args: Vec<String>,

    /// Environment variables to set for the server process.
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,

    /// Whether this server is disabled.
    #[serde(default)]
    pub disabled: bool,

    /// Tool names to auto-approve (skip confirmation).
    #[serde(default)]
    pub auto_approve: Vec<String>,
}

/// Skill library configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillsConfig {
    /// Enable the auto-skill-creation loop after complex tasks.
    #[serde(default = "default_true")]
    pub auto_create: bool,

    /// Minimum tool-call count in a session before reflection is triggered.
    #[serde(default = "default_skill_threshold")]
    pub auto_create_threshold: u32,

    /// Cron expression for the autonomous curator (default: weekly Sunday midnight).
    #[serde(default = "default_curator_schedule")]
    pub curator_schedule: String,

    /// Enable the autonomous background curator.
    #[serde(default = "default_true")]
    pub curator_enabled: bool,

    /// Override the default skills directory (`~/.xenoclaw/skills/`).
    #[serde(default)]
    pub skills_dir: Option<PathBuf>,
}

impl Default for SkillsConfig {
    fn default() -> Self {
        Self {
            auto_create: true,
            auto_create_threshold: default_skill_threshold(),
            curator_schedule: default_curator_schedule(),
            curator_enabled: true,
            skills_dir: None,
        }
    }
}

fn default_skill_threshold() -> u32 {
    15
}

fn default_curator_schedule() -> String {
    "0 0 * * 0".to_string()
}

/// Session source identifier — tracks where a session originated from.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionSource {
    /// Web UI session
    Web,
    /// API client session
    Api,
    /// TUI (terminal) session
    Tui,
    /// Claude Code CLI session
    ClaudeCode,
    /// GitHub Copilot CLI session
    CopilotCli,
    /// Telegram messaging session
    Telegram { chat_id: String },
    /// Discord messaging session
    Discord { channel_id: String },
    /// WhatsApp messaging session
    WhatsApp { phone: String },
}
