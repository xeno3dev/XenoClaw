//! Zero-downtime configuration reload support.
//!
//! This module provides the ability to reload a subset of configuration settings
//! at runtime without restarting the service. Only settings that don't affect
//! service bindings, database connections, or authentication can be hot-reloaded.
//!
//! # Reloadable Settings
//!
//! - Rate limit (requests per minute, burst)
//! - Log level
//! - Plugin enabled flag
//! - Monitoring refresh intervals (metrics port is NOT reloadable)
//! - Alert rules
//! - `mcp.server_enabled` — MCP server toggle (enables/disables the server)
//! - `mcp.servers` — External MCP server list (triggers reconnection to new/removed servers)
//!
//! # Non-Reloadable Settings (require full restart)
//!
//! - Bind address/port (web, API, metrics)
//! - Database path
//! - Authentication keys and TLS certificates
//! - LLM provider configurations
//! - Security filesystem/network rules
//! - `mcp.server_transport` — MCP server transport type (requires restart)
//! - `mcp.server_port` — MCP server port (requires restart)
//! - `security.admin_username` — Admin username (security — requires restart)
//! - `security.admin_password_hash` — Admin password hash (security — requires restart)
//!
//! Requirements: 12.6

use std::path::Path;

use tracing::info;

use super::models::{AlertRule, LogLevel, McpServerConfig, PlatformConfig};
use super::validation::ConfigError;

/// Contains only the settings that can be hot-reloaded without a service restart.
///
/// These settings are safe to change at runtime because they don't affect
/// service bindings, database connections, or authentication state.
#[derive(Debug, Clone)]
pub struct ReloadableConfig {
    /// API rate limit: requests per minute.
    pub rate_limit_requests_per_minute: u32,

    /// API rate limit: burst allowance (same as requests_per_minute for token bucket).
    pub rate_limit_burst: u32,

    /// Log level for the monitoring system.
    pub log_level: LogLevel,

    /// Whether the plugin system is enabled.
    pub plugin_enabled: bool,

    /// Monitoring: log retention in days.
    pub log_retention_days: u16,

    /// Monitoring: max log file size in MB before rotation.
    pub max_log_file_size_mb: u32,

    /// Monitoring: whether metrics collection is enabled.
    pub metrics_enabled: bool,

    /// Monitoring: alert rules (can be updated at runtime).
    pub alert_rules: Vec<AlertRule>,

    /// Whether the MCP server is enabled (can be toggled at runtime).
    /// NOTE: Changing server_transport or server_port requires restart.
    pub mcp_server_enabled: bool,

    /// Updated list of external MCP servers to connect to.
    /// Changes here will trigger reconnection to new/removed servers.
    pub mcp_servers: Vec<McpServerConfig>,
}

/// Reload configuration from a TOML file, extracting only the reloadable settings.
///
/// This function:
/// 1. Reads the TOML config file
/// 2. Parses it into a full `PlatformConfig`
/// 3. Extracts only the reloadable settings
/// 4. Validates the reloadable values
/// 5. Returns the new values
///
/// # Errors
///
/// Returns `ConfigError` if the file cannot be read, parsed, or if the
/// reloadable settings fail validation.
pub fn reload_config(config_path: &Path) -> Result<ReloadableConfig, ConfigError> {
    let content = std::fs::read_to_string(config_path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            ConfigError::FileNotFound(config_path.to_path_buf())
        } else {
            ConfigError::IoError(e.to_string())
        }
    })?;

    reload_config_from_str(&content)
}

/// Reload configuration from a TOML string, extracting only the reloadable settings.
///
/// This is useful for testing or when the config content is already in memory.
pub fn reload_config_from_str(toml_content: &str) -> Result<ReloadableConfig, ConfigError> {
    let config: PlatformConfig =
        toml::from_str(toml_content).map_err(|e| ConfigError::ParseError(e.to_string()))?;

    let reloadable = extract_reloadable(&config);
    validate_reloadable(&reloadable)?;

    Ok(reloadable)
}

/// Extract the reloadable settings from a full platform configuration.
fn extract_reloadable(config: &PlatformConfig) -> ReloadableConfig {
    ReloadableConfig {
        rate_limit_requests_per_minute: config.api.rate_limit_per_minute,
        rate_limit_burst: config.api.rate_limit_per_minute, // burst = rate limit
        log_level: config.monitoring.log_level,
        plugin_enabled: config.plugins.enabled,
        log_retention_days: config.monitoring.log_retention_days,
        max_log_file_size_mb: config.monitoring.max_log_file_size_mb,
        metrics_enabled: config.monitoring.metrics_enabled,
        alert_rules: config.monitoring.alert_rules.clone(),
        mcp_server_enabled: config.mcp.server_enabled,
        mcp_servers: config.mcp.servers.clone(),
    }
}

/// Validate the reloadable configuration values.
///
/// Returns `Ok(())` if all values are valid, or a `ConfigError::Validation`
/// with details about what's wrong.
fn validate_reloadable(reloadable: &ReloadableConfig) -> Result<(), ConfigError> {
    use super::validation::{ConfigValidationError, ValidationError};

    let mut errors = Vec::new();

    if reloadable.rate_limit_requests_per_minute == 0 {
        errors.push(ValidationError {
            setting: "api.rate_limit_per_minute".to_string(),
            reason: "rate limit must be greater than 0".to_string(),
        });
    }

    if reloadable.log_retention_days == 0 || reloadable.log_retention_days > 365 {
        errors.push(ValidationError {
            setting: "monitoring.log_retention_days".to_string(),
            reason: format!(
                "log retention must be between 1 and 365 days, got {}",
                reloadable.log_retention_days
            ),
        });
    }

    if reloadable.max_log_file_size_mb == 0 {
        errors.push(ValidationError {
            setting: "monitoring.max_log_file_size_mb".to_string(),
            reason: "max log file size must be greater than 0".to_string(),
        });
    }

    for (i, rule) in reloadable.alert_rules.iter().enumerate() {
        if rule.name.trim().is_empty() {
            errors.push(ValidationError {
                setting: format!("monitoring.alert_rules[{i}].name"),
                reason: "alert rule name must not be empty".to_string(),
            });
        }
        if rule.metric.trim().is_empty() {
            errors.push(ValidationError {
                setting: format!("monitoring.alert_rules[{i}].metric"),
                reason: "alert rule metric must not be empty".to_string(),
            });
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(ConfigError::Validation(ConfigValidationError { errors }))
    }
}

/// Apply reloadable settings to the running platform configuration.
///
/// This function updates only the fields that are safe to change at runtime.
/// It logs each setting that was actually changed.
pub fn apply_reload(current: &mut PlatformConfig, reloadable: ReloadableConfig) {
    let mut changes = Vec::new();

    // Rate limits
    if current.api.rate_limit_per_minute != reloadable.rate_limit_requests_per_minute {
        changes.push(format!(
            "api.rate_limit_per_minute: {} -> {}",
            current.api.rate_limit_per_minute, reloadable.rate_limit_requests_per_minute
        ));
        current.api.rate_limit_per_minute = reloadable.rate_limit_requests_per_minute;
    }

    // Log level
    if current.monitoring.log_level != reloadable.log_level {
        changes.push(format!(
            "monitoring.log_level: {} -> {}",
            current.monitoring.log_level, reloadable.log_level
        ));
        current.monitoring.log_level = reloadable.log_level;
    }

    // Plugin enabled
    if current.plugins.enabled != reloadable.plugin_enabled {
        changes.push(format!(
            "plugins.enabled: {} -> {}",
            current.plugins.enabled, reloadable.plugin_enabled
        ));
        current.plugins.enabled = reloadable.plugin_enabled;
    }

    // Log retention
    if current.monitoring.log_retention_days != reloadable.log_retention_days {
        changes.push(format!(
            "monitoring.log_retention_days: {} -> {}",
            current.monitoring.log_retention_days, reloadable.log_retention_days
        ));
        current.monitoring.log_retention_days = reloadable.log_retention_days;
    }

    // Max log file size
    if current.monitoring.max_log_file_size_mb != reloadable.max_log_file_size_mb {
        changes.push(format!(
            "monitoring.max_log_file_size_mb: {} -> {}",
            current.monitoring.max_log_file_size_mb, reloadable.max_log_file_size_mb
        ));
        current.monitoring.max_log_file_size_mb = reloadable.max_log_file_size_mb;
    }

    // Metrics enabled
    if current.monitoring.metrics_enabled != reloadable.metrics_enabled {
        changes.push(format!(
            "monitoring.metrics_enabled: {} -> {}",
            current.monitoring.metrics_enabled, reloadable.metrics_enabled
        ));
        current.monitoring.metrics_enabled = reloadable.metrics_enabled;
    }

    // Alert rules (always replace — no easy equality check)
    let old_count = current.monitoring.alert_rules.len();
    let new_count = reloadable.alert_rules.len();
    if old_count != new_count {
        changes.push(format!(
            "monitoring.alert_rules: {} rules -> {} rules",
            old_count, new_count
        ));
    }
    current.monitoring.alert_rules = reloadable.alert_rules;

    // MCP server enabled
    if current.mcp.server_enabled != reloadable.mcp_server_enabled {
        changes.push(format!(
            "mcp.server_enabled: {} -> {}",
            current.mcp.server_enabled, reloadable.mcp_server_enabled
        ));
        current.mcp.server_enabled = reloadable.mcp_server_enabled;
    }

    // MCP external servers (compare by count and names)
    let old_server_names: Vec<&str> = current
        .mcp
        .servers
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    let new_server_names: Vec<&str> = reloadable
        .mcp_servers
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    if old_server_names != new_server_names {
        changes.push(format!(
            "mcp.servers: {:?} -> {:?}",
            old_server_names, new_server_names
        ));
        current.mcp.servers = reloadable.mcp_servers;
    } else {
        // Even if names match, replace in case other fields changed (command, args, env, etc.)
        current.mcp.servers = reloadable.mcp_servers;
    }
    // NOTE: Actual MCP server restart/client reconnection must be handled by the caller
    // (main.rs) after `apply_reload` returns. This function only updates the config state.

    if changes.is_empty() {
        info!("Configuration reload: no changes detected");
    } else {
        for change in &changes {
            info!(change = %change, "Configuration reloaded");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::models::*;

    fn minimal_valid_toml() -> &'static str {
        r#"
[general]
agent_name = "xenoclaw"

[llm]
[[llm.providers]]
name = "ollama-local"
provider_type = "ollama"
base_url = "http://localhost:11434"
model = "llama3"
priority = 1
timeout_seconds = 30

[security]

[scheduler]

[web]

[api]
rate_limit_per_minute = 200

[monitoring]
log_level = "debug"
log_retention_days = 60
max_log_file_size_mb = 50
metrics_enabled = true
metrics_port = 9100

[plugins]
enabled = false
"#
    }

    fn minimal_valid_config() -> PlatformConfig {
        PlatformConfig {
            general: GeneralConfig::default(),
            llm: LlmConfig {
                providers: vec![ProviderConfig {
                    name: "ollama-local".to_string(),
                    provider_type: ProviderType::Ollama,
                    api_key: None,
                    base_url: "http://localhost:11434".to_string(),
                    model: "llama3".to_string(),
                    priority: 1,
                    timeout_seconds: 30,
                    max_tokens: None,
                }],
            },
            security: SecurityConfig::default(),
            coding: None,
            scheduler: SchedulerConfig::default(),
            web: WebConfig::default(),
            api: ApiConfig::default(),
            messaging: MessagingConfig::default(),
            monitoring: MonitoringConfig::default(),
            plugins: PluginConfig::default(),
            mcp: McpConfig::default(),
        }
    }

    #[test]
    fn test_reload_config_from_str_valid() {
        let result = reload_config_from_str(minimal_valid_toml());
        assert!(result.is_ok(), "Expected Ok, got: {:?}", result.err());

        let reloadable = result.unwrap();
        assert_eq!(reloadable.rate_limit_requests_per_minute, 200);
        assert_eq!(reloadable.log_level, LogLevel::Debug);
        assert!(!reloadable.plugin_enabled);
        assert_eq!(reloadable.log_retention_days, 60);
        assert_eq!(reloadable.max_log_file_size_mb, 50);
        assert!(reloadable.metrics_enabled);
    }

    #[test]
    fn test_reload_config_from_str_invalid_rate_limit() {
        let toml = r#"
[general]
agent_name = "xenoclaw"

[llm]
[[llm.providers]]
name = "ollama-local"
provider_type = "ollama"
base_url = "http://localhost:11434"
model = "llama3"
priority = 1
timeout_seconds = 30

[api]
rate_limit_per_minute = 0

[monitoring]
log_level = "info"
"#;
        let result = reload_config_from_str(toml);
        assert!(result.is_err());
        if let Err(ConfigError::Validation(v)) = result {
            assert!(v
                .errors
                .iter()
                .any(|e| e.setting == "api.rate_limit_per_minute"));
        }
    }

    #[test]
    fn test_reload_config_from_str_invalid_retention() {
        let toml = r#"
[general]
agent_name = "xenoclaw"

[llm]
[[llm.providers]]
name = "ollama-local"
provider_type = "ollama"
base_url = "http://localhost:11434"
model = "llama3"
priority = 1
timeout_seconds = 30

[monitoring]
log_retention_days = 400
"#;
        let result = reload_config_from_str(toml);
        assert!(result.is_err());
        if let Err(ConfigError::Validation(v)) = result {
            assert!(v
                .errors
                .iter()
                .any(|e| e.setting == "monitoring.log_retention_days"));
        }
    }

    #[test]
    fn test_reload_config_file_not_found() {
        let result = reload_config(Path::new("/nonexistent/path/config.toml"));
        assert!(matches!(result, Err(ConfigError::FileNotFound(_))));
    }

    #[test]
    fn test_apply_reload_changes_rate_limit() {
        let mut config = minimal_valid_config();
        assert_eq!(config.api.rate_limit_per_minute, 100); // default

        let reloadable = ReloadableConfig {
            rate_limit_requests_per_minute: 500,
            rate_limit_burst: 500,
            log_level: config.monitoring.log_level,
            plugin_enabled: config.plugins.enabled,
            log_retention_days: config.monitoring.log_retention_days,
            max_log_file_size_mb: config.monitoring.max_log_file_size_mb,
            metrics_enabled: config.monitoring.metrics_enabled,
            alert_rules: vec![],
            mcp_server_enabled: config.mcp.server_enabled,
            mcp_servers: config.mcp.servers.clone(),
        };

        apply_reload(&mut config, reloadable);
        assert_eq!(config.api.rate_limit_per_minute, 500);
    }

    #[test]
    fn test_apply_reload_changes_log_level() {
        let mut config = minimal_valid_config();
        assert_eq!(config.monitoring.log_level, LogLevel::Info); // default

        let reloadable = ReloadableConfig {
            rate_limit_requests_per_minute: config.api.rate_limit_per_minute,
            rate_limit_burst: config.api.rate_limit_per_minute,
            log_level: LogLevel::Debug,
            plugin_enabled: config.plugins.enabled,
            log_retention_days: config.monitoring.log_retention_days,
            max_log_file_size_mb: config.monitoring.max_log_file_size_mb,
            metrics_enabled: config.monitoring.metrics_enabled,
            alert_rules: vec![],
            mcp_server_enabled: config.mcp.server_enabled,
            mcp_servers: config.mcp.servers.clone(),
        };

        apply_reload(&mut config, reloadable);
        assert_eq!(config.monitoring.log_level, LogLevel::Debug);
    }

    #[test]
    fn test_apply_reload_changes_plugin_enabled() {
        let mut config = minimal_valid_config();
        assert!(config.plugins.enabled); // default is true

        let reloadable = ReloadableConfig {
            rate_limit_requests_per_minute: config.api.rate_limit_per_minute,
            rate_limit_burst: config.api.rate_limit_per_minute,
            log_level: config.monitoring.log_level,
            plugin_enabled: false,
            log_retention_days: config.monitoring.log_retention_days,
            max_log_file_size_mb: config.monitoring.max_log_file_size_mb,
            metrics_enabled: config.monitoring.metrics_enabled,
            alert_rules: vec![],
            mcp_server_enabled: config.mcp.server_enabled,
            mcp_servers: config.mcp.servers.clone(),
        };

        apply_reload(&mut config, reloadable);
        assert!(!config.plugins.enabled);
    }

    #[test]
    fn test_apply_reload_no_changes() {
        let mut config = minimal_valid_config();
        let original = config.clone();

        let reloadable = ReloadableConfig {
            rate_limit_requests_per_minute: config.api.rate_limit_per_minute,
            rate_limit_burst: config.api.rate_limit_per_minute,
            log_level: config.monitoring.log_level,
            plugin_enabled: config.plugins.enabled,
            log_retention_days: config.monitoring.log_retention_days,
            max_log_file_size_mb: config.monitoring.max_log_file_size_mb,
            metrics_enabled: config.monitoring.metrics_enabled,
            alert_rules: config.monitoring.alert_rules.clone(),
            mcp_server_enabled: config.mcp.server_enabled,
            mcp_servers: config.mcp.servers.clone(),
        };

        apply_reload(&mut config, reloadable);

        // Verify nothing changed
        assert_eq!(
            config.api.rate_limit_per_minute,
            original.api.rate_limit_per_minute
        );
        assert_eq!(config.monitoring.log_level, original.monitoring.log_level);
        assert_eq!(config.plugins.enabled, original.plugins.enabled);
        assert_eq!(config.mcp.server_enabled, original.mcp.server_enabled);
    }

    #[test]
    fn test_apply_reload_multiple_changes() {
        let mut config = minimal_valid_config();

        let reloadable = ReloadableConfig {
            rate_limit_requests_per_minute: 250,
            rate_limit_burst: 250,
            log_level: LogLevel::Warn,
            plugin_enabled: false,
            log_retention_days: 7,
            max_log_file_size_mb: 200,
            metrics_enabled: false,
            alert_rules: vec![AlertRule {
                name: "high_cpu".to_string(),
                metric: "cpu_percent".to_string(),
                threshold: 90.0,
                operator: AlertOperator::GreaterThan,
            }],
            mcp_server_enabled: false,
            mcp_servers: vec![McpServerConfig {
                name: "test-server".to_string(),
                command: "npx".to_string(),
                args: vec!["-y".to_string(), "test-mcp".to_string()],
                env: std::collections::HashMap::new(),
                disabled: false,
                auto_approve: vec![],
            }],
        };

        apply_reload(&mut config, reloadable);

        assert_eq!(config.api.rate_limit_per_minute, 250);
        assert_eq!(config.monitoring.log_level, LogLevel::Warn);
        assert!(!config.plugins.enabled);
        assert_eq!(config.monitoring.log_retention_days, 7);
        assert_eq!(config.monitoring.max_log_file_size_mb, 200);
        assert!(!config.monitoring.metrics_enabled);
        assert_eq!(config.monitoring.alert_rules.len(), 1);
        assert_eq!(config.monitoring.alert_rules[0].name, "high_cpu");
        assert!(!config.mcp.server_enabled);
        assert_eq!(config.mcp.servers.len(), 1);
        assert_eq!(config.mcp.servers[0].name, "test-server");
    }

    #[test]
    fn test_apply_reload_mcp_fields_extracted_and_applied() {
        let mut config = minimal_valid_config();
        assert!(config.mcp.server_enabled); // default is true
        assert!(config.mcp.servers.is_empty()); // default is empty

        // Test disabling MCP server
        let reloadable = ReloadableConfig {
            rate_limit_requests_per_minute: config.api.rate_limit_per_minute,
            rate_limit_burst: config.api.rate_limit_per_minute,
            log_level: config.monitoring.log_level,
            plugin_enabled: config.plugins.enabled,
            log_retention_days: config.monitoring.log_retention_days,
            max_log_file_size_mb: config.monitoring.max_log_file_size_mb,
            metrics_enabled: config.monitoring.metrics_enabled,
            alert_rules: vec![],
            mcp_server_enabled: false,
            mcp_servers: vec![
                McpServerConfig {
                    name: "filesystem".to_string(),
                    command: "npx".to_string(),
                    args: vec![
                        "-y".to_string(),
                        "@modelcontextprotocol/server-filesystem".to_string(),
                    ],
                    env: std::collections::HashMap::new(),
                    disabled: false,
                    auto_approve: vec!["read_file".to_string()],
                },
                McpServerConfig {
                    name: "github".to_string(),
                    command: "npx".to_string(),
                    args: vec![
                        "-y".to_string(),
                        "@modelcontextprotocol/server-github".to_string(),
                    ],
                    env: std::collections::HashMap::new(),
                    disabled: false,
                    auto_approve: vec![],
                },
            ],
        };

        apply_reload(&mut config, reloadable);

        assert!(!config.mcp.server_enabled);
        assert_eq!(config.mcp.servers.len(), 2);
        assert_eq!(config.mcp.servers[0].name, "filesystem");
        assert_eq!(config.mcp.servers[1].name, "github");
        assert_eq!(
            config.mcp.servers[0].auto_approve,
            vec!["read_file".to_string()]
        );
    }

    #[test]
    fn test_extract_reloadable_includes_mcp_fields() {
        let mut config = minimal_valid_config();
        config.mcp.server_enabled = false;
        config.mcp.servers = vec![McpServerConfig {
            name: "test-mcp".to_string(),
            command: "node".to_string(),
            args: vec!["server.js".to_string()],
            env: std::collections::HashMap::new(),
            disabled: false,
            auto_approve: vec![],
        }];

        let reloadable = extract_reloadable(&config);

        assert!(!reloadable.mcp_server_enabled);
        assert_eq!(reloadable.mcp_servers.len(), 1);
        assert_eq!(reloadable.mcp_servers[0].name, "test-mcp");
        assert_eq!(reloadable.mcp_servers[0].command, "node");
    }

    #[test]
    fn test_reload_config_from_str_with_mcp_section() {
        let toml = r#"
[general]
agent_name = "xenoclaw"

[llm]
[[llm.providers]]
name = "ollama-local"
provider_type = "ollama"
base_url = "http://localhost:11434"
model = "llama3"
priority = 1
timeout_seconds = 30

[security]

[scheduler]

[web]

[api]
rate_limit_per_minute = 200

[monitoring]
log_level = "debug"
log_retention_days = 60
max_log_file_size_mb = 50
metrics_enabled = true
metrics_port = 9100

[plugins]
enabled = false

[mcp]
server_enabled = false
server_transport = "http"
server_port = 3200

[[mcp.servers]]
name = "filesystem"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem"]
"#;
        let result = reload_config_from_str(toml);
        assert!(result.is_ok(), "Expected Ok, got: {:?}", result.err());

        let reloadable = result.unwrap();
        assert!(!reloadable.mcp_server_enabled);
        assert_eq!(reloadable.mcp_servers.len(), 1);
        assert_eq!(reloadable.mcp_servers[0].name, "filesystem");
    }
}
